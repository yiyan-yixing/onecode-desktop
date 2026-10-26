//! ACP 客户端（Agent Client Protocol）—— 目标 ① 点名的"ACP 类"。
//!
//! 协议依据：<https://agentclientprotocol.com/protocol/v1/overview>（v1）。
//! JSON-RPC 2.0 over stdio，行分隔。流程：
//!
//! ```text
//! client → agent : initialize            （协商版本与能力）
//! client → agent : session/new           （建会话，给 cwd）
//! client → agent : session/prompt        （发一句）
//! agent  → client: session/update *       （流式：文本片段 / 工具调用 / 用量）
//! agent  → client: session/request_permission （要权限 → 我们答复）
//! agent  → client: session/prompt 响应    （stopReason，一轮结束）
//! ```
//!
//! ## 两个刻意的"不做"
//!
//! 1. **不宣告 fs / terminal 能力**。协议里未宣告即不支持（"Clients and Agents
//!    MUST treat all capabilities omitted in the initialize request as UNSUPPORTED"），
//!    于是 Agent 不会通过我们去读写文件、起终端 —— 它用自己的工具。
//!    这符合本产品的默认纪律：**默认不给**（见第 24 刀的姿态审计）。
//! 2. **权限请求默认拒绝**。要权限时我们优先挑 `reject_once`/`reject_always` 选项；
//!    没有可拒绝的选项就回 `cancelled`。**绝不默认放行。**
//!
//! ## 证据分级
//!
//! 本机**没有**真实的 ACP Agent（`claude-agent-acp` / `codex-acp` 都不存在，第 11 刀
//! 已取证），所以这一份的验证是**对着按规范写的 mock agent** 做的**协议一致性测试**：
//! 它证明"我们发出的与回应的消息符合 v1 规范"，**不证明**与某个真实 Agent 互通。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// ACP v1 的协议版本号（规范：单个整数，只在破坏性变更时递增）。
pub const PROTOCOL_VERSION: i64 = 1;

/// 我们这一侧的实现信息（规范 SHOULD 提供）。
const CLIENT_NAME: &str = "onecode-desktop";

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AcpReport {
    pub ok: bool,
    pub agent_name: String,
    pub protocol_version: i64,
    pub agent_load_session: bool,
    pub session_id: String,
    /// 最终回复（把 agent_message_chunk 拼起来）
    pub reply: String,
    /// 思考片段（agent_thought_chunk）—— 与回复分开，不混进回复
    pub thought: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: String,
    /// 引擎自报的累计花费（`usage_update` 的 `cost.amount`；不换算、不估）
    pub cost_usd: f64,
    pub used_tokens: u64,
    pub size_tokens: u64,
    /// 收到的权限请求次数
    pub permission_requests: usize,
    /// 我们**拒绝**的次数（默认策略下应等于请求次数）
    pub permission_denials: usize,
    /// 我们无法服务的请求数（必须**逐个回错误**，不能不理）
    pub unsupported_requests: usize,
    pub wall_ms: u64,
    pub error: Option<String>,
    pub stderr_tail: String,
    /// 关键动作留痕（便于人核对我们到底回了什么）
    pub actions: Vec<String>,
}

fn line(value: serde_json::Value) -> String {
    serde_json::to_string(&value).unwrap_or_default()
}

