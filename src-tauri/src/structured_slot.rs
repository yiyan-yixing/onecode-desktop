//! 结构化常驻 Slot —— 目标 ① 的最后一块："可换引擎"不该要求引擎长得像终端。
//!
//! 与 PTY 槽位的区别：不走 TUI，走**结构化协议**。
//!
//! ```text
//! <engine> <plan args> -p <prompt> --output-format stream-json --verbose --session-id <id>
//! ```
//!
//! 于是：输出是事件、每轮有**结算**（金额/用量）、多轮靠**固定 session id** 接续。
//! 与 `engine_run`（跑一次就结束）的区别就是那个 session id 与**落盘的对话记录**。
//!
//! 实测依据：
//! - `--session-id <uuid>` **被 CLI 接受**（2026-10-14 探测：init 事件正常发出）；
//! - `--resume <id>` 被拒、`--continue` 被接受 —— 所以"指定会话"这条路走 `--session-id`。
//!
//! ⚠️ **实测更正（2026-10-14，本机真实调用）**：`--session-id` **只给会话打标签，
//! 不接续上下文**。证据：同一个 id 跑第二轮，`input_tokens` 从 **10038 掉到 367**，
//! 而模型明确回答"我没有你先前告诉我的数字记录…这段对话里没有相关上下文"。
//! 顺带测了 `--resume <id>`：**零成本失败**（`subtype=error_during_execution`、
//! `duration_ms=0`、`total_cost_usd=0`、连错误文本都没有）。
//!
//! 所以多轮上下文**由我们自己拥有**：每轮把之前的对话**回放**进提示词
//! （见 [`compose_prompt`]）。这不只是权宜 —— 它对**任何引擎**都成立，
//! 而"换引擎后对话就断了"恰恰违背 ① 的"可换引擎"。
//! `--session-id` 仍然传下去，但只当**标签**用（便于在引擎自己的转录里找到这一次），
//! 绝不依赖它接续。

use std::path::{Path, PathBuf};

/// 一轮对话。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Turn {
    pub at: String,
    pub prompt: String,
    pub reply: String,
    pub ok: bool,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    /// 引擎回给自己的会话 id（与请求时传下去的**可能不同**，以引擎回的为准）
    pub session_id: String,
    /// 失败原因（成功时为空）
    #[serde(default)]
    pub error: String,
    /// 协议特有的、值得人知道的事实（如 ACP 的 stopReason / 上下文占用 / 权限请求次数）。
    /// 各协议能报的东西不一样 —— 与其硬塞进 input/output（会假），不如单独一句话说清。
    #[serde(default)]
    pub extra_meta: String,
}

/// 一个结构化槽位的状态 = 头 + 若干轮。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StructuredSlot {
    pub id: String,
    pub cwd: String,
    pub agent: String,
    pub provider_id: String,
    pub model: String,
    /// 我们**传下去**的会话 id（多轮接续靠它）
    pub session_id: String,
    /// 引擎**回过来**的会话 id（以它为准；不一致时界面要能看出来）
    pub engine_session_id: String,
    pub created_at: String,
    pub turns: Vec<Turn>,
}

impl StructuredSlot {
    /// 累计金额（把每轮结算加起来 —— 与账本同源，但这里是这个槽位自己的账）
    pub fn total_cost(&self) -> f64 {
        self.turns.iter().map(|t| t.cost_usd).sum()
    }
    pub fn total_input(&self) -> u64 {
        self.turns.iter().map(|t| t.input_tokens).sum()
    }
    pub fn total_output(&self) -> u64 {
        self.turns.iter().map(|t| t.output_tokens).sum()
    }
    /// 引擎回的会话 id 与我们传下去的不一致 —— 值得显眼（说明接续可能没按预期走）
    pub fn session_mismatch(&self) -> bool {
        !self.engine_session_id.is_empty()
            && !self.session_id.is_empty()
            && self.engine_session_id != self.session_id
    }
}

