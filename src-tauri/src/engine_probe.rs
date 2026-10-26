//! 引擎能力协商（目标 ①）：**问引擎自己**，而不是靠我们在定义里手写。
//!
//! 实测依据（2026-10-14，本机官方 Claude Code 2.1.63 bundle）：
//!
//! ```text
//! claude -p <prompt> --output-format stream-json --verbose
//! ```
//!
//! 会在**推理之前**先吐一行 `{"type":"system","subtype":"init",…}`，里面有：
//! `model` / `permissionMode` / `apiKeySource` / `claude_code_version` /
//! `tools[]` / `agents[]` / `skills[]` / `mcp_servers[]` / `plugins[]`。
//!
//! 三件事因此成立：
//! 1. **探测很便宜**：init 在推理前就发出 —— 我们读到它**立刻杀进程**，不产生任何
//!    推理开销（不用花 token 就能知道这个引擎的能力）；
//! 2. **它能验证我们的注入**：`apiKeySource` 就是引擎实际用的凭据来源，
//!    `permissionMode` 就是它实际采纳的权限档 —— 这比"我们以为传对了"强得多；
//! 3. **它把"能力协商"从手写变成实测**：引擎到底加载了哪些 agents/skills/tools，
//!    只有它自己说了算。
//!
//! 不支持这个协议的引擎**如实报告不支持**，不假装成功。

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 引擎自己报告的初始化信息。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineInit {
    /// 引擎实际使用的模型
    pub model: String,
    /// 引擎实际采纳的权限档 —— **这是对我们注入的 `--permission-mode` 的实测回执**
    pub permission_mode: String,
    /// 引擎实际用的凭据来源（例如 `ANTHROPIC_API_KEY`）—— 验证我们的 env 注入
    pub api_key_source: String,
    pub version: String,
    pub output_style: String,
    pub session_id: String,
    pub cwd: String,
    /// 引擎这次加载到的工具
    pub tools: Vec<String>,
    /// 引擎这次能用的 agents（我们的 `@name` 到底有没有被看见，看这里）
    pub agents: Vec<String>,
    /// 引擎这次加载到的 skills
    pub skills: Vec<String>,
    pub slash_commands: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub plugins: Vec<String>,
}

/// 一次探测的结果。**成功与失败都要能说清楚**。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProbeReport {
    pub engine_id: String,
    pub cmd: String,
    pub args: Vec<String>,
    /// 拿到了 init 事件 = 这个引擎支持结构化探测
    pub supported: bool,
    pub init: Option<EngineInit>,
    /// 失败/不支持的原因（给人和给日志看的是同一份）
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// 解析一行 stdout，如果是 `system/init` 就提取出来。
///
/// 纯函数：解析口径只此一处（探测、以及将来读引擎事件流都用它）。
pub fn parse_init_line(line: &str) -> Option<EngineInit> {
    let line = line.trim();
    if line.is_empty() || !line.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("system") {
        return None;
    }
    if v.get("subtype").and_then(|t| t.as_str()) != Some("init") {
        return None;
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let arr = |k: &str| -> Vec<String> {
        match v.get(k) {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|x| x.as_str())
                .map(|x| x.to_string())
                .collect(),
            _ => Vec::new(),
        }
    };
    Some(EngineInit {
        model: s("model"),
        permission_mode: s("permissionMode"),
        api_key_source: s("apiKeySource"),
        version: s("claude_code_version"),
        output_style: s("output_style"),
        session_id: s("session_id"),
        cwd: s("cwd"),
        tools: arr("tools"),
        agents: arr("agents"),
        skills: arr("skills"),
        slash_commands: arr("slash_commands"),
        mcp_servers: arr("mcp_servers"),
        plugins: arr("plugins"),
    })
}