/// 跑一轮 ACP 对话（一次 prompt）。
///
/// `policy` 见 [`PermissionPolicy`]；默认拒绝。
pub fn run(
    cmd: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    prompt: &str,
    policy: PermissionPolicy,
    timeout: Duration,
) -> AcpReport {
    let start = Instant::now();
    let mut rep = AcpReport {
        ok: false,
        agent_name: String::new(),
        protocol_version: 0,
        agent_load_session: false,
        session_id: String::new(),
        reply: String::new(),
        thought: String::new(),
        tool_calls: Vec::new(),
        stop_reason: String::new(),
        cost_usd: 0.0,
        used_tokens: 0,
        size_tokens: 0,
        permission_requests: 0,
        permission_denials: 0,
        unsupported_requests: 0,
        wall_ms: 0,
        error: None,
        stderr_tail: String::new(),
        actions: Vec::new(),
    };

    let mut command = Command::new(cmd);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            rep.error = Some(format!("启动 ACP agent 失败：{e}"));
            rep.wall_ms = start.elapsed().as_millis() as u64;
            return rep;
        }
    };
    let pid = child.id();
    let Some(stdin) = child.stdin.take() else {
        rep.error = Some("拿不到 agent 的 stdin".to_string());
        return rep;
    };
    let Some(stdout) = child.stdout.take() else {
        rep.error = Some("拿不到 agent 的 stdout".to_string());
        return rep;
    };
    let mut stdin = stdin;

    // stdout 按行读（JSON-RPC 行分隔）
    let (tx, rx): (mpsc::Sender<String>, Receiver<String>) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let r = BufReader::new(stdout);
        for l in r.lines() {
            match l {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sb = stderr_buf.clone();
    let err_reader = {
        let err = child.stderr.take();
        std::thread::spawn(move || {
            if let Some(err) = err {
                let r = BufReader::new(err);
                for l in r.lines().map_while(Result::ok) {
                    if let Ok(mut b) = sb.lock() {
                        b.push_str(&l);
                        b.push('\n');
                    }
                }
            }
        })
    };

    let deadline = start + timeout;
    let mut next_id: i64 = 0;
    let send = |stdin: &mut std::process::ChildStdin, v: serde_json::Value| -> bool {
        let s = line(v);
        if stdin.write_all(s.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() {
            return false;
        }
        stdin.flush().is_ok()
    };

    // ── ① initialize ──
    // 刻意**不宣告** fs / terminal：未宣告即不支持 → agent 不会经我们读写文件。
    let init_id = next_id;
    next_id += 1;
    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "title": "OneCode 调度台", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    rep.actions.push(format!("→ initialize (protocolVersion {PROTOCOL_VERSION}，不宣告 fs/terminal)"));
    if !send(&mut stdin, init) {
        rep.error = Some("写入 initialize 失败".to_string());
    }

    // ── 主循环：处理响应 / 通知 / agent 发来的请求 ──
    let mut session_id: Option<String> = None;
    // ★ 必须**先等 initialize 的响应**再发 session/new ——
    // 早先这里把 pending 留成 None，于是循环立刻发 session/new，
    // initialize 的响应因 id 不匹配被当成"迟到的答复"丢掉（版本/能力全空）。
    // 规范：Sessions **MUST** 在初始化完成之后才能建立。
    let mut pending: Option<i64> = Some(init_id);
    let mut phase = "initialize";

    while rep.error.is_none() && Instant::now() < deadline {
        // 该发下一步了吗？
        if pending.is_none() {
            match phase {
                "initialize" => {
                    let id = next_id;
                    next_id += 1;
                    let v = serde_json::json!({
                        "jsonrpc": "2.0", "id": id, "method": "session/new",
                        "params": { "cwd": cwd.to_string_lossy(), "mcpServers": [] }
                    });
                    rep.actions.push("→ session/new".to_string());
                    if !send(&mut stdin, v) {
                        rep.error = Some("写入 session/new 失败".to_string());
                        break;
                    }
                    pending = Some(id);
                    phase = "session/new";
                }
                "session/new" => {
                    let Some(sid) = session_id.clone() else {
                        rep.error = Some("session/new 没给 sessionId".to_string());
                        break;
                    };
                    let id = next_id;
                    next_id += 1;
                    let v = serde_json::json!({
                        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
                        "params": { "sessionId": sid, "prompt": [ { "type": "text", "text": prompt } ] }
                    });
                    rep.actions.push("→ session/prompt".to_string());
                    if !send(&mut stdin, v) {
                        rep.error = Some("写入 session/prompt 失败".to_string());
                        break;
                    }
                    pending = Some(id);
                    phase = "prompt";
                }
                _ => {}
            }
        }

        let msg = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if rep.error.is_none() {
                    rep.error = Some("agent 关闭了输出（没有给出结果）".to_string());
                }
                break;
            }
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg) else {
            // 非 JSON 行：不致命（有些 agent 会往 stdout 打日志），留痕即可
            rep.actions.push(format!("（非 JSON 行已忽略）{}", msg.chars().take(80).collect::<String>()));
            continue;
        };

        let has_method = v.get("method").and_then(|m| m.as_str());
        let has_id = v.get("id").and_then(|i| i.as_i64());

        // ── agent 发来的**请求**（有 id 又有 method）→ 必须逐个答复 ──
        if let (Some(method), Some(id)) = (has_method, has_id) {
            match method {
                "session/request_permission" => {
                    rep.permission_requests += 1;
                    let options = v
                        .pointer("/params/options")
                        .and_then(|o| o.as_array())
                        .cloned()
                        .unwrap_or_default();
                    // 默认拒绝：优先挑"拒绝"选项；没有可拒绝的选项就回 cancelled
                    let deny_kind = match policy {
                        PermissionPolicy::Reject => {
                            let pick = options.iter().find(|o| {
                                matches!(
                                    o.get("kind").and_then(|k| k.as_str()),
                                    Some("reject_once") | Some("reject_always")
                                )
                            });
                            match pick.and_then(|o| o.get("optionId").and_then(|x| x.as_str())) {
                                Some(option_id) => {
                                    rep.permission_denials += 1;
                                    serde_json::json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
                                }
                                None => {
                                    rep.permission_denials += 1;
                                    serde_json::json!({ "outcome": { "outcome": "cancelled" } })
                                }
                            }
                        }
                        PermissionPolicy::AllowOnce => {
                            let pick = options.iter().find(|o| {
                                matches!(o.get("kind").and_then(|k| k.as_str()), Some("allow_once"))
                            });
                            match pick.and_then(|o| o.get("optionId").and_then(|x| x.as_str())) {
                                Some(option_id) => serde_json::json!({
                                    "outcome": { "outcome": "selected", "optionId": option_id }
                                }),
                                None => serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                            }
                        }
                    };
                    rep.actions.push(format!("← session/request_permission（我们回：{deny_kind}）"));
                    let _ = send(&mut stdin, serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": deny_kind }));
                }
                other => {
                    // 规范要求：请求必须被答复（不能不理）。我们不支持就回标准错误。
                    rep.unsupported_requests += 1;
                    rep.actions.push(format!("← {other}（不支持 → 回 -32601）"));
                    let _ = send(
                        &mut stdin,
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32601, "message": format!("method not supported: {other}") }
                        }),
                    );
                }
            }
            continue;
        }

        // ── 通知：session/update ──
        if has_method == Some("session/update") {
            let u = v.pointer("/params/update").cloned().unwrap_or_default();
            match u.get("sessionUpdate").and_then(|s| s.as_str()) {
                Some("agent_message_chunk") => {
                    if let Some(t) = u.pointer("/content/text").and_then(|t| t.as_str()) {
                        rep.reply.push_str(t);
                    }
                }
                Some("agent_thought_chunk") => {
                    if let Some(t) = u.pointer("/content/text").and_then(|t| t.as_str()) {
                        rep.thought.push_str(t);
                    }
                }
                Some("tool_call") | Some("tool_call_update") => {
                    let id = u.get("toolCallId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if let Some(existing) = rep.tool_calls.iter_mut().find(|c| c.tool_call_id == id) {
                        // 更新是**补丁**：只覆盖出现的字段
                        if let Some(t) = u.get("title").and_then(|x| x.as_str()) {
                            existing.title = t.to_string();
                        }
                        if let Some(k) = u.get("kind").and_then(|x| x.as_str()) {
                            existing.kind = k.to_string();
                        }
                        if let Some(s) = u.get("status").and_then(|x| x.as_str()) {
                            existing.status = s.to_string();
                        }
                    } else {
                        rep.tool_calls.push(ToolCall {
                            tool_call_id: id,
                            title: u.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            kind: u.get("kind").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            status: u.get("status").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        });
                    }
                }
                Some("usage_update") => {
                    rep.used_tokens = u.get("used").and_then(|x| x.as_u64()).unwrap_or(rep.used_tokens);
                    rep.size_tokens = u.get("size").and_then(|x| x.as_u64()).unwrap_or(rep.size_tokens);
                    // 引擎自报的花费：照抄，不换算、不估
                    if let Some(a) = u.pointer("/cost/amount").and_then(|x| x.as_f64()) {
                        rep.cost_usd = a;
                    }
                }
                _ => {}
            }
            continue;
        }

        // ── 响应 ──
        if let Some(id) = has_id {
            if Some(id) != pending {
                continue; // 不是我们在等的那条（例如迟到的答复）
            }
            if let Some(err) = v.get("error") {
                rep.error = Some(format!("agent 报错：{}", line(err.clone())));
                break;
            }
            let result = v.get("result").cloned().unwrap_or(serde_json::Value::Null);
            match phase {
                "initialize" => {
                    rep.protocol_version = result.get("protocolVersion").and_then(|x| x.as_i64()).unwrap_or(0);
                    rep.agent_name = result
                        .pointer("/agentInfo/name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    rep.agent_load_session = result
                        .pointer("/agentCapabilities/loadSession")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    // 规范：版本不一致时客户端**应**关掉连接并告知用户
                    if rep.protocol_version != PROTOCOL_VERSION {
                        rep.error = Some(format!(
                            "协议版本不一致：我们 {PROTOCOL_VERSION}，agent {}",
                            rep.protocol_version
                        ));
                        break;
                    }
                    pending = None;
                }
                "session/new" => {
                    session_id = result.get("sessionId").and_then(|x| x.as_str()).map(str::to_string);
                    rep.session_id = session_id.clone().unwrap_or_default();
                    pending = None;
                }
                "prompt" => {
                    rep.stop_reason = result.get("stopReason").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    rep.ok = true;
                    break;
                }
                _ => break,
            }
        }
    }

    if rep.error.is_none() && rep.stop_reason.is_empty() {
        rep.error = Some(format!("超时（{}s）内没有拿到 stopReason", timeout.as_secs()));
    }

    // ── 收尾 ──
    // 先给它**优雅退出**的机会：关掉它的 stdin，等它自己退（最多 1 秒）。
    // 早先这里收到最后一轮结果就立刻杀进程 —— 于是 agent 还没来得及落盘/关连接
    // 就被砍了（一次真实的竞态：mock 的 observed.json 因此空着）。
    drop(stdin);
    let grace = Instant::now() + Duration::from_secs(1);
    let mut exited = false;
    while Instant::now() < grace {
        match child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    if !exited {
        let _ = child.kill();
        crate::pty::kill_process_tree(pid);
    }
    let _ = child.wait();
    let drained = crate::proc::drain_bounded(
        &[&reader, &err_reader],
        crate::proc::DEFAULT_DRAIN_GRACE,
    );
    if let Ok(b) = stderr_buf.lock() {
        rep.stderr_tail = b.chars().take(2000).collect();
    }
    if !drained {
        rep.actions.push("（仍有子进程未退出，输出可能不完整）".to_string());
    }
    rep.wall_ms = start.elapsed().as_millis() as u64;
    rep
}

