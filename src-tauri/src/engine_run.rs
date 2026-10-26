//! 结构化跑一次（目标 ① 的非 PTY 引擎路径 + ⑤ 账本的金额）。
//!
//! 第 8 刀做账本时的结论是"**金额算不出来**：本机 transcript 里没有成本字段
//! （`total_cost_usd`/`costUSD` 出现 0 次），算金额需要一张价格表"。
//!
//! 那个结论对 transcript 是对的，但**漏了一条更好的路**：结构化协议的 `result`
//! 事件**自带成本与用量**。本机实测（2026-10-14，一次极短提示）：
//!
//! ```text
//! total_cost_usd = 0.174335
//! usage          = { input_tokens: 34857, output_tokens: 2, … }
//! modelUsage     = { "deepseek-v4-flash[1m]": { costUSD: 0.174335,
//!                     contextWindow: 1000000, maxOutputTokens: 32000 } }
//! ```
//!
//! 于是三件事一起成立：
//! 1. **金额不用价格表** —— 引擎自己算（它还知道缓存价、分档价）；
//! 2. **按模型拆分** —— `modelUsage` 直接给了每个模型的用量与花费；
//! 3. **上下文窗口引擎自己报** —— 比维护"型号后缀 + 内置表 + 手动覆盖"准得多。
//!
//! 注意这不是"再跑一次探针"：这是一条**真的干活的路径**（`-p <prompt>`），
//! 只是把输出从 TUI 换成结构化事件，因此它可以被记录、被计价、被归因到员工。

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::engine_probe::{parse_init_line, EngineInit};

/// 一次运行消耗的 token。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// 单个模型的用量与花费（引擎自报）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// ★ 引擎自报的上下文窗口（比我们猜型号后缀准）
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

/// `result` 事件 —— 一次运行的最终结算。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunResult {
    pub subtype: String,
    pub is_error: bool,
    pub num_turns: u64,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub total_cost_usd: f64,
    /// 最终回复文本
    pub reply: String,
    pub session_id: String,
    pub usage: RunUsage,
    pub model_usage: Vec<ModelUsage>,
    pub permission_denials: usize,
}

/// 事件流解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    Init(EngineInit),
    /// 一条助手消息（我们只数次数，正文由 `result.reply` 给）
    Assistant,
    Result(RunResult),
    /// 其它事件（control_request 等）—— 不认识就如实归到"其它"，不猜
    Other(String),
}

/// 解析一行事件。纯函数：解析口径只此一处。
pub fn parse_event_line(line: &str) -> Option<RunEvent> {
    let line = line.trim();
    if line.is_empty() || !line.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "system" && v.get("subtype").and_then(|t| t.as_str()) == Some("init") {
        return parse_init_line(line).map(RunEvent::Init);
    }
    match ty {
        "assistant" => Some(RunEvent::Assistant),
        "result" => Some(RunEvent::Result(parse_result(&v))),
        "" => None,
        other => Some(RunEvent::Other(other.to_string())),
    }
}