/// 给界面的视图：槽位本体 + **算好的合计**。
///
/// 为什么把合计放进视图：前端一旦自己把 `turns` 加一遍，就会与后端漂移
/// （第 9 刀本地模型档、第 12 刀工作台都吃过这个亏）。判断与聚合留在 Rust。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StructuredSlotView {
    #[serde(flatten)]
    pub slot: StructuredSlot,
    pub total_cost_usd: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// 引擎回的会话 id 与我们传下去的不一致（接续可能没按预期走）
    pub session_mismatch: bool,
}

impl From<StructuredSlot> for StructuredSlotView {
    fn from(slot: StructuredSlot) -> Self {
        Self {
            total_cost_usd: slot.total_cost(),
            total_input_tokens: slot.total_input(),
            total_output_tokens: slot.total_output(),
            session_mismatch: slot.session_mismatch(),
            slot,
        }
    }
}

/// 这一轮**真正发给引擎**的提示词：把之前的对话回放进来。
///
/// 为什么必须自己回放：实测 `--session-id` 不接续上下文（见模块文档），
/// 而"我们自己拥有对话"是唯一能跨引擎成立的做法。
///
/// `budget_chars` 是历史部分的上限：**超了就丢最早的几轮**，并在提示词里
/// **如实写明丢了几轮** —— 静默截断会让模型以为"我们从头聊到现在"。
pub fn compose_prompt(turns: &[Turn], prompt: &str, budget_chars: usize) -> String {
    if turns.is_empty() {
        return prompt.to_string();
    }
    // 从**最近**往前收，收到预算用尽为止
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for t in turns.iter().rev() {
        let line = if t.ok {
            format!("用户：{}\n助手：{}", t.prompt, t.reply)
        } else {
            // 失败的轮次也要带上：否则模型会以为"上一句我已经答过了"
            format!("用户：{}\n助手：（上一轮失败：{}）", t.prompt, t.error)
        };
        if used + line.len() > budget_chars && !kept.is_empty() {
            break;
        }
        used += line.len();
        kept.push(line);
    }
    kept.reverse();
    let dropped = turns.len() - kept.len();

    let mut out = String::new();
    out.push_str("以下是我们之前这段对话的历史，请据此理解上下文并回答最后一问。\n\n");
    if dropped > 0 {
        out.push_str(&format!("（更早的 {dropped} 轮已因长度省略）\n\n"));
    }
    out.push_str(&kept.join("\n\n"));
    out.push_str("\n\n用户：");
    out.push_str(prompt);
    out
}

/// 把一次 ACP 运行的结果变成一轮记录（**纯函数**，便于单测）。
///
/// 两点刻意的诚实：
/// 1. ACP 的 `usage_update` 报的是**会话级**的上下文占用（`used`/`size`）与累计花费，
///    **不是本轮**的输入/输出 token —— 所以这里 input/output 记 0，把上下文占用
///    写进 `extra_meta`。硬把 `used` 当 input 会污染账本。
/// 2. `stopReason` 是 `refusal` / `cancelled` / `max_tokens` 时**不算成功** ——
///    "代理拒绝继续"不该显示成正常完成。
pub fn turn_from_acp(rep: &crate::acp::AcpReport, prompt: &str) -> Turn {
    let refused = matches!(
        rep.stop_reason.as_str(),
        "refusal" | "cancelled" | "max_tokens" | "max_turn_requests"
    );
    let ok = rep.ok && !refused;

    let mut meta = String::from("ACP");
    if !rep.agent_name.is_empty() {
        meta.push_str(&format!("（{}）", rep.agent_name));
    }
    meta.push_str(&format!(
        " · stop={}",
        if rep.stop_reason.is_empty() { "无" } else { rep.stop_reason.as_str() }
    ));
    if rep.size_tokens > 0 {
        meta.push_str(&format!(" · 上下文 {}/{}", rep.used_tokens, rep.size_tokens));
    }
    if rep.permission_requests > 0 {
        meta.push_str(&format!(
            " · 权限请求 {} 次（拒绝 {}）",
            rep.permission_requests, rep.permission_denials
        ));
    }
    if rep.unsupported_requests > 0 {
        meta.push_str(&format!(" · 我们无法服务的请求 {} 次", rep.unsupported_requests));
    }
    if len_limited(&rep.thought) {
        meta.push_str(" · 有思考片段（未计入回复）");
    }

    Turn {
        at: chrono::Utc::now().to_rfc3339(),
        prompt: prompt.to_string(),
        reply: rep.reply.clone(),
        ok,
        // 引擎自报的累计花费；照抄，不换算
        cost_usd: rep.cost_usd,
        input_tokens: 0,
        output_tokens: 0,
        duration_ms: rep.wall_ms,
        session_id: rep.session_id.clone(),
        error: if ok {
            String::new()
        } else if let Some(e) = &rep.error {
            e.clone()
        } else if refused {
            format!("引擎停下：stopReason={}", rep.stop_reason)
        } else {
            "没有正常结束".to_string()
        },
        extra_meta: meta,
    }
}