/// 探测一个引擎：跑一次 `stream-json` 握手，拿到 init 事件后**立刻杀掉**。
///
/// `args_prefix` 是引擎自己的参数（`profile.default_args` + 员工姿态/白名单等
/// 已应用的参数），本函数只负责追加协议所需的 `-p … --output-format stream-json
/// --verbose`。探测用的是一个**极短且不需要模型回答**的提示 —— 因为 init 在推理
/// 之前就发出，我们在它之后立刻杀进程，所以这次探测**不产生推理开销**。
pub fn probe(
    engine_id: &str,
    cmd: &str,
    args_prefix: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    timeout: Duration,
) -> ProbeReport {
    let started = Instant::now();
    let mut args: Vec<String> = args_prefix.to_vec();
    args.extend(
        [
            "-p",
            "ok",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
        .iter()
        .map(|s| s.to_string()),
    );

    let fail = |msg: String, args: Vec<String>, started: Instant| ProbeReport {
        engine_id: engine_id.to_string(),
        cmd: cmd.to_string(),
        args,
        supported: false,
        init: None,
        error: Some(msg),
        duration_ms: started.elapsed().as_millis() as u64,
    };

    let mut child = match Command::new(cmd)
        .args(&args)
        .current_dir(cwd)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return fail(format!("无法启动 {cmd}：{e}"), args, started),
    };
    let pid = child.id();

    // 读到 init 就收工。用**独立线程 + 通道**读，避免管道阻塞把探测卡死
    // （同一个坑在 span 里踩过：`wait_with_output` 会被孙进程握着的管道卡住）。
    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        if let Some(out) = stdout {
            let mut r = BufReader::new(out);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.clone()).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let mut init: Option<EngineInit> = None;
    let mut saw_any = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    saw_any = true;
                }
                if let Some(i) = parse_init_line(&line) {
                    init = Some(i);
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 立刻清扫整棵进程树：init 之后没必要让它继续（继续就是真的在花 token 推理了）
    let _ = child.kill();
    crate::pty::kill_process_tree(pid);
    let _ = child.wait();
    drop(rx);
    let _ = reader.join();

    let duration_ms = started.elapsed().as_millis() as u64;
    match init {
        Some(i) => ProbeReport {
            engine_id: engine_id.to_string(),
            cmd: cmd.to_string(),
            args,
            supported: true,
            init: Some(i),
            error: None,
            duration_ms,
        },
        None => ProbeReport {
            engine_id: engine_id.to_string(),
            cmd: cmd.to_string(),
            args,
            supported: false,
            init: None,
            error: Some(if saw_any {
                format!(
                    "{cmd} 有输出，但在 {timeout:?} 内没有 system/init 事件 —— 它可能不支持 \
                     `--output-format stream-json`，或版本不同（如实报告，不假装支持）"
                )
            } else {
                format!("{cmd} 在 {timeout:?} 内没有任何输出（可能不支持该协议，或启动就失败了）")
            }),
            duration_ms,
        },
    }
}