fn parse_result(v: &serde_json::Value) -> RunResult {
    let g_u64 = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let u = v.get("usage").cloned().unwrap_or(serde_json::Value::Null);
    let gu = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let mut model_usage: Vec<ModelUsage> = Vec::new();
    if let Some(serde_json::Value::Object(map)) = v.get("modelUsage") {
        for (model, vals) in map {
            let gm = |k: &str| vals.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            model_usage.push(ModelUsage {
                model: model.clone(),
                input_tokens: gm("inputTokens"),
                output_tokens: gm("outputTokens"),
                cost_usd: vals.get("costUSD").and_then(|x| x.as_f64()).unwrap_or(0.0),
                context_window: vals.get("contextWindow").and_then(|x| x.as_u64()),
                max_output_tokens: vals.get("maxOutputTokens").and_then(|x| x.as_u64()),
            });
        }
    }
    RunResult {
        subtype: v.get("subtype").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        is_error: v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
        num_turns: g_u64("num_turns"),
        duration_ms: g_u64("duration_ms"),
        duration_api_ms: g_u64("duration_api_ms"),
        total_cost_usd: v.get("total_cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0),
        reply: v.get("result").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        session_id: v.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        usage: RunUsage {
            input_tokens: gu("input_tokens"),
            output_tokens: gu("output_tokens"),
            cache_read_input_tokens: gu("cache_read_input_tokens"),
            cache_creation_input_tokens: gu("cache_creation_input_tokens"),
        },
        model_usage,
        permission_denials: v
            .get("permission_denials")
            .and_then(|x| x.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
    }
}

/// 这一次运行**是怎么结束的**。
///
/// 为什么要单独记：只靠"有没有 result 事件"分不清下面三种完全不同的情况 ——
/// 而它们在**该不该换供应商**上的答案正好相反：
/// - `TimedOut`：可能是"模型想用工具挂住了"（第 20 刀实测过）→ 换供应商没用；
/// - `ExitedWithoutResult`：进程自己退了、一轮都没成 → **连接/凭据问题的特征**；
/// - `SpawnFailed`：二进制都不在 → 换个供应商同样救不了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ExitKind {
    /// 拿到了 `result` 事件（正常结束，成功或引擎自报失败）
    Completed,
    /// 进程自己退出，但没有任何 result 事件
    ExitedWithoutResult,
    /// 到了超时，我们动手杀的
    TimedOut,
    /// 连进程都没起来
    SpawnFailed,
}

/// 一次结构化运行的完整报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RunReport {
    pub ok: bool,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub init: Option<EngineInit>,
    pub result: Option<RunResult>,
    pub assistant_turns: usize,
    /// 本机实测墙钟
    pub wall_ms: u64,
    pub error: Option<String>,
    pub stderr_tail: String,
    /// 结束方式（见 [`ExitKind`]）
    pub exit_kind: ExitKind,
}