fn len_limited(s: &str) -> bool {
    !s.trim().is_empty()
}

/// 槽位记录目录（相对 cwd）。
pub const DIR: &str = ".onecode/structured";

pub fn slot_path(cwd: &Path, id: &str) -> PathBuf {
    cwd.join(DIR).join(format!("{id}.jsonl"))
}

/// 槽位 id 白名单（会拼进路径）。
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// 开一个新槽位（生成会话 id）。
pub fn open(
    cwd: &Path,
    agent: &str,
    provider_id: &str,
    model: &str,
    id: &str,
    session_id: &str,
) -> Result<StructuredSlot, String> {
    if !is_safe_id(id) {
        return Err(format!("非法槽位 id：{id:?}"));
    }
    let slot = StructuredSlot {
        id: id.to_string(),
        cwd: cwd.to_string_lossy().into_owned(),
        agent: agent.to_string(),
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        session_id: session_id.to_string(),
        engine_session_id: String::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
        turns: Vec::new(),
    };
    // 先把头写下去 —— 这样"开过但还没说话"的槽位也存在（刷新后能看到）
    write_header(cwd, &slot)?;
    Ok(slot)
}

fn write_header(cwd: &Path, slot: &StructuredSlot) -> Result<(), String> {
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let header = serde_json::json!({
        "kind": "slot",
        "id": slot.id,
        "agent": slot.agent,
        "provider_id": slot.provider_id,
        "model": slot.model,
        "session_id": slot.session_id,
        "created_at": slot.created_at,
    });
    std::fs::write(
        slot_path(cwd, &slot.id),
        format!("{}\n", serde_json::to_string(&header).map_err(|e| e.to_string())?),
    )
    .map_err(|e| format!("写入失败：{e}"))
}

/// 追加一轮（append-only）。
pub fn append_turn(cwd: &Path, id: &str, turn: &Turn) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("非法槽位 id：{id:?}"));
    }
    let path = slot_path(cwd, id);
    if !path.exists() {
        return Err(format!("槽位不存在：{id}"));
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开失败：{e}"))?;
    writeln!(f, "{}", serde_json::to_string(turn).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入失败：{e}"))
}

/// 读一个槽位（头 + 所有轮）。
///
/// 坏行**跳过并计数** —— 返回 `(slot, bad_lines)`：坏行必须能被说出来，
/// 否则"对话少了几句"会被当成"我说过啊"。
pub fn read(cwd: &Path, id: &str) -> Result<(StructuredSlot, usize), String> {
    if !is_safe_id(id) {
        return Err(format!("非法槽位 id：{id:?}"));
    }
    let raw = std::fs::read_to_string(slot_path(cwd, id))
        .map_err(|e| format!("读取失败：{e}"))?;
    let mut slot: Option<StructuredSlot> = None;
    let mut turns: Vec<Turn> = Vec::new();
    let mut bad = 0usize;

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            bad += 1;
            continue;
        };
        if v.get("kind").and_then(|k| k.as_str()) == Some("slot") {
            slot = Some(StructuredSlot {
                id: v.get("id").and_then(|x| x.as_str()).unwrap_or(id).to_string(),
                cwd: cwd.to_string_lossy().into_owned(),
                agent: v.get("agent").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                provider_id: v.get("provider_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                model: v.get("model").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                session_id: v.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                engine_session_id: String::new(),
                created_at: v.get("created_at").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                turns: Vec::new(),
            });
            continue;
        }
        match serde_json::from_str::<Turn>(line) {
            Ok(t) => turns.push(t),
            Err(_) => bad += 1,
        }
    }

    let mut slot = slot.ok_or_else(|| format!("槽位文件缺少头部：{id}"))?;
    // 引擎回的会话 id 取**最后一轮**的（它才代表当前实际会话）
    slot.engine_session_id = turns.last().map(|t| t.session_id.clone()).unwrap_or_default();
    slot.turns = turns;
    Ok((slot, bad))
}