/// 权限策略。**默认拒绝** —— 与第 24 刀的默认姿态审计同一条纪律。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    #[default]
    Reject,
    /// 只在明确要"放行一次"时使用（例如人在界面上点了允许）。
    ///
    /// 给这个枚举留一条"能放行"的路是刻意的：只有"拒绝"的策略看起来安全，
    /// 实际上会逼着人去改代码才能放行 —— 那更糟。
    /// ⚠️ 目前界面还没有这个按钮，所以生产路径上只会用到 `Reject`。
    #[allow(dead_code)]
    AllowOnce,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-acp-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 按 ACP v1 规范写的 mock agent（见 `src/testdata/acp-mock-agent.py`）。
    fn mock_args(dir: &Path, scenario: &str) -> Vec<String> {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/testdata/acp-mock-agent.py");
        vec![
            script.to_string_lossy().into_owned(),
            scenario.to_string(),
            dir.join("observed.json").to_string_lossy().into_owned(),
        ]
    }

    fn have_python() -> bool {
        Command::new("python3").arg("-c").arg("pass").output().is_ok()
    }

    fn run_mock(dir: &Path, scenario: &str, prompt: &str, policy: PermissionPolicy) -> AcpReport {
        run(
            "python3",
            &mock_args(dir, scenario),
            dir,
            &HashMap::new(),
            prompt,
            policy,
            Duration::from_secs(20),
        )
    }

    fn observed(dir: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(dir.join("observed.json")).unwrap_or_default();
        serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null)
    }

    /// ★ 协议一致性：完整一轮（initialize → session/new → session/prompt → stopReason）。
    #[test]
    fn a_full_turn_follows_the_v1_flow() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("full");
        let rep = run_mock(&d, "full", "你好", PermissionPolicy::Reject);

        assert!(rep.error.is_none(), "不该有错：{:?}", rep.error);
        assert!(rep.ok);
        assert_eq!(rep.protocol_version, PROTOCOL_VERSION, "版本要协商成 1");
        assert_eq!(rep.agent_name, "mock-agent");
        assert!(rep.agent_load_session);
        assert_eq!(rep.session_id, "sess_mock_1");
        assert_eq!(rep.stop_reason, "end_turn");
        // 流式片段要拼成回复；思考片段单独放，不混进回复
        assert_eq!(rep.reply, "你好，我是 mock agent。");
        assert_eq!(rep.thought, "正在思考…");
        assert!(!rep.reply.contains("正在思考"), "思考不该混进回复：{}", rep.reply);

        // 我们确实按规范发了这三条，且 initialize 不宣告 fs/terminal
        let obs = observed(&d);
        assert_eq!(obs["protocol_version_seen"], PROTOCOL_VERSION);
        assert_eq!(obs["client_capabilities"], serde_json::json!({}), "不该宣告任何能力");
        assert_eq!(obs["got_session_new_cwd"], true);
        assert_eq!(obs["prompt_text"], "你好");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 权限：默认**拒绝**，且要挑"拒绝"选项而不是 `cancelled`。
    #[test]
    fn permission_is_denied_by_default_using_a_reject_option() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("deny");
        let rep = run_mock(&d, "permission", "跑个命令", PermissionPolicy::Reject);
        assert!(rep.ok, "{:?}", rep.error);
        assert_eq!(rep.permission_requests, 1, "动作留痕：{:?}", rep.actions);
        assert_eq!(rep.permission_denials, 1, "默认必须拒绝；动作留痕：{:?}", rep.actions);
        let obs = observed(&d);
        assert_eq!(obs["permission_outcome"], "selected", "客户端动作：{:?}", rep.actions);
        assert_eq!(obs["permission_option"], "reject-once", "要挑拒绝选项");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 显式放行一次：才会挑 `allow_once`。
    #[test]
    fn allow_once_picks_the_allow_option_only_when_asked() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("allow");
        let rep = run_mock(&d, "permission", "跑个命令", PermissionPolicy::AllowOnce);
        assert!(rep.ok, "{:?}", rep.error);
        assert_eq!(rep.permission_denials, 0, "明确放行时不该算拒绝");
        let obs = observed(&d);
        assert_eq!(obs["permission_option"], "allow-once");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 请求必须**逐个答复**：不支持的也要回标准错误，不能不理。
    #[test]
    fn an_unsupported_agent_request_gets_a_jsonrpc_error_not_silence() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("unsupported");
        let rep = run_mock(&d, "unsupported", "读个文件", PermissionPolicy::Reject);
        assert!(rep.ok, "{:?}", rep.error);
        assert_eq!(rep.unsupported_requests, 1);
        let obs = observed(&d);
        assert_eq!(obs["unsupported_got_response"], true, "必须答复，不能沉默");
        assert_eq!(obs["unsupported_error_code"], -32601);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 工具调用：`tool_call` 是**补丁**语义，后续 update 只能改出现的字段。
    #[test]
    fn tool_call_updates_patch_only_the_fields_they_carry() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("tool");
        let rep = run_mock(&d, "full", "你好", PermissionPolicy::Reject);
        assert_eq!(rep.tool_calls.len(), 1, "{:?}", rep.tool_calls);
        let c = &rep.tool_calls[0];
        assert_eq!(c.tool_call_id, "call_1");
        assert_eq!(c.title, "读取配置", "初次报告带的 title 要保留");
        assert_eq!(c.kind, "read");
        assert_eq!(c.status, "completed", "后续 update 只改 status");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 用量与花费：**照抄引擎自报**，不换算、不估。
    #[test]
    fn usage_and_cost_are_taken_verbatim_from_the_agent() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("usage");
        let rep = run_mock(&d, "full", "你好", PermissionPolicy::Reject);
        assert_eq!(rep.used_tokens, 53000);
        assert_eq!(rep.size_tokens, 200000);
        assert!((rep.cost_usd - 0.045).abs() < 1e-12, "{}", rep.cost_usd);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 版本不一致：规范说客户端**应**断开并告知用户 —— 我们要报出来。
    #[test]
    fn a_protocol_version_mismatch_is_reported_not_ignored() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("version");
        let rep = run_mock(&d, "badversion", "你好", PermissionPolicy::Reject);
        assert!(!rep.ok);
        let e = rep.error.unwrap_or_default();
        assert!(e.contains("协议版本不一致"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// agent 根本没起来 / 起来就退出：要如实报错，不能假装成功。
    #[test]
    fn a_dead_agent_is_reported_as_an_error() {
        let d = tmp("dead");
        let rep = run(
            "definitely-not-a-real-binary-xyz",
            &[],
            &d,
            &HashMap::new(),
            "hi",
            PermissionPolicy::Reject,
            Duration::from_secs(5),
        );
        assert!(!rep.ok);
        assert!(rep.error.unwrap_or_default().contains("启动 ACP agent 失败"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 收尾必须有界：agent 赖着不退时，墙钟不能被拖爆。
    #[test]
    fn a_hanging_agent_does_not_blow_past_the_timeout() {
        if !have_python() {
            eprintln!("跳过：没有 python3");
            return;
        }
        let d = tmp("hang");
        let start = Instant::now();
        let rep = run(
            "python3",
            &mock_args(&d, "hang"),
            &d,
            &HashMap::new(),
            "hi",
            PermissionPolicy::Reject,
            Duration::from_millis(800),
        );
        let elapsed = start.elapsed();
        assert!(!rep.ok);
        assert!(elapsed < Duration::from_secs(6), "墙钟必须有界（实测 {elapsed:?}）");
        let _ = std::fs::remove_dir_all(&d);
    }
}