/// 结构化跑一次：`<engine> <args> -p <prompt> --output-format stream-json --verbose`。
///
/// 与 `engine_probe` 的区别：探针拿到 init 就杀（**不花 token**）；这里是**真的干活**，
/// 会等到 `result` 事件、产生推理开销 —— 所以它必须被记录（见 [`append_run`]）。
pub fn run(
    cmd: &str,
    args_prefix: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    prompt: &str,
    timeout: Duration,
) -> RunReport {
    let mut args: Vec<String> = args_prefix.to_vec();
    args.extend(
        ["-p", prompt, "--output-format", "stream-json", "--verbose"]
            .iter()
            .map(|s| s.to_string()),
    );

    let started = Instant::now();
    let fail = |msg: String, stderr_tail: String| RunReport {
        ok: false,
        cmd: cmd.to_string(),
        args: args.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        init: None,
        result: None,
        assistant_turns: 0,
        wall_ms: started.elapsed().as_millis() as u64,
        error: Some(msg),
        stderr_tail,
        exit_kind: ExitKind::SpawnFailed,
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
        Err(e) => return fail(format!("无法启动 {cmd}：{e}"), String::new()),
    };
    let pid = child.id();

    // 读取线程 + 通道：绝不用 wait_with_output（孙进程握着管道会把"超时"变成"等到天荒地老"）
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
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
    let (txe, rxe) = std::sync::mpsc::channel::<String>();
    let err_reader = std::thread::spawn(move || {
        if let Some(e) = stderr {
            let mut r = BufReader::new(e);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if txe.send(line.clone()).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    let mut init: Option<EngineInit> = None;
    let mut result: Option<RunResult> = None;
    let mut assistant_turns = 0usize;
    let mut stderr_tail = String::new();
    let mut timed_out = false;

    let deadline = Instant::now() + timeout;
    // 结束条件：拿到 result（正常收工）或超时。stdout 关闭也要退出，否则会空等到超时。
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            timed_out = true;
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(400))) {
            Ok(line) => match parse_event_line(&line) {
                Some(RunEvent::Init(i)) => init = Some(i),
                Some(RunEvent::Assistant) => assistant_turns += 1,
                Some(RunEvent::Result(r)) => {
                    result = Some(r);
                    break;
                }
                Some(RunEvent::Other(_)) | None => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // 顺手收一下 stderr（不阻塞）
                while let Ok(l) = rxe.try_recv() {
                    stderr_tail.push_str(&l);
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 清扫整棵进程树（拿到 result 后引擎通常自己退了；超时时必须我们动手）
    let _ = child.kill();
    crate::pty::kill_process_tree(pid);
    let _ = child.wait();
    // ★ 收尾**必须有界**：若还有孙代握着 stdout/stderr（例如 `ps` 不可用导致树清扫
    // 只能降级为杀 root），`join()` 会一直等到它自己退出 —— 那"超时"就又是假的。
    // 第 5 刀在 `run_shell` 上踩过同一个坑（600ms 的超时等了 31.7s）。
    // 这里改成轮询 `is_finished`，超时就不等了，线程交给运行时自行回收。
    // 有界排空：**唯一实现**在 `crate::proc`（这里曾因 join() 假超时 30 秒）
    let drained = crate::proc::drain_bounded(
        &[&reader, &err_reader],
        crate::proc::DEFAULT_DRAIN_GRACE,
    );
    // 先把 stderr 收干净再丢接收端（顺序反了就是"用了已移动的值"）
    while let Ok(l) = rxe.try_recv() {
        stderr_tail.push_str(&l);
    }
    drop(rx);
    drop(rxe);
    stderr_tail.push_str(crate::proc::drain_note(drained));
    if stderr_tail.len() > 2000 {
        let cut = stderr_tail.len() - 2000;
        let mut s = cut;
        while s < stderr_tail.len() && !stderr_tail.is_char_boundary(s) {
            s += 1;
        }
        stderr_tail = format!("…（前面 {cut} 字节省略）\n{}", &stderr_tail[s..]);
    }

    let wall_ms = started.elapsed().as_millis() as u64;
    match result {
        Some(r) => {
            let ok = !r.is_error;
            RunReport {
                ok,
                cmd: cmd.to_string(),
                args,
                cwd: cwd.to_string_lossy().into_owned(),
                init,
                result: Some(r),
                assistant_turns,
                wall_ms,
                error: if ok {
                    None
                } else {
                    Some(format!("引擎返回错误：{}", stderr_tail.trim()))
                },
                stderr_tail,
                exit_kind: ExitKind::Completed,
            }
        }
        None => {
            let exit_kind = if timed_out { ExitKind::TimedOut } else { ExitKind::ExitedWithoutResult };
            let msg = if timed_out {
                format!("超时（{timeout:?}）内没有 result 事件")
            } else {
                "引擎结束了但没有 result 事件（可能不支持 stream-json）".to_string()
            };
            RunReport {
                ok: false,
                cmd: cmd.to_string(),
                args,
                cwd: cwd.to_string_lossy().into_owned(),
                init,
                result: None,
                assistant_turns,
                wall_ms,
                error: Some(msg),
                stderr_tail,
                exit_kind,
            }
        }
    }
}

// ── 运行记录（账本的金额来源）────────────────────────────────────────

/// 一次运行的**账目**（落盘一行，供账本聚合）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunEntry {
    pub at: String,
    /// 归因到哪个员工（没有就空）
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub provider_id: String,
    pub model: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    pub num_turns: u64,
    pub ok: bool,
    #[serde(default)]
    pub session_id: String,
    /// 干什么花的（预留给任务/提示摘要）
    #[serde(default)]
    pub label: String,
    /// 引擎自报的上下文窗口（来自 `modelUsage[].contextWindow`）。
    /// 存进运行记录，账本就能显示"引擎说这个模型窗口多大" —— 比猜型号后缀准。
    #[serde(default)]
    pub context_window: Option<u64>,
    /// 失败原因（成功时为空）。**失败也要记账** —— 否则"跑了三次都失败"
    /// 在账本上看起来像"什么都没发生"。
    #[serde(default)]
    pub error: String,
}

pub const RUNS_FILE: &str = ".onecode/runs.jsonl";

pub fn runs_path(cwd: &Path) -> PathBuf {
    cwd.join(RUNS_FILE)
}

/// 追加一条运行记录（JSONL：追加写，坏行不影响其它行）。
pub fn append_run(cwd: &Path, entry: &RunEntry) -> Result<(), String> {
    let path = runs_path(cwd);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入失败：{e}"))?;
    Ok(())
}

/// 读全部运行记录。**坏行跳过并计数**（不静默丢：返回值里有 `bad_lines`）。
pub fn read_runs(cwd: &Path) -> (Vec<RunEntry>, usize) {
    let Ok(text) = std::fs::read_to_string(runs_path(cwd)) else {
        return (Vec::new(), 0);
    };
    let mut out = Vec::new();
    let mut bad = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RunEntry>(line) {
            Ok(e) => out.push(e),
            Err(_) => bad += 1,
        }
    }
    (out, bad)
}

/// 运行记录的**人看到的样子**。
pub fn run_text(r: &RunReport) -> String {
    let mut s = String::new();
    s.push_str(&format!("引擎运行{}（{:.1}s 墙钟）\n\n", if r.ok { "成功" } else { "失败" }, r.wall_ms as f64 / 1000.0));
    s.push_str(&format!("命令：{} {}\n", r.cmd, r.args.join(" ")));
    if let Some(i) = &r.init {
        s.push_str(&format!("引擎：{} ｜ 版本 {}\n", i.model, i.version));
        s.push_str(&format!("权限档：{}（实测回执）\n", i.permission_mode));
    }
    s.push_str(&format!("助手消息：{} 条\n", r.assistant_turns));
    match &r.result {
        Some(res) => {
            s.push_str(&format!("\n回复：{}\n", res.reply));
            s.push_str(&format!("\n── 结算（引擎自报）──\n"));
            s.push_str(&format!("  金额：${:.6}\n", res.total_cost_usd));
            s.push_str(&format!(
                "  用量：输入 {} ｜ 输出 {} ｜ 缓存写 {} ｜ 缓存读 {}\n",
                res.usage.input_tokens,
                res.usage.output_tokens,
                res.usage.cache_creation_input_tokens,
                res.usage.cache_read_input_tokens
            ));
            s.push_str(&format!(
                "  轮次 {} ｜ API 耗时 {}ms ｜ 会话 {}\n",
                res.num_turns, res.duration_api_ms, res.session_id
            ));
            for m in &res.model_usage {
                s.push_str(&format!(
                    "  · {} → 输入 {} ｜ 输出 {} ｜ ${:.6}",
                    m.model, m.input_tokens, m.output_tokens, m.cost_usd
                ));
                if let Some(w) = m.context_window {
                    s.push_str(&format!(" ｜ 窗口 {w}"));
                }
                if let Some(mx) = m.max_output_tokens {
                    s.push_str(&format!(" ｜ 最大输出 {mx}"));
                }
                s.push('\n');
            }
            if res.permission_denials > 0 {
                s.push_str(&format!("  ⚠ 权限被拒 {} 次\n", res.permission_denials));
            }
        }
        None => {
            s.push_str(&format!(
                "\n⚠ 没有拿到结算：{}\n",
                r.error.clone().unwrap_or_default()
            ));
        }
    }
    if !r.stderr_tail.trim().is_empty() {
        s.push_str(&format!("\nstderr:\n{}\n", r.stderr_tail.trim()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **真实夹具**：这一串事件是本机 Claude Code 实测抓下来的
    /// （一次 `-p "…" --output-format stream-json --verbose` 的完整输出，
    /// 只把 init 的长数组与助手正文截短了）。
    const REAL_RUN: &str = include_str!("testdata/claude-code-run.jsonl");

    fn real_events() -> Vec<RunEvent> {
        REAL_RUN.lines().filter_map(parse_event_line).collect()
    }

    #[test]
    fn parses_the_real_event_stream() {
        let ev = real_events();
        assert_eq!(ev.len(), 4, "夹具应有 4 个事件：{ev:?}");
        assert!(matches!(ev[0], RunEvent::Init(_)));
        assert!(matches!(ev[1], RunEvent::Assistant));
        assert!(matches!(ev[3], RunEvent::Result(_)));
    }

    #[test]
    fn the_result_event_carries_real_money_and_usage() {
        let ev = real_events();
        let RunEvent::Result(r) = ev.into_iter().last().unwrap() else { panic!("最后应是 result") };
        assert_eq!(r.subtype, "success");
        assert!(!r.is_error);
        assert_eq!(r.num_turns, 1);
        assert_eq!(r.reply, "ok");
        // 真实数字：一个 2 token 的回复，输入 34857 token
        assert!((r.total_cost_usd - 0.174335).abs() < 1e-9, "{}", r.total_cost_usd);
        assert_eq!(r.usage.input_tokens, 34857);
        assert_eq!(r.usage.output_tokens, 2);
        assert!(!r.session_id.is_empty());
    }

    /// ★ 引擎自己报窗口与最大输出 —— 比我们猜型号后缀准。
    #[test]
    fn model_usage_reports_the_context_window_from_the_engine() {
        let ev = real_events();
        let RunEvent::Result(r) = ev.into_iter().last().unwrap() else { panic!() };
        assert_eq!(r.model_usage.len(), 1);
        let m = &r.model_usage[0];
        assert_eq!(m.model, "deepseek-v4-flash[1m]");
        assert_eq!(m.context_window, Some(1_000_000));
        assert_eq!(m.max_output_tokens, Some(32_000));
        assert!((m.cost_usd - 0.174335).abs() < 1e-9);
    }

    #[test]
    fn unknown_and_broken_lines_are_handled_without_guessing() {
        assert!(parse_event_line("").is_none());
        assert!(parse_event_line("not json").is_none());
        assert!(parse_event_line(r#"{"no":"type"}"#).is_none());
        assert_eq!(
            parse_event_line(r#"{"type":"control_request"}"#),
            Some(RunEvent::Other("control_request".to_string()))
        );
        // 残缺的 result 也要能解析（字段缺就取默认，不 panic）
        let RunEvent::Result(r) = parse_event_line(r#"{"type":"result","subtype":"error"}"#).unwrap()
        else {
            panic!()
        };
        assert_eq!(r.subtype, "error");
        assert_eq!(r.total_cost_usd, 0.0);
        assert!(r.model_usage.is_empty());
    }

    // ── 运行记录 ──────────────────────────────────────────────────────

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-run-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(at: &str, agent: &str, cost: f64) -> RunEntry {
        RunEntry {
            at: at.to_string(),
            agent: agent.to_string(),
            provider_id: "main".to_string(),
            model: "m".to_string(),
            cost_usd: cost,
            input_tokens: 100,
            output_tokens: 10,
            duration_ms: 1000,
            num_turns: 1,
            ok: true,
            session_id: "s".to_string(),
            label: String::new(),
            context_window: Some(1_000_000),
            error: String::new(),
        }
    }

    #[test]
    fn runs_are_appended_and_read_back() {
        let cwd = tmp("append");
        append_run(&cwd, &entry("t1", "dev", 0.01)).unwrap();
        append_run(&cwd, &entry("t2", "writer", 0.02)).unwrap();
        let (runs, bad) = read_runs(&cwd);
        assert_eq!(runs.len(), 2);
        assert_eq!(bad, 0);
        assert_eq!(runs[0].agent, "dev");
        assert!((runs[1].cost_usd - 0.02).abs() < 1e-12);
        // JSONL 追加：文件里就是两行
        let text = std::fs::read_to_string(runs_path(&cwd)).unwrap();
        assert_eq!(text.lines().count(), 2);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_broken_line_is_counted_not_silently_dropped() {
        let cwd = tmp("broken");
        append_run(&cwd, &entry("t1", "dev", 0.01)).unwrap();
        let mut text = std::fs::read_to_string(runs_path(&cwd)).unwrap();
        text.push_str("{ not json\n");
        text.push_str(&serde_json::to_string(&entry("t3", "dev", 0.03)).unwrap());
        text.push('\n');
        std::fs::write(runs_path(&cwd), text).unwrap();

        let (runs, bad) = read_runs(&cwd);
        assert_eq!(runs.len(), 2, "坏行不该影响好行");
        assert_eq!(bad, 1, "坏行要计数，不能静默");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn missing_runs_file_is_empty_not_an_error() {
        let cwd = tmp("none");
        let (runs, bad) = read_runs(&cwd);
        assert!(runs.is_empty());
        assert_eq!(bad, 0);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── 端到端（桩引擎）──────────────────────────────────────────────

    fn write_stub(dir: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("stub-run");
        std::fs::write(&p, script).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    #[test]
    fn a_stub_engine_that_emits_a_result_is_parsed_end_to_end() {
        let cwd = tmp("e2e");
        let stub = write_stub(
            &cwd,
            r#"#!/bin/sh
cat <<'EOF'
{"type":"system","subtype":"init","model":"stub-m","permissionMode":"plan","apiKeySource":"K","claude_code_version":"1","tools":[],"agents":[],"skills":[],"slash_commands":[],"mcp_servers":[],"plugins":[],"session_id":"s","cwd":"/x"}
{"type":"assistant","message":{"content":[]}}
{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":10,"duration_api_ms":9,"total_cost_usd":0.5,"result":"done","session_id":"s","usage":{"input_tokens":7,"output_tokens":3},"modelUsage":{"stub-m":{"inputTokens":7,"outputTokens":3,"costUSD":0.5,"contextWindow":200000}}}
EOF
"#,
        );
        let r = run(
            &stub.to_string_lossy(),
            &[],
            &cwd,
            &HashMap::new(),
            "hello",
            Duration::from_secs(10),
        );
        assert!(r.ok, "{:?}", r.error);
        let res = r.result.clone().expect("要有 result");
        assert_eq!(res.reply, "done");
        assert!((res.total_cost_usd - 0.5).abs() < 1e-12);
        assert_eq!(res.num_turns, 3);
        assert_eq!(r.assistant_turns, 1);
        assert_eq!(r.init.as_ref().unwrap().permission_mode, "plan");
        assert_eq!(res.model_usage[0].context_window, Some(200_000));
        // 命令里必须带上协议参数
        let join = r.args.join(" ");
        assert!(join.contains("-p hello"));
        assert!(join.contains("--output-format stream-json"));
        assert!(join.contains("--verbose"), "stream-json 必须配 --verbose（实测）");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_engine_that_never_emits_a_result_is_reported_not_hung() {
        let cwd = tmp("noresult");
        let stub = write_stub(&cwd, "#!/bin/sh\nsleep 30\n");
        let started = Instant::now();
        let r = run(
            &stub.to_string_lossy(),
            &[],
            &cwd,
            &HashMap::new(),
            "x",
            Duration::from_millis(600),
        );
        assert!(!r.ok);
        assert!(r.result.is_none());
        assert!(r.error.clone().unwrap_or_default().contains("超时"), "{:?}", r.error);
        // ★ 硬不变量：超时必须约束**墙钟**，而不只是"标记为超时"。
        // 这条测试当初就是这么失败的：桩脚本的 `sleep` 是孙进程，在 ps 不可用的
        // 沙箱里树清扫只能降级为杀 root，孙代握着管道 → join() 卡满 30 秒。
        assert!(started.elapsed() < Duration::from_secs(5), "超时必须真的约束墙钟，实际 {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_missing_engine_binary_says_so() {
        let cwd = tmp("nocmd");
        let r = run("/definitely/not/here", &[], &cwd, &HashMap::new(), "x", Duration::from_secs(1));
        assert!(!r.ok);
        assert!(r.error.unwrap_or_default().contains("无法启动"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn run_text_shows_money_and_never_hides_a_failure() {
        let cwd = tmp("text");
        let stub = write_stub(
            &cwd,
            r#"#!/bin/sh
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":5,"duration_api_ms":4,"total_cost_usd":0.174335,"result":"ok","session_id":"s","usage":{"input_tokens":34857,"output_tokens":2},"modelUsage":{"deepseek-v4-flash[1m]":{"inputTokens":34857,"outputTokens":2,"costUSD":0.174335,"contextWindow":1000000,"maxOutputTokens":32000}}}'
"#,
        );
        let r = run(&stub.to_string_lossy(), &[], &cwd, &HashMap::new(), "x", Duration::from_secs(10));
        let t = run_text(&r);
        assert!(t.contains("$0.174335"), "{t}");
        assert!(t.contains("34857"));
        assert!(t.contains("窗口 1000000"), "引擎自报的窗口要显示：{t}");
        assert!(t.contains("最大输出 32000"));

        // 失败时不能装作成功
        let bad = run("/definitely/not/here", &[], &cwd, &HashMap::new(), "x", Duration::from_secs(1));
        let bt = run_text(&bad);
        assert!(bt.contains("失败"));
        assert!(bt.contains("没有拿到结算"));
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