/// 列出一个项目里所有结构化槽位（供前端恢复）。
pub fn list(cwd: &Path) -> (Vec<StructuredSlot>, usize) {
    let dir = cwd.join(DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), 0);
    };
    let mut out = Vec::new();
    let mut bad = 0usize;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match read(cwd, &id) {
            Ok((s, b)) => {
                bad += b;
                out.push(s);
            }
            Err(_) => bad += 1,
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    (out, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-ss-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn turn(prompt: &str, reply: &str, cost: f64) -> Turn {
        Turn {
            at: "2026-10-14T00:00:00Z".to_string(),
            prompt: prompt.to_string(),
            reply: reply.to_string(),
            ok: true,
            cost_usd: cost,
            input_tokens: 100,
            output_tokens: 10,
            duration_ms: 500,
            session_id: "sess-1".to_string(),
            error: String::new(),
            extra_meta: String::new(),
        }
    }

    #[test]
    fn open_writes_a_header_so_an_empty_slot_survives_a_refresh() {
        let cwd = tmp("open");
        let s = open(&cwd, "dev", "glm", "glm-4", "slot-a", "sess-1").unwrap();
        assert_eq!(s.id, "slot-a");
        assert!(slot_path(&cwd, "slot-a").exists());
        let (back, bad) = read(&cwd, "slot-a").unwrap();
        assert_eq!(bad, 0);
        assert_eq!(back.agent, "dev");
        assert_eq!(back.session_id, "sess-1");
        assert!(back.turns.is_empty(), "开过但还没说话的槽位也要能读回来");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn turns_are_appended_and_totals_add_up() {
        let cwd = tmp("turns");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "sess-1").unwrap();
        append_turn(&cwd, "slot-a", &turn("第一句", "第一次回答", 0.10)).unwrap();
        append_turn(&cwd, "slot-a", &turn("第二句", "第二次回答", 0.25)).unwrap();

        let (s, bad) = read(&cwd, "slot-a").unwrap();
        assert_eq!(bad, 0);
        assert_eq!(s.turns.len(), 2);
        assert_eq!(s.turns[0].prompt, "第一句");
        assert!((s.total_cost() - 0.35).abs() < 1e-9, "槽位自己的账要能加起来");
        assert_eq!(s.total_input(), 200);
        assert_eq!(s.total_output(), 20);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 同一个会话 id 必须**稳定传下去** —— 多轮接续全靠它。
    #[test]
    fn the_session_id_is_stable_across_turns() {
        let cwd = tmp("sess");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "fixed-session-id").unwrap();
        // 轮次里记的是**引擎回给我们的** id —— 与请求一致时才不算 mismatch
        let mut t1 = turn("a", "b", 0.0);
        t1.session_id = "fixed-session-id".to_string();
        let mut t2 = turn("c", "d", 0.0);
        t2.session_id = "fixed-session-id".to_string();
        append_turn(&cwd, "slot-a", &t1).unwrap();
        append_turn(&cwd, "slot-a", &t2).unwrap();
        let (s, _) = read(&cwd, "slot-a").unwrap();
        assert_eq!(s.session_id, "fixed-session-id");
        assert!(!s.session_mismatch(), "引擎回的和传下去的一致时不该报不一致");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 引擎回的会话 id 与传下去的不一致 —— 要能被看出来（说明接续可能没按预期走）。
    #[test]
    fn a_session_mismatch_is_visible() {
        let cwd = tmp("mismatch");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "requested-id").unwrap();
        let mut t = turn("a", "b", 0.0);
        t.session_id = "engine-chose-this".to_string();
        append_turn(&cwd, "slot-a", &t).unwrap();
        let (s, _) = read(&cwd, "slot-a").unwrap();
        assert_eq!(s.engine_session_id, "engine-chose-this", "以引擎回的为准");
        assert!(s.session_mismatch(), "不一致必须能被发现");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_broken_line_is_counted_not_silently_dropped() {
        let cwd = tmp("broken");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "s").unwrap();
        append_turn(&cwd, "slot-a", &turn("a", "b", 0.1)).unwrap();
        let mut text = std::fs::read_to_string(slot_path(&cwd, "slot-a")).unwrap();
        text.push_str("{ 这不是 json\n");
        text.push_str(&serde_json::to_string(&turn("c", "d", 0.2)).unwrap());
        text.push('\n');
        std::fs::write(slot_path(&cwd, "slot-a"), text).unwrap();

        let (s, bad) = read(&cwd, "slot-a").unwrap();
        assert_eq!(s.turns.len(), 2, "坏行不该影响好行");
        assert_eq!(bad, 1, "坏行要计数 —— 否则'对话少了几句'会被当成'我说过啊'");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_failed_turn_is_recorded_with_its_reason() {
        let cwd = tmp("fail");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "s").unwrap();
        let mut t = turn("a", "", 0.0);
        t.ok = false;
        t.error = "超时（300s）内没有 result 事件".to_string();
        append_turn(&cwd, "slot-a", &t).unwrap();
        let (s, _) = read(&cwd, "slot-a").unwrap();
        assert!(!s.turns[0].ok);
        assert_eq!(s.turns[0].error, "超时（300s）内没有 result 事件");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn list_returns_slots_in_creation_order_and_counts_bad_ones() {
        let cwd = tmp("list");
        open(&cwd, "dev", "glm", "glm-4", "a", "s1").unwrap();
        open(&cwd, "writer", "glm", "glm-4", "b", "s2").unwrap();
        std::fs::write(cwd.join(DIR).join("c.jsonl"), "{ 坏文件\n").unwrap();
        let (slots, bad) = list(&cwd);
        assert_eq!(slots.len(), 2);
        assert_eq!(bad, 1, "读不出来的槽位也要计数");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn unsafe_ids_are_refused_everywhere() {
        let cwd = tmp("unsafe");
        assert!(open(&cwd, "dev", "p", "m", "../escape", "s").is_err());
        assert!(append_turn(&cwd, "../escape", &turn("a", "b", 0.0)).is_err());
        assert!(read(&cwd, "../escape").is_err());
        assert!(read(&cwd, "has space").is_err());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn appending_to_a_missing_slot_is_an_error_not_a_silent_create() {
        let cwd = tmp("missing");
        let e = append_turn(&cwd, "ghost", &turn("a", "b", 0.0)).unwrap_err();
        assert!(e.contains("不存在"), "{e}");
        let _ = std::fs::remove_dir_all(&cwd);
    }


    // ── 多轮上下文：由我们自己回放（实测引擎不接续）──────────────────

    #[test]
    fn no_history_means_the_prompt_is_sent_as_is() {
        assert_eq!(compose_prompt(&[], "你好", 1000), "你好");
    }

    #[test]
    fn previous_turns_are_replayed_into_the_prompt() {
        let turns = vec![turn("记住 42", "ok", 0.0)];
        let p = compose_prompt(&turns, "我刚才让你记的是什么？", 10_000);
        assert!(p.contains("用户：记住 42"), "{p}");
        assert!(p.contains("助手：ok"), "{p}");
        assert!(p.trim_end().ends_with("用户：我刚才让你记的是什么？"), "{p}");
        assert!(p.contains("历史"), "要说明这是历史：{p}");
    }

    /// 一轮都没丢时**不该**出现"省略"字样（否则模型会以为丢了东西）。
    #[test]
    fn no_truncation_note_when_nothing_was_dropped() {
        let turns = vec![turn("a", "b", 0.0)];
        let p = compose_prompt(&turns, "c", 10_000);
        assert!(!p.contains("省略"), "{p}");
    }

    /// 超预算要**丢最早的**、并如实写明丢了几轮。
    #[test]
    fn over_budget_drops_the_oldest_turns_and_says_how_many() {
        let turns = vec![
            turn("第一轮", &"x".repeat(200), 0.0),
            turn("第二轮", &"y".repeat(200), 0.0),
            turn("第三轮", "最近一条", 0.0),
        ];
        let p = compose_prompt(&turns, "新问题", 300);
        assert!(p.contains("最近一条"), "最近的必须在：{p}");
        assert!(!p.contains("第一轮"), "最早的该被丢掉：{p}");
        assert!(p.contains("省略"), "丢了就要写明：{p}");
    }

    /// 失败的轮次要带上失败标记 —— 否则模型会以为"上一句我已经答过了"。
    #[test]
    fn a_failed_turn_is_replayed_as_failed() {
        let mut t = turn("上一句", "", 0.0);
        t.ok = false;
        t.error = "超时".to_string();
        let p = compose_prompt(&[t], "继续", 10_000);
        assert!(p.contains("上一轮失败：超时"), "{p}");
    }

    /// 端到端（真机实测过的那条路径）：回放之后模型才答得出早期信息。
    /// 这里只断言"历史确实进了提示词"——真实调用那一步在第 20 轮实测做过
    /// （模型答出 42，$0.050710，输入 9932 token）。
    #[test]
    fn the_replay_carries_the_early_fact_forward() {
        let turns = vec![turn("请记住这个数字：42。只回复 ok", "ok", 0.0)];
        let p = compose_prompt(&turns, "我刚才让你记住的数字是多少？只回复数字", 32 * 1024);
        assert!(p.contains("42"), "早期事实必须在提示词里");
    }

    // ── 交给前端的 JSON **必须**带算好的合计 ──────────────────────────
    //
    // 这条测试的来历：第 21 刀发现 `structured_send` 的返回类型漏了视图，
    // 于是真前端会显示"合计 $0.000000"。启动检查用的是桩，**照不出来**
    // —— 所以把"交给前端的形状"钉在单元层。

    #[test]
    fn the_json_handed_to_the_frontend_carries_the_computed_totals() {
        let cwd = tmp("viewjson");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "s").unwrap();
        // 夹具的轮次 session_id 要与槽位请求的一致（否则 session_mismatch 会是 true）
        let mut t1 = turn("a", "b", 0.25);
        t1.session_id = "s".to_string();
        let mut t2 = turn("c", "d", 0.10);
        t2.session_id = "s".to_string();
        append_turn(&cwd, "slot-a", &t1).unwrap();
        append_turn(&cwd, "slot-a", &t2).unwrap();
        let (slot, _) = read(&cwd, "slot-a").unwrap();
        let v = serde_json::to_value(StructuredSlotView::from(slot)).unwrap();

        assert!((v["total_cost_usd"].as_f64().unwrap() - 0.35).abs() < 1e-9, "{v}");
        assert_eq!(v["total_input_tokens"], 200);
        assert_eq!(v["total_output_tokens"], 20);
        assert_eq!(v["session_mismatch"], false);
        // 槽位本体也要在（前端要显示轮次与身份）
        assert_eq!(v["agent"], "dev");
        assert_eq!(v["turns"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn the_json_flags_a_session_mismatch_for_the_frontend() {
        let cwd = tmp("viewjson2");
        open(&cwd, "dev", "glm", "glm-4", "slot-a", "requested").unwrap();
        let mut t = turn("a", "b", 0.0);
        t.session_id = "engine-chose".to_string();
        append_turn(&cwd, "slot-a", &t).unwrap();
        let (slot, _) = read(&cwd, "slot-a").unwrap();
        let v = serde_json::to_value(StructuredSlotView::from(slot)).unwrap();
        assert_eq!(v["session_mismatch"], true, "不一致必须传到前端");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── ACP 轮次映射（纯函数）────────────────────────────────────────

    fn acp_report(stop: &str, ok: bool) -> crate::acp::AcpReport {
        crate::acp::AcpReport {
            ok,
            agent_name: "mock-agent".to_string(),
            protocol_version: 1,
            agent_load_session: true,
            session_id: "sess_acp".to_string(),
            reply: "你好".to_string(),
            thought: "想了想".to_string(),
            tool_calls: Vec::new(),
            stop_reason: stop.to_string(),
            cost_usd: 0.045,
            used_tokens: 53000,
            size_tokens: 200000,
            permission_requests: 2,
            permission_denials: 2,
            unsupported_requests: 1,
            wall_ms: 1234,
            error: None,
            stderr_tail: String::new(),
            actions: Vec::new(),
        }
    }

    #[test]
    fn an_acp_turn_records_cost_and_session_but_not_fake_token_counts() {
        let t = turn_from_acp(&acp_report("end_turn", true), "问一句");
        assert!(t.ok);
        assert_eq!(t.reply, "你好");
        assert!((t.cost_usd - 0.045).abs() < 1e-12, "引擎自报的花费要照抄");
        assert_eq!(t.session_id, "sess_acp");
        // ★ ACP 报的是**会话级**上下文占用，不是本轮 token —— 记 0 才诚实
        assert_eq!(t.input_tokens, 0);
        assert_eq!(t.output_tokens, 0);
        // 上下文占用进 extra_meta，别混进 token
        assert!(t.extra_meta.contains("53000/200000"), "{}", t.extra_meta);
        assert!(t.extra_meta.contains("权限请求 2 次（拒绝 2）"), "{}", t.extra_meta);
        assert!(t.extra_meta.contains("无法服务的请求 1 次"), "{}", t.extra_meta);
    }

    /// `refusal` / `cancelled` / `max_tokens` **不算成功** —— 代理拒绝继续
    /// 不该显示成正常完成。
    #[test]
    fn a_refusal_or_cancellation_is_not_a_success() {
        for stop in ["refusal", "cancelled", "max_tokens", "max_turn_requests"] {
            let t = turn_from_acp(&acp_report(stop, true), "问一句");
            assert!(!t.ok, "stop={stop} 不该算成功");
            assert!(t.error.contains(stop), "要说清为什么：{}", t.error);
            assert!(t.extra_meta.contains(stop), "{}", t.extra_meta);
        }
    }

    #[test]
    fn a_normal_stop_reason_is_a_success() {
        let t = turn_from_acp(&acp_report("end_turn", true), "问一句");
        assert!(t.ok);
        assert!(t.error.is_empty(), "{}", t.error);
    }

    /// 有错就如实带出（例如版本不一致 / 启动失败）。
    #[test]
    fn an_acp_error_is_carried_through() {
        let mut r = acp_report("", false);
        r.error = Some("协议版本不一致：我们 1，agent 99".to_string());
        let t = turn_from_acp(&r, "问一句");
        assert!(!t.ok);
        assert!(t.error.contains("版本不一致"), "{}", t.error);
    }

    /// `extra_meta` 是后端算好的事实；渲染层只转述。
    #[test]
    fn extra_meta_survives_the_json_round_trip() {
        let cwd = tmp("meta");
        open(&cwd, "dev", "glm", "glm-4", "s1", "sess").unwrap();
        let mut t = turn("a", "b", 0.0);
        t.extra_meta = "ACP · stop=end_turn · 上下文 1/2".to_string();
        append_turn(&cwd, "s1", &t).unwrap();
        let (back, _) = read(&cwd, "s1").unwrap();
        assert_eq!(back.turns[0].extra_meta, "ACP · stop=end_turn · 上下文 1/2");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