/// 探测结果的**人看到的样子**（纯函数，便于单测）。
pub fn probe_text(r: &ProbeReport) -> String {
    let mut s = String::new();
    s.push_str(&format!("引擎 {} 的能力协商\n\n", r.engine_id));
    s.push_str(&format!("命令：{} {}\n", r.cmd, r.args.join(" ")));
    s.push_str(&format!("耗时：{}ms\n\n", r.duration_ms));
    match (&r.init, &r.error) {
        (Some(i), _) => {
            s.push_str("✅ 引擎自己报告：\n");
            s.push_str(&format!("  模型：{}\n", i.model));
            s.push_str(&format!("  权限档：{}（这是对注入参数的**实测回执**）\n", i.permission_mode));
            s.push_str(&format!("  凭据来源：{}（验证 env 注入是否生效）\n", i.api_key_source));
            s.push_str(&format!("  版本：{}｜输出风格：{}\n", i.version, i.output_style));
            s.push_str(&format!("  工作目录：{}\n", i.cwd));
            s.push_str(&format!(
                "  能力：工具 {} ｜ agents {} ｜ skills {} ｜ slash {} ｜ MCP {} ｜ 插件 {}\n",
                i.tools.len(),
                i.agents.len(),
                i.skills.len(),
                i.slash_commands.len(),
                i.mcp_servers.len(),
                i.plugins.len()
            ));
            if !i.agents.is_empty() {
                let shown: Vec<&str> = i.agents.iter().take(8).map(|x| x.as_str()).collect();
                s.push_str(&format!("  agents 前 8 个：{}\n", shown.join(", ")));
            }
            if !i.tools.is_empty() {
                s.push_str(&format!("  工具：{}\n", i.tools.join(", ")));
            }
        }
        (None, Some(e)) => {
            s.push_str(&format!("⚠ 探测未成功：{e}\n"));
            s.push_str(
                "（这不是「引擎坏了」—— 只说明它不按这个协议自报能力；\n  \
                 我们不会因此假装知道它的能力。）\n",
            );
        }
        (None, None) => s.push_str("⚠ 什么都没有拿到（不该发生）\n"),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **真实夹具**：这一行是从本机 Claude Code 2.1.63 实测抓下来的 init 事件
    /// （只把三个长数组截短了，其余字段一字未改）。
    /// 用真实数据当夹具，能挡住"我按记忆写了个形状"这类错。
    const REAL_INIT: &str = include_str!("testdata/claude-code-init.jsonl");

    #[test]
    fn parses_the_real_captured_init_event() {
        let i = parse_init_line(REAL_INIT).expect("真实 init 事件必须能解析");
        assert_eq!(i.model, "claude-sonnet-4-6");
        assert_eq!(i.permission_mode, "plan");
        assert_eq!(i.api_key_source, "ANTHROPIC_API_KEY");
        assert_eq!(i.version, "2.1.63");
        assert_eq!(i.cwd, "/Users/zhanglei/yycc/eng");
        assert!(!i.session_id.is_empty());
        assert_eq!(i.tools, vec!["Agent", "TaskOutput"]);
        assert!(i.skills.len() >= 1);
        assert!(!i.slash_commands.is_empty());
    }

    #[test]
    fn non_init_lines_are_rejected_not_guessed() {
        assert!(parse_init_line("").is_none());
        assert!(parse_init_line("not json").is_none());
        assert!(parse_init_line("[]").is_none());
        assert!(parse_init_line(r#"{"type":"assistant","message":{}}"#).is_none());
        assert!(parse_init_line(r#"{"type":"system","subtype":"hook_started"}"#).is_none());
        assert!(parse_init_line(r#"{"type":"result","subtype":"success"}"#).is_none());
    }

    #[test]
    fn a_minimal_init_line_still_parses_with_empty_defaults() {
        let i = parse_init_line(r#"{"type":"system","subtype":"init","model":"m"}"#).unwrap();
        assert_eq!(i.model, "m");
        assert_eq!(i.permission_mode, "", "缺字段留空，不编值");
        assert!(i.tools.is_empty());
    }

    #[test]
    fn probe_reports_unsupported_instead_of_pretending() {
        // 一个**不吐 init** 的命令（真的存在、且立刻退出）
        let r = probe(
            "true-ish",
            "/bin/echo",
            &[],
            Path::new("/tmp"),
            &HashMap::new(),
            Duration::from_secs(2),
        );
        assert!(!r.supported);
        assert!(r.init.is_none());
        let e = r.error.clone().unwrap_or_default();
        assert!(e.contains("没有") || e.contains("不支持"), "{e}");
        let text = probe_text(&r);
        assert!(text.contains("探测未成功"));
    }

    #[test]
    fn probe_reports_a_missing_binary_clearly() {
        let r = probe(
            "ghost",
            "/definitely/not/a/binary",
            &[],
            Path::new("/tmp"),
            &HashMap::new(),
            Duration::from_secs(1),
        );
        assert!(!r.supported);
        assert!(r.error.clone().unwrap_or_default().contains("无法启动"));
    }

    /// 用一个**桩引擎**验证 spawn → 读 init → 立刻杀 这条链：
    /// 桩会先打印 init，然后一直睡 —— 探测必须**不等它**就返回。
    #[test]
    fn probe_reads_init_then_kills_without_waiting_for_the_engine() {
        let Some(node) = which("node") else { return };
        let dir = std::env::temp_dir().join(format!("dsh-probe-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let stub = dir.join("stub.js");
        std::fs::write(
            &stub,
            r#"console.log(JSON.stringify({type:"system",subtype:"init",model:"stub-model",permissionMode:"plan",apiKeySource:"STUB_KEY",claude_code_version:"0.0.1",tools:["Read"],agents:["probe"],skills:[],slash_commands:[],mcp_servers:[],plugins:[],session_id:"s1",cwd:"/tmp"}));
setTimeout(()=>{}, 60000);"#,
        )
        .unwrap();

        let started = Instant::now();
        let r = probe(
            "stub",
            &node,
            &[stub.to_string_lossy().into_owned()],
            &dir,
            &HashMap::new(),
            Duration::from_secs(15),
        );
        let elapsed = started.elapsed();

        assert!(r.supported, "桩引擎应被识别为支持：{:?}", r.error);
        let i = r.init.unwrap();
        assert_eq!(i.model, "stub-model");
        assert_eq!(i.permission_mode, "plan");
        assert_eq!(i.api_key_source, "STUB_KEY");
        assert_eq!(i.tools, vec!["Read"]);
        assert!(elapsed < Duration::from_secs(10), "不该等桩睡满 60 秒：{elapsed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn which(cmd: &str) -> Option<String> {
        let out = Command::new("which").arg(cmd).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    }

    /// **真实引擎探测**（本机官方 Claude Code bundle）。
    ///
    /// 用**不可达的 base_url + 假 key**：init 在推理之前发出，我们立刻杀进程，
    /// 所以这条测试**不花 token、不碰用户配置**（`CLAUDE_CONFIG_DIR` 指向临时目录）。
    /// 引擎不在本机时跳过。
    #[test]
    fn real_engine_probe_reports_its_own_capabilities() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
        // 找入口：**优先官方 bundle 的 cli.js**（用 node 跑），PATH 上的 `claude`
        // 只作兜底。原因：本机 PATH 上的 claude 是个 Go launcher，实测会卡在版本
        // 拉取上（第一轮就记录过），用它会让"探测"变成"等 30 秒"。
        let (cmd, prefix): (String, Vec<String>) = {
            let mut from_bundle: Option<(String, Vec<String>)> = None;
            let bdir = home.join(".baidu-cc");
            if let Ok(entries) = std::fs::read_dir(&bdir) {
                for e in entries.flatten() {
                    let cli = e.path().join("claude-code").join("cli.js");
                    if cli.exists() {
                        if let Some(node) = which("node") {
                            from_bundle = Some((node, vec![cli.to_string_lossy().into_owned()]));
                        }
                        break;
                    }
                }
            }
            match from_bundle {
                Some(x) => x,
                None => match which("claude") {
                    Some(c) => (c, Vec::new()),
                    None => return,
                },
            }
        };

        let cfghome = std::env::temp_dir().join(format!("dsh-probe-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cfghome);
        let env: HashMap<String, String> = HashMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), cfghome.to_string_lossy().into_owned()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-bogus-probe".to_string()),
            ("ANTHROPIC_BASE_URL".to_string(), "http://127.0.0.1:9".to_string()),
            ("DISABLE_TELEMETRY".to_string(), "1".to_string()),
            ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
        ]);

        let r = probe(
            "claude-code",
            &cmd,
            &prefix,
            Path::new("/tmp"),
            &env,
            Duration::from_secs(20),
        );
        eprintln!("===== 真实引擎探测 =====\n{}\n===== end =====", probe_text(&r));
        if !r.supported {
            // 本机这个引擎版本不支持该协议时如实跳过（不假装通过）
            eprintln!("[probe] 跳过：{:?}", r.error);
            let _ = std::fs::remove_dir_all(&cfghome);
            return;
        }
        let i = r.init.unwrap();
        assert!(!i.model.is_empty(), "真实引擎必须报出模型");
        assert!(!i.version.is_empty());
        assert!(!i.tools.is_empty(), "真实引擎必须报出工具列表");
        // 用假 key + 不可达地址 → 凭据来源应当就是我们注入的那个
        assert_eq!(i.api_key_source, "ANTHROPIC_API_KEY");
        let _ = std::fs::remove_dir_all(&cfghome);
    }
}
