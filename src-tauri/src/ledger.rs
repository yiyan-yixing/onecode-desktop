//! 账本（目标 ⑤ 的第四块）。
//!
//! 账本只记**可测量的事实**，分两侧：
//!
//! **一、产出侧（我们自己的记录）** —— 任务与产出本就落在
//!   `<cwd>/.onecode/{tasks,deliverables}/`，这里只做聚合：
//!   完成率、验收通过率、证据通过率、**产出有没有挂到任务上**、谁产出的。
//!
//! **二、机时侧（token）** —— 本机实测：Claude Code 的 transcript 每条 assistant
//!   消息都带 `message.usage.{input,output,cache_creation,cache_read}_tokens`，
//!   还有 `message.model` 与 `isSidechain`。所以能算出按模型拆分、以及
//!   **子代理 vs 主对话**的用量。
//!
//! **成本（金额）故意不算**：transcript 里**没有** `total_cost_usd`/`costUSD`
//!   （实测：出现 0 次）。算金额需要一张价格表（models.dev 的 `cost` 字段有）。
//!   在接上价格表之前，本模块**只报 token**，并在报告里写明金额为何缺席 ——
//!   编一个数字出来比不报更糟。
//!
//! 另外：**两个 transcript 根要分开报**。调度台的槽位跑在隔离配置目录里
//! （`CLAUDE_CONFIG_DIR` → `~/.onecode/cc-config/projects`），用户自己手敲的
//! Claude Code 在 `~/.claude/projects`。混在一起就答不了"这个调度台花了多少"。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{deliverables, tasks};

/// 一段用量的合计。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct UsageTotal {
    pub sessions: usize,
    pub messages: u64,
    pub input: u64,
    pub output: u64,
    /// 缓存写入（通常按更高单价计费，所以单独列）
    pub cache_create: u64,
    /// 缓存命中（通常最便宜）
    pub cache_read: u64,
}

impl UsageTotal {
    /// 全部 token（含缓存命中），只用于说明规模。
    pub fn all_tokens(&self) -> u64 {
        self.input + self.cache_create + self.cache_read + self.output
    }

    fn add_assign(&mut self, o: &UsageTotal) {
        self.sessions += o.sessions;
        self.messages += o.messages;
        self.input += o.input;
        self.output += o.output;
        self.cache_create += o.cache_create;
        self.cache_read += o.cache_read;
    }
}

/// 按员工（任务负责人）聚合的产出情况。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EmployeeRow {
    /// 员工 id；空 owner 归到 `(未指派)`
    pub owner: String,
    pub tasks: usize,
    /// 生效状态为 done 的任务
    pub tasks_done: usize,
    /// 其余（含 dropped 之外的未完成）
    pub tasks_open: usize,
    /// 该员工任务关联到的产出数（按 id 去重）
    pub deliverables: usize,
    pub accepted: usize,
    /// 证据**验过且全过**的产出数
    pub evidence_passed: usize,
    /// 证据验过但有失败
    pub evidence_failed: usize,
    /// 没验过证据
    pub evidence_unverified: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Ledger {
    pub cwd: String,

    // ── 产出侧 ──
    pub tasks_total: usize,
    /// 按生效状态计（状态名, 条数），键有序，便于稳定显示与断言
    pub tasks_by_status: Vec<(String, usize)>,
    pub tasks_blocked: usize,
    /// 没有关联任何产出的任务数（"干了但没留下东西"）
    pub tasks_without_deliverables: usize,
    pub deliverables_total: usize,
    pub deliverables_by_status: Vec<(String, usize)>,
    /// 没有挂到任何任务上的产出数（"有东西但不知道该算哪项工作"）
    pub deliverables_without_task: usize,
    pub evidence_verified: usize,
    pub evidence_passed: usize,
    pub evidence_failed: usize,
    pub evidence_unverified: usize,
    pub employees: Vec<EmployeeRow>,

    // ── 机时侧（token）──
    /// 隔离配置目录（调度台的槽位）
    pub console_usage: UsageTotal,
    /// 用户自己的 `~/.claude`（含手敲的会话）
    pub user_usage: UsageTotal,
    /// 按模型拆分（两个根合计）
    pub by_model: Vec<(String, UsageTotal)>,
    /// **子代理**按执行者归属的用量（transcript 的 `attributionAgent`）
    pub by_agent: Vec<(String, UsageTotal)>,
    /// 子代理（`isSidechain`）的用量
    pub sidechain_usage: UsageTotal,
    /// 主对话的用量
    pub main_usage: UsageTotal,
    /// 两个根里**全部项目**的合计。`None` = 这次没算（快路径），见 `full`。
    pub console_usage_all_projects: Option<UsageTotal>,
    pub user_usage_all_projects: Option<UsageTotal>,
    /// 全库会话文件数 / 子代理文件数（数文件，永远是准确值）
    pub console_all_session_files: usize,
    pub console_all_subagent_files: usize,
    pub user_all_session_files: usize,
    pub user_all_subagent_files: usize,
    /// 本次是否做了全库完整扫描
    pub full: bool,

    // ── 金额（来自引擎自报，不是我们估的）──
    /// 有记录的结构化运行次数
    pub runs_total: usize,
    /// 运行记录里读不出来的行数（不静默）
    pub runs_bad_lines: usize,
    /// 合计金额（USD）
    pub cost_usd_total: f64,
    /// 按员工：`(agent, 金额, 次数)`，金额降序
    pub cost_by_agent: Vec<(String, f64, usize)>,
    /// 按模型：`(model, 金额, 次数)`
    pub cost_by_model: Vec<(String, f64, usize)>,
    /// 按**任务**：`(task_id, 金额, 次数)` —— 答"这条活花了多少钱"
    pub cost_by_task: Vec<(String, f64, usize)>,
    /// 失败的运行次数。失败的花费通常是 0，但"跑了几次都没成"本身是管理信息 ——
    /// 只看金额会把"跑三次都失败"看成"什么都没发生"。
    pub runs_failed: usize,
    /// 引擎自报过的上下文窗口：`(model, window)`（升序去重）
    pub context_windows: Vec<(String, u64)>,

    /// 关于金额从哪来的说明（**不是占位符，是结论**）
    pub cost_note: String,
}

/// 把一个绝对路径编码成 Claude Code 的 project 目录名。
///
/// 实测约定：`/Users/zhanglei/yycc/eng/onecode-desktop` →
/// `-Users-zhanglei-yycc-eng-onecode-desktop`（就是把 `/` 换成 `-`）。
///
/// 注意：**不能反向解码**。`cc_sessions::decode_project_dir` 把 `-` 一律还原成
/// `/`，于是 `eng-onecode-desktop` 会被解成 `eng/onecode/desktop`（错的）——
/// 那条路径只用于展示，本模块改用"正向编码 + 精确匹配"来定位目录。
pub fn encode_project_dir(cwd: &Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

/// 从一个 transcript 根目录里读取用量。`only_project` = 只算匹配的项目目录。
struct RootUsage {
    matched: UsageTotal,
    /// 全库合计 —— **只有 `full` 才算**（完整扫描本机实测约 15 秒，见 `read_root` 注释）
    all: UsageTotal,
    by_model: BTreeMap<String, UsageTotal>,
    by_agent: BTreeMap<String, UsageTotal>,
    sidechain: UsageTotal,
    main: UsageTotal,
    /// 全库的会话文件数 / 子代理文件数（**数文件不读内容**，毫秒级）
    all_session_files: usize,
    all_subagent_files: usize,
}

/// 读一个 transcript 根。
///
/// `full == false`（默认）：**只读匹配的项目目录**，其余项目**只数文件不读内容**。
/// 为什么：本机实测完整扫描（1767 个会话 + 906 个子代理转录）要 **~15 秒**，
/// 瓶颈是文件 I/O 而不是 JSON 解析（加廉价预筛只省了 3 秒）。账本是随时要点的东西，
/// 不能一按卡十几秒。所以本项目数字**永远瞬间**，全库合计走显式的慢路径
/// （`scan_full`），并且**没算的时候如实说没算**，不给一个假数。
fn read_root(root: &Path, only_project: Option<&str>, full: bool) -> RootUsage {
    let mut out = RootUsage {
        matched: UsageTotal::default(),
        all: UsageTotal::default(),
        by_model: BTreeMap::new(),
        by_agent: BTreeMap::new(),
        sidechain: UsageTotal::default(),
        main: UsageTotal::default(),
        all_session_files: 0,
        all_subagent_files: 0,
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let is_matched = only_project.map(|p| p == name).unwrap_or(false);

        // 数文件不计内容：读目录比读内容便宜几个数量级
        let (sess_files, sub_files) = count_transcripts(&dir);
        out.all_session_files += sess_files;
        out.all_subagent_files += sub_files;

        if !is_matched && !full {
            continue; // 非本项目且不算全库 → 不读内容
        }

        let sub = match read_project_dir(&dir) {
            Some(s) => s,
            None => continue,
        };
        // all 只收合计；by_model/sidechain 只按匹配项目统计（否则跨项目混在一起没意义）
        out.all.add_assign(&sub.total);
        if is_matched {
            out.matched.add_assign(&sub.total);
            out.sidechain.add_assign(&sub.sidechain);
            out.main.add_assign(&sub.main);
            for (m, u) in &sub.by_model {
                out.by_model.entry(m.clone()).or_default().add_assign(u);
            }
            for (a, u) in &sub.by_agent {
                out.by_agent.entry(a.clone()).or_default().add_assign(u);
            }
        }
    }
    out
}

#[derive(Default)]
struct ProjectUsage {
    total: UsageTotal,
    by_model: BTreeMap<String, UsageTotal>,
    /// 子代理的用量按**执行者**归属（transcript 里的 `attributionAgent`）
    by_agent: BTreeMap<String, UsageTotal>,
    sidechain: UsageTotal,
    main: UsageTotal,
}

/// 数一个项目目录下的转录文件（会话 + 子代理），**不读内容**。
fn count_transcripts(dir: &Path) -> (usize, usize) {
    let mut sess = 0usize;
    let mut subs = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if let Ok(se) = std::fs::read_dir(path.join("subagents")) {
                subs += se
                    .flatten()
                    .filter(|x| x.path().extension().and_then(|y| y.to_str()) == Some("jsonl"))
                    .count();
            }
            continue;
        }
        if path.extension().and_then(|y| y.to_str()) == Some("jsonl") {
            sess += 1;
        }
    }
    (sess, subs)
}

/// 解析一个 jsonl 转录文件的用量并累加。
///
/// `force_sidechain`：`subagents/` 目录下的文件**整份都是子代理**。
/// 本机实测：这类文件里 `isSidechain` 确实是 true，但**不能依赖它** ——
/// 早期版本可能不写。目录位置本身是更可靠的信号。
fn parse_transcript(path: &Path, force_sidechain: bool, out: &mut ProjectUsage) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else { return 0 };
    let mut msgs = 0u64;
    for line in content.lines() {
        // ★ 廉价预筛：transcript 里绝大多数行是用户消息/工具结果，**不含 usage**。
        // 先做子串检查再解析 JSON —— 对真实语料（2700+ 个文件、单个可达 MB）是
        // 数倍到十几倍的差别，直接把账本从"卡十几秒"变成可点。
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else { continue };
        let Some(u) = msg.get("usage") else { continue };
        let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let (input, output) = (g("input_tokens"), g("output_tokens"));
        let cache_create = g("cache_creation_input_tokens");
        let cache_read = g("cache_read_input_tokens");
        // 全 0 的行没有信息量，跳过以免虚增消息数
        if input == 0 && output == 0 && cache_create == 0 && cache_read == 0 {
            continue;
        }
        let one = UsageTotal { sessions: 0, messages: 1, input, output, cache_create, cache_read };
        out.total.messages += 1;
        out.total.input += input;
        out.total.output += output;
        out.total.cache_create += cache_create;
        out.total.cache_read += cache_read;
        msgs += 1;

        let is_side = force_sidechain
            || v.get("isSidechain").and_then(|b| b.as_bool()).unwrap_or(false);
        if is_side {
            out.sidechain.add_assign(&one);
            // 归属：子代理转录带 attributionAgent（实测），没有就退回 agentId
            let who = v
                .get("attributionAgent")
                .and_then(|a| a.as_str())
                .or_else(|| v.get("agentId").and_then(|a| a.as_str()))
                .filter(|s| !s.is_empty())
                .unwrap_or("(未标注执行者)")
                .to_string();
            out.by_agent.entry(who).or_default().add_assign(&one);
        } else {
            out.main.add_assign(&one);
        }

        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .unwrap_or("(未知模型)")
            .to_string();
        out.by_model.entry(model).or_default().add_assign(&one);
    }
    msgs
}

/// 读一个项目目录下所有 transcript 的用量，**含子代理**。
///
/// 目录布局（本机实测）：
/// ```text
/// <projectDir>/<sessionId>.jsonl              ← 主对话
/// <projectDir>/<sessionId>/subagents/agent-*.jsonl  ← 子代理（独立转录）
/// <projectDir>/<sessionId>/tool-results/      ← 工具结果，不是转录
/// ```
/// 子代理的用量**不在**主转录里（本机实测 `isSidechain:true` 在主转录中出现 0 次，
/// 而 `subagents/` 下有 906 个文件），所以**必须下钻一层**，否则"子代理占多少"
/// 不是 0，而是**结构性看不见**。
fn read_project_dir(dir: &Path) -> Option<ProjectUsage> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut out = ProjectUsage::default();
    let mut any_session = false;

    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();

        if path.is_dir() {
            // <sessionId>/subagents/*.jsonl
            let sub_dir = path.join("subagents");
            let Ok(sub_entries) = std::fs::read_dir(&sub_dir) else { continue };
            let before = out.total.messages;
            for se in sub_entries.flatten() {
                let sp = se.path();
                if sp.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    continue;
                }
                parse_transcript(&sp, true, &mut out);
            }
            if out.total.messages > before {
                any_session = true;
            }
            continue;
        }

        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        if parse_transcript(&path, false, &mut out) > 0 {
            // 一个 jsonl 文件 = 一个会话
            out.total.sessions += 1;
            any_session = true;
        }
        let _ = name;
    }

    if any_session {
        Some(out)
    } else {
        None
    }
}

fn by_status<T, F: Fn(&T) -> String>(items: &[T], f: F) -> Vec<(String, usize)> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for i in items {
        *m.entry(f(i)).or_default() += 1;
    }
    m.into_iter().collect()
}

/// 用**指定的 transcript 根**做账（测试用；生产走 [`scan`]）。
pub fn scan_with_roots(cwd: &Path, roots: &[PathBuf]) -> Ledger {
    scan_with_roots_opts(cwd, roots, false)
}

/// 同上，但可指定是否做**全库完整扫描**（慢，实测 ~15 秒）。
pub fn scan_with_roots_opts(cwd: &Path, roots: &[PathBuf], full: bool) -> Ledger {
    let ds = deliverables::scan(cwd);
    let board = tasks::scan(cwd);

    // 任务侧
    let tasks_total = board.tasks.len();
    let tasks_by_status = by_status(&board.tasks, |t| t.status.clone());
    let tasks_blocked = board.tasks.iter().filter(|t| !t.unresolved_blockers.is_empty()).count();
    let tasks_without_deliverables =
        board.tasks.iter().filter(|t| t.deliverables.is_empty()).count();

    // 产出侧
    let deliverables_total = ds.deliverables.len();
    let deliverables_by_status = by_status(&ds.deliverables, |d| d.status.clone());

    // 哪些产出被任务引用了
    let mut linked: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in &board.tasks {
        for d in &t.deliverables {
            linked.insert(d.clone());
        }
    }
    let deliverables_without_task = ds
        .deliverables
        .iter()
        .filter(|d| !linked.contains(&d.id))
        .count();

    // 证据验证情况（只算真实存在的产出）
    let mut evidence_verified = 0usize;
    let mut evidence_passed = 0usize;
    let mut evidence_failed = 0usize;
    let mut evidence_unverified = 0usize;
    for d in &ds.deliverables {
        match &d.verification {
            None => evidence_unverified += 1,
            Some(v) => {
                evidence_verified += 1;
                if v.failed() == 0 && !v.results.is_empty() {
                    evidence_passed += 1;
                } else {
                    evidence_failed += 1;
                }
            }
        }
    }

    // 按员工聚合（键 = 任务负责人；空 → 未指派）
    let mut emp: BTreeMap<String, EmployeeRow> = BTreeMap::new();
    for t in &board.tasks {
        let owner = if t.owner.is_empty() { "(未指派)".to_string() } else { t.owner.clone() };
        let row = emp.entry(owner.clone()).or_insert_with(|| EmployeeRow {
            owner: owner.clone(),
            tasks: 0,
            tasks_done: 0,
            tasks_open: 0,
            deliverables: 0,
            accepted: 0,
            evidence_passed: 0,
            evidence_failed: 0,
            evidence_unverified: 0,
        });
        row.tasks += 1;
        if t.status == "done" {
            row.tasks_done += 1;
        } else {
            row.tasks_open += 1;
        }
        // 该任务关联产出的实况（去重，避免同一产出被多任务重复计数）
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for r in &t.deliverable_refs {
            if !seen.insert(r.id.as_str()) {
                continue;
            }
            row.deliverables += 1;
            if r.status == "accepted" {
                row.accepted += 1;
            }
            match r.verified {
                None => row.evidence_unverified += 1,
                Some((p, n)) => {
                    if n > 0 && p == n {
                        row.evidence_passed += 1;
                    } else {
                        row.evidence_failed += 1;
                    }
                }
            }
        }
    }
    let employees: Vec<EmployeeRow> = emp.into_values().collect();

    // ── 金额侧：来自引擎自报的 result 事件（见 engine_run）──
    let (runs, runs_bad_lines) = crate::engine_run::read_runs(cwd);
    let runs_total = runs.len();
    let cost_usd_total: f64 = runs.iter().map(|r| r.cost_usd).sum();
    let mut agent_map: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    let mut model_map: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    let mut window_map: BTreeMap<String, u64> = BTreeMap::new();
    let mut task_map: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    let mut runs_failed = 0usize;
    for r in &runs {
        let who = if r.agent.trim().is_empty() { "(未标注员工)".to_string() } else { r.agent.clone() };
        let a = agent_map.entry(who).or_insert((0.0, 0));
        a.0 += r.cost_usd;
        a.1 += 1;
        let m = if r.model.trim().is_empty() { "(未标注模型)".to_string() } else { r.model.clone() };
        let b = model_map.entry(m.clone()).or_insert((0.0, 0));
        b.0 += r.cost_usd;
        b.1 += 1;
        if let Some(w) = r.context_window {
            window_map.insert(m, w);
        }
        // 按任务归集：`label` 是任务 id（手工运行是 "manual"）
        if !r.ok {
            runs_failed += 1;
        }
        let task = if r.label.trim().is_empty() { "(未挂任务)".to_string() } else { r.label.clone() };
        let c = task_map.entry(task).or_insert((0.0, 0));
        c.0 += r.cost_usd;
        c.1 += 1;
    }
    let mut cost_by_agent: Vec<(String, f64, usize)> =
        agent_map.into_iter().map(|(k, (c, n))| (k, c, n)).collect();
    cost_by_agent.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut cost_by_model: Vec<(String, f64, usize)> =
        model_map.into_iter().map(|(k, (c, n))| (k, c, n)).collect();
    cost_by_model.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut cost_by_task: Vec<(String, f64, usize)> =
        task_map.into_iter().map(|(k, (c, n))| (k, c, n)).collect();
    cost_by_task.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let context_windows: Vec<(String, u64)> = window_map.into_iter().collect();

    // 机时侧
    let dir_name = encode_project_dir(cwd);
    let c = read_root(&roots[0], Some(&dir_name), full);
    let u = if roots.len() > 1 {
        read_root(&roots[1], Some(&dir_name), full)
    } else {
        RootUsage {
            matched: UsageTotal::default(),
            all: UsageTotal::default(),
            by_model: BTreeMap::new(),
            by_agent: BTreeMap::new(),
            sidechain: UsageTotal::default(),
            main: UsageTotal::default(),
            all_session_files: 0,
            all_subagent_files: 0,
        }
    };

    let mut by_model: Vec<(String, UsageTotal)> = c.by_model.clone().into_iter().collect();
    for (m, v) in &u.by_model {
        match by_model.iter_mut().find(|(k, _)| k == m) {
            Some((_, e)) => e.add_assign(v),
            None => by_model.push((m.clone(), v.clone())),
        }
    }
    by_model.sort_by(|a, b| b.1.all_tokens().cmp(&a.1.all_tokens()));

    let mut by_agent: Vec<(String, UsageTotal)> = c.by_agent.clone().into_iter().collect();
    for (a, v) in &u.by_agent {
        match by_agent.iter_mut().find(|(k, _)| k == a) {
            Some((_, e)) => e.add_assign(v),
            None => by_agent.push((a.clone(), v.clone())),
        }
    }
    by_agent.sort_by(|a, b| b.1.all_tokens().cmp(&a.1.all_tokens()));

    let mut sidechain = c.sidechain.clone();
    sidechain.add_assign(&u.sidechain);
    let mut main = c.main.clone();
    main.add_assign(&u.main);

    Ledger {
        cwd: cwd.to_string_lossy().into_owned(),
        tasks_total,
        tasks_by_status,
        tasks_blocked,
        tasks_without_deliverables,
        deliverables_total,
        deliverables_by_status,
        deliverables_without_task,
        evidence_verified,
        evidence_passed,
        evidence_failed,
        evidence_unverified,
        employees,
        by_agent,
        console_usage: c.matched,
        user_usage: u.matched,
        by_model,
        sidechain_usage: sidechain,
        main_usage: main,
        runs_total,
        runs_bad_lines,
        cost_usd_total,
        cost_by_agent,
        cost_by_model,
        cost_by_task,
        runs_failed,
        context_windows,
        console_usage_all_projects: if full { Some(c.all) } else { None },
        user_usage_all_projects: if full { Some(u.all) } else { None },
        console_all_session_files: c.all_session_files,
        console_all_subagent_files: c.all_subagent_files,
        user_all_session_files: u.all_session_files,
        user_all_subagent_files: u.all_subagent_files,
        full,
        // ★ 这条结论在第 14 刀被**修正**过。原文写的是"金额未计算，需要一张价格表" ——
        // 那对 **transcript** 是对的（里面确实没有成本字段），但漏了一条更好的路：
        // 结构化协议的 `result` 事件**自带 `total_cost_usd`**（本机实测确认）。
        // 过时的自我描述和过时的取值一样危险，所以这里连结论一起改。
        // 文案里刻意**不写 "$0"**：否则"没有运行记录时报告里不出现任何金额"这条
        // 不变量就没法用一条简单的断言钉住了。也刻意不带 markdown 星号 ——
        // 这段是要塞进对话框的纯文本。
        cost_note: "金额来自引擎自报：结构化运行结束时引擎给出 result.total_cost_usd\
（含它自己的缓存与分档计价），我们不乘价格表、也不估算。没有运行记录就是零。"
            .to_string(),
    }
}

fn real_roots() -> Vec<PathBuf> {
    vec![
        crate::providers::cc_config_dir().join("projects"),
        dirs_home().join(".claude").join("projects"),
    ]
}

/// 生产入口（**快路径**）：本项目 token 瞬间出，全库只数文件不合计。
pub fn scan(cwd: &Path) -> Ledger {
    scan_with_roots(cwd, &real_roots())
}

/// 生产入口（**慢路径**）：连全库合计一起算（本机实测 ~15 秒）。
pub fn scan_full(cwd: &Path) -> Ledger {
    scan_with_roots_opts(cwd, &real_roots(), true)
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// 账本**人看到的样子**（纯函数，便于单测）。
pub fn ledger_text(l: &Ledger) -> String {
    let mut s = String::new();
    s.push_str(&format!("账本 · {}\n", l.cwd));

    // ── 产出侧 ──
    s.push_str(&format!("\n── 任务（{} 条）──\n", l.tasks_total));
    if l.tasks_total == 0 {
        s.push_str("（还没有任务记录）\n");
    } else {
        let parts: Vec<String> = l
            .tasks_by_status
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        s.push_str(&format!("{}\n", parts.join(" ｜ ")));
        if l.tasks_blocked > 0 {
            s.push_str(&format!("被卡住：{} 条\n", l.tasks_blocked));
        }
        if l.tasks_without_deliverables > 0 {
            s.push_str(&format!(
                "⚠ 没有关联任何产出：{} 条（干了但没在板上留下东西）\n",
                l.tasks_without_deliverables
            ));
        }
    }

    s.push_str(&format!("\n── 产出（{} 条）──\n", l.deliverables_total));
    if l.deliverables_total == 0 {
        s.push_str("（还没有产出记录）\n");
    } else {
        let parts: Vec<String> = l
            .deliverables_by_status
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        s.push_str(&format!("{}\n", parts.join(" ｜ ")));
        s.push_str(&format!(
            "证据：验过 {} ｜ 全过 {} ｜ 有失败 {} ｜ **未验证 {}**\n",
            l.evidence_verified, l.evidence_passed, l.evidence_failed, l.evidence_unverified
        ));
        if l.deliverables_without_task > 0 {
            s.push_str(&format!(
                "⚠ 没挂到任何任务上：{} 条（不知道该算哪项工作）\n",
                l.deliverables_without_task
            ));
        }
    }

    // ── 按员工 ──
    s.push_str("\n── 按员工 ──\n");
    if l.employees.is_empty() {
        s.push_str("（还没有派出去的任务）\n");
    }
    for e in &l.employees {
        s.push_str(&format!(
            "@{}  任务 {}/{} 完成  产出 {}（验收 {}）  证据 过{}/败{}/未验{}\n",
            e.owner,
            e.tasks_done,
            e.tasks,
            e.deliverables,
            e.accepted,
            e.evidence_passed,
            e.evidence_failed,
            e.evidence_unverified
        ));
    }

    // ── 金额（引擎自报）──
    s.push_str("\n── 金额（引擎自报，不是我们估的）──\n");
    if l.runs_total == 0 {
        s.push_str("（还没有结构化运行记录 —— 金额来自运行时引擎给的 result.total_cost_usd）\n");
    } else {
        s.push_str(&format!(
            "运行 {} 次 ｜ 合计 ${:.6} ｜ 平均 ${:.6}\n",
            l.runs_total,
            l.cost_usd_total,
            l.cost_usd_total / l.runs_total as f64
        ));
        s.push_str("按员工：");
        let parts: Vec<String> = l
            .cost_by_agent
            .iter()
            .map(|(a, c, n)| format!("@{a} ${c:.6}（{n} 次）"))
            .collect();
        s.push_str(&parts.join(" ｜ "));
        s.push('\n');
        s.push_str("按模型：");
        let parts: Vec<String> = l
            .cost_by_model
            .iter()
            .map(|(m, c, n)| format!("{m} ${c:.6}（{n} 次）"))
            .collect();
        s.push_str(&parts.join(" ｜ "));
        s.push('\n');
        // 按任务：这才是"这条活花了多少钱"
        if l.runs_failed > 0 {
            s.push_str(&format!("其中失败 {} 次（失败通常不花钱，但要说出来）\n", l.runs_failed));
        }
        s.push_str("按任务：");
        let parts: Vec<String> = l
            .cost_by_task
            .iter()
            .map(|(t, c, n)| format!("{t} ${c:.6}（{n} 次）"))
            .collect();
        s.push_str(&parts.join(" ｜ "));
        s.push('\n');
        if l.runs_bad_lines > 0 {
            s.push_str(&format!("⚠ 运行记录里有 {} 行读不出来\n", l.runs_bad_lines));
        }
    }
    // 上下文窗口：引擎自报，比猜型号后缀准（目标 ② 的一项欠账）
    if !l.context_windows.is_empty() {
        s.push_str("引擎自报的上下文窗口：");
        let parts: Vec<String> = l
            .context_windows
            .iter()
            .map(|(m, w)| format!("{m} → {w}"))
            .collect();
        s.push_str(&parts.join(" ｜ "));
        s.push('\n');
    }

    // ── 机时侧 ──
    s.push_str("\n── 机时（token，本机 transcript 实测）──\n");
    let line = |label: &str, u: &UsageTotal| {
        format!(
            "{label}  会话 {} ｜ 消息 {} ｜ 输入 {} ｜ 输出 {} ｜ 缓存写 {} ｜ 缓存读 {}\n",
            u.sessions,
            u.messages,
            fmt_tokens(u.input),
            fmt_tokens(u.output),
            fmt_tokens(u.cache_create),
            fmt_tokens(u.cache_read)
        )
    };
    s.push_str(&line("调度台（隔离配置）", &l.console_usage));
    s.push_str(&line("你自己手敲的      ", &l.user_usage));
    // 全库对照：算了就报数，没算就**如实说没算**（并给出文件数），绝不糊一个假数
    match (&l.console_usage_all_projects, &l.user_usage_all_projects) {
        (Some(c), Some(u)) => {
            s.push_str(&format!(
                "  全库合计：调度台 {} ｜ 自己的 {}\n",
                fmt_tokens(c.all_tokens()),
                fmt_tokens(u.all_tokens())
            ));
        }
        _ => {
            s.push_str(&format!(
                "  全库合计**本次未算**（完整扫描约 15 秒，是显式动作）。\n     \
                 文件数：调度台 {} 会话 / {} 子代理 ｜ 自己的 {} 会话 / {} 子代理\n",
                l.console_all_session_files,
                l.console_all_subagent_files,
                l.user_all_session_files,
                l.user_all_subagent_files
            ));
        }
    }
    if !l.by_model.is_empty() {
        s.push_str("\n按模型：\n");
        for (m, u) in &l.by_model {
            s.push_str(&format!(
                "  {}  {} 条消息 ｜ 输入 {} ｜ 输出 {}\n",
                m,
                u.messages,
                fmt_tokens(u.input),
                fmt_tokens(u.output)
            ));
        }
    }
    if !l.by_agent.is_empty() {
        s.push_str("\n子代理按执行者：\n");
        for (a, u) in &l.by_agent {
            s.push_str(&format!(
                "  {}  {} 条消息 ｜ 输入 {} ｜ 输出 {}\n",
                a,
                u.messages,
                fmt_tokens(u.input),
                fmt_tokens(u.output)
            ));
        }
    }

    let sc = l.sidechain_usage.all_tokens();
    let mn = l.main_usage.all_tokens();
    if sc + mn > 0 {
        s.push_str(&format!(
            "\n子代理 vs 主对话：子代理 {} ｜ 主对话 {}（子代理占 {:.0}%）\n",
            fmt_tokens(sc),
            fmt_tokens(mn),
            100.0 * sc as f64 / (sc + mn) as f64
        ));
    }
    s.push_str(&format!("\n💰 {}\n", l.cost_note));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-ledger-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个 Claude Code 形态的 transcript（结构照抄实测）。
    fn write_transcript(root: &Path, project_dir: &str, session: &str, lines: &[&str]) {
        let dir = root.join(project_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
    }

    fn assistant(model: &str, input: u64, output: u64, cache_read: u64, side: bool) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":{},"message":{{"model":"{}","usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":0,"cache_read_input_tokens":{}}}}}}}"#,
            side, model, input, output, cache_read
        )
    }

    fn write_task(cwd: &Path, id: &str, body: &str) {
        let dir = cwd.join(".onecode").join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), body).unwrap();
    }

    fn write_deliverable(cwd: &Path, id: &str, status: Option<&str>) {
        let dir = cwd.join(".onecode").join("deliverables");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.json")),
            r#"{"title":"T","summary":"S","files":[],"evidence":[]}"#,
        )
        .unwrap();
        if let Some(s) = status {
            deliverables::set_status(cwd, id, s, None).unwrap();
        }
    }

    #[test]
    fn encode_matches_the_real_convention() {
        assert_eq!(
            encode_project_dir(Path::new("/Users/zhanglei/yycc/eng/onecode-desktop")),
            "-Users-zhanglei-yycc-eng-onecode-desktop"
        );
    }

    #[test]
    fn usage_is_summed_per_project_with_sidechain_split() {
        let cwd = tmp("usage");
        let root = tmp("root");
        let dir = encode_project_dir(&cwd);
        write_transcript(
            &root,
            &dir,
            "s1",
            &[
                &assistant("claude-x", 100, 10, 1000, false),
                &assistant("claude-x", 200, 20, 2000, false),
                &assistant("claude-y", 300, 30, 3000, true), // 子代理
                r#"{"type":"user","message":{"content":"hi"}}"#, // 非 assistant，不计数
            ],
        );

        let l = scan_with_roots(&cwd, &[root.clone()]);
        assert_eq!(l.console_usage.sessions, 1);
        assert_eq!(l.console_usage.messages, 3, "只数 assistant 且有 usage 的行");
        assert_eq!(l.console_usage.input, 600);
        assert_eq!(l.console_usage.output, 60);
        assert_eq!(l.console_usage.cache_read, 6000);
        assert_eq!(l.main_usage.input, 300, "主对话 = 前两条");
        assert_eq!(l.sidechain_usage.input, 300, "子代理 = 第三条");
        assert_eq!(l.by_model.len(), 2);
        // 按名字查，不依赖排序：两个模型的用量恰好相等时顺序是不确定的
        let x = l.by_model.iter().find(|(m, _)| m == "claude-x").expect("claude-x 应在");
        let y = l.by_model.iter().find(|(m, _)| m == "claude-y").expect("claude-y 应在");
        assert_eq!(x.1.input, 300, "claude-x 是主对话两条");
        assert_eq!(x.1.messages, 2);
        assert_eq!(y.1.input, 300, "claude-y 是子代理一条");
        assert_eq!(y.1.messages, 1);
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn other_projects_do_not_leak_into_this_project() {
        let cwd = tmp("isolate");
        let root = tmp("root2");
        write_transcript(&root, &encode_project_dir(&cwd), "s1", &[&assistant("m", 100, 1, 0, false)]);
        write_transcript(&root, "-some-other-project", "s2", &[&assistant("m", 999, 9, 0, false)]);

        // 本项目数字走快路径
        let l = scan_with_roots(&cwd, &[root.clone()]);
        assert_eq!(l.console_usage.input, 100, "本项目只算自己的");
        // 全库合计要显式请求（慢路径），且确实包含别人
        let lf = scan_with_roots_opts(&cwd, &[root.clone()], true);
        let all = lf.console_usage_all_projects.as_ref().expect("full 必须有全库合计");
        assert_eq!(all.input, 1099);
        assert_eq!(all.sessions, 2);
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn zero_usage_lines_do_not_inflate_message_counts() {
        let cwd = tmp("zero");
        let root = tmp("root3");
        let zero = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        write_transcript(&root, &encode_project_dir(&cwd), "s1", &[zero, &assistant("m", 5, 5, 0, false)]);
        let l = scan_with_roots(&cwd, &[root.clone()]);
        assert_eq!(l.console_usage.messages, 1, "全 0 的行不算一条用量");
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_roots_are_reported_as_zero_not_an_error() {
        let cwd = tmp("noroot");
        let l = scan_with_roots(&cwd, &[PathBuf::from("/definitely/not/here")]);
        assert_eq!(l.console_usage, UsageTotal::default());
        let text = ledger_text(&l);
        assert!(text.contains("账户") || text.contains("账本"), "空数据也要出报告");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 第 14 刀**修正**了这条结论：金额**能算**，而且不用价格表 ——
    /// 结构化协议的 `result` 事件自带 `total_cost_usd`（本机实测）。
    /// 旧断言钉的是错的结论，这里改成钉正确的那句。
    #[test]
    fn cost_note_says_money_comes_from_the_engine_not_a_price_table() {
        let cwd = tmp("costnote");
        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert!(l.cost_note.contains("引擎自报"), "{}", l.cost_note);
        assert!(!l.cost_note.contains('$'), "说明文案里不带金额字符：{}", l.cost_note);
        assert!(l.cost_note.contains("total_cost_usd"), "{}", l.cost_note);
        assert!(
            !l.cost_note.contains("需要一张价格表"),
            "旧结论（要价格表）已经过时，不该再出现：{}",
            l.cost_note
        );
        let text = ledger_text(&l);
        assert!(text.contains("还没有结构化运行记录"), "{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn runs_are_aggregated_into_real_money_by_agent_and_model() {
        let cwd = tmp("money");
        let mk = |agent: &str, model: &str, cost: f64| crate::engine_run::RunEntry {
            at: "2026-10-14T00:00:00Z".to_string(),
            agent: agent.to_string(),
            provider_id: "main".to_string(),
            model: model.to_string(),
            cost_usd: cost,
            input_tokens: 100,
            output_tokens: 5,
            duration_ms: 1000,
            num_turns: 1,
            ok: true,
            session_id: "s".to_string(),
            label: String::new(),
            context_window: Some(1_000_000),
            error: String::new(),
        };
        crate::engine_run::append_run(&cwd, &mk("dev", "deepseek-v4-flash[1m]", 0.10)).unwrap();
        crate::engine_run::append_run(&cwd, &mk("dev", "deepseek-v4-flash[1m]", 0.20)).unwrap();
        crate::engine_run::append_run(&cwd, &mk("writer", "glm-5.2", 0.05)).unwrap();

        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.runs_total, 3);
        assert!((l.cost_usd_total - 0.35).abs() < 1e-9, "{}", l.cost_usd_total);
        // 按员工：dev 两次共 0.30，排第一
        assert_eq!(l.cost_by_agent[0].0, "dev");
        assert!((l.cost_by_agent[0].1 - 0.30).abs() < 1e-9);
        assert_eq!(l.cost_by_agent[0].2, 2);
        assert_eq!(l.cost_by_agent[1].0, "writer");
        // 按模型
        assert_eq!(l.cost_by_model[0].0, "deepseek-v4-flash[1m]");
        assert!(l.cost_by_model.iter().any(|(m, _, _)| m == "glm-5.2"));
        // 引擎自报的窗口
        assert!(l.context_windows.iter().any(|(m, w)| m == "deepseek-v4-flash[1m]" && *w == 1_000_000));

        let text = ledger_text(&l);
        eprintln!("===== 账本（含真实金额）=====\n{text}\n===== end =====");
        assert!(text.contains("合计 $0.350000"), "{text}");
        assert!(text.contains("@dev $0.300000（2 次）"), "{text}");
        assert!(text.contains("deepseek-v4-flash[1m] $0.300000（2 次）"), "{text}");
        assert!(text.contains("引擎自报的上下文窗口"), "{text}");
        // 按任务：label 是任务 id 时，账本能答"这条活花了多少钱"
        assert!(text.contains("按任务："), "{text}");
        // label 为空 → 归到"（未挂任务）"（"manual" 是命令层给手工运行的默认标签）
        assert!(text.contains("(未挂任务) $0.350000"), "{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_run_without_an_agent_is_attributed_to_nobody_explicitly() {
        let cwd = tmp("noagent");
        let mut e = crate::engine_run::RunEntry {
            at: "t".to_string(),
            agent: String::new(),
            provider_id: String::new(),
            model: String::new(),
            cost_usd: 0.01,
            input_tokens: 1,
            output_tokens: 1,
            duration_ms: 1,
            num_turns: 1,
            ok: true,
            session_id: String::new(),
            label: String::new(),
            context_window: None,
            error: String::new(),
        };
        e.cost_usd = 0.01;
        crate::engine_run::append_run(&cwd, &e).unwrap();
        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.cost_by_agent[0].0, "(未标注员工)");
        assert_eq!(l.cost_by_model[0].0, "(未标注模型)");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn outcome_side_counts_tasks_deliverables_and_gaps() {
        let cwd = tmp("outcome");
        write_deliverable(&cwd, "d1", Some("accepted"));
        write_deliverable(&cwd, "d2", None); // 没有挂到任务
        write_task(&cwd, "t1", r#"{"title":"A","owner":"dev","status":"review","deliverables":["d1"]}"#);
        write_task(&cwd, "t2", r#"{"title":"B","owner":"dev"}"#); // 没有产出
        deliverables::verify_evidence(&cwd, "d1", std::time::Duration::from_secs(10)).ok();

        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.tasks_total, 2);
        assert_eq!(l.deliverables_total, 2);
        assert_eq!(l.tasks_without_deliverables, 1, "t2 没有关联产出");
        assert_eq!(l.deliverables_without_task, 1, "d2 没挂到任务上");

        let dev = l.employees.iter().find(|e| e.owner == "dev").expect("dev 应有一行");
        assert_eq!(dev.tasks, 2);
        assert_eq!(dev.deliverables, 1);
        assert_eq!(dev.accepted, 1);
        let text = ledger_text(&l);
        assert!(text.contains("@dev"));
        assert!(text.contains("干了但没在板上留下东西"), "{text}");
        assert!(text.contains("不知道该算哪项工作"), "{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn employee_rows_include_unassigned_tasks() {
        let cwd = tmp("unassigned");
        write_task(&cwd, "t1", r#"{"title":"没人认领"}"#);
        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.employees.len(), 1);
        assert_eq!(l.employees[0].owner, "(未指派)");
        assert_eq!(l.employees[0].tasks_open, 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn human_completed_tasks_count_as_done() {
        let cwd = tmp("done");
        write_task(&cwd, "t1", r#"{"title":"A","owner":"dev"}"#);
        tasks::set_human_status(&cwd, "t1", "done", None).unwrap();
        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.tasks_by_status.iter().find(|(k, _)| k == "done").map(|(_, v)| *v), Some(1));
        assert_eq!(l.employees[0].tasks_done, 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 金额现在是真的（引擎自报），所以不变量换成更强的那条：
    /// **没有运行记录时，报告里不许出现任何金额**；有记录时，金额必须等于记录之和。
    /// （旧版钉的是"绝不出现 $"，那是"金额算不出来"时代的纪律；现在金额能算了，
    /// 纪律要升级成"只在真有记录时才显示，且必须可核对"。）
    #[test]
    fn amounts_only_appear_when_there_are_real_run_records() {
        let cwd = tmp("noamount");
        write_task(&cwd, "t1", r#"{"title":"A","owner":"dev"}"#);
        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        let text = ledger_text(&l);
        assert!(
            !text.contains('$'),
            "没有运行记录就不该出现任何金额（不许估、不许编）：\n{text}"
        );
        assert!(text.contains("还没有结构化运行记录"), "{text}");

        // 有记录时：显示的合计必须等于记录之和
        let e = crate::engine_run::RunEntry {
            at: "t".to_string(),
            agent: "dev".to_string(),
            provider_id: "main".to_string(),
            model: "m".to_string(),
            cost_usd: 0.004321,
            input_tokens: 1,
            output_tokens: 1,
            duration_ms: 1,
            num_turns: 1,
            ok: true,
            session_id: String::new(),
            label: String::new(),
            context_window: None,
            error: String::new(),
        };
        crate::engine_run::append_run(&cwd, &e).unwrap();
        let l2 = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert!((l2.cost_usd_total - 0.004321).abs() < 1e-12);
        assert!(ledger_text(&l2).contains("$0.004321"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// **真实数据**：对本仓跑一遍账本，确认它真能读出本机 transcript 的用量
    /// 与真实的任务/产出记录。无数据时跳过（CI 上不适用）。
    #[test]
    fn real_repo_ledger_produces_real_numbers() {
        let Some(repo) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() else { return };
        // 守卫要检查**仓根真实存在**的东西（Cargo.toml 在 src-tauri/ 里，不在仓根）
        if !repo.join("package.json").exists() {
            return;
        }
        // 快路径（UI 默认）：必须瞬间出
        let t0 = std::time::Instant::now();
        let fast = scan(repo);
        let fast_ms = t0.elapsed().as_millis();
        eprintln!("===== 真实账本（快路径）=====\n{}\n===== end =====", ledger_text(&fast));
        eprintln!("[ledger] 快路径耗时 {fast_ms}ms");

        let total_sessions = fast.user_usage.sessions + fast.console_usage.sessions;
        if total_sessions == 0 && fast.user_all_session_files == 0 {
            return; // 本机没有 transcript → 跳过，不假装通过
        }
        assert!(total_sessions > 0, "本项目应能读到 transcript（已实测存在）");
        assert!(
            fast.user_usage.messages + fast.console_usage.messages > 0,
            "读到了 session 却没有任何 assistant 消息，说明解析结构错了"
        );
        // 快路径必须真的快（文件 I/O 是瓶颈，别退化成全库扫描）
        assert!(fast_ms < 3000, "快路径太慢：{fast_ms}ms —— 是不是把非本项目也读了？");

        // 慢路径（显式）：全库合计，实测约 15 秒 —— 因此**默认不跑**，
        // 免得每次 cargo test 都被拖慢。要验它：ONECODE_TEST_LEDGER_FULL=1 cargo test ledger::
        if std::env::var("ONECODE_TEST_LEDGER_FULL").is_err() {
            eprintln!("[ledger] 跳过全库扫描（设 ONECODE_TEST_LEDGER_FULL=1 可验证）");
            return;
        }
        let full = scan_full(repo);
        assert!(full.console_usage_all_projects.is_some());
        assert!(full.user_usage_all_projects.is_some());
        eprintln!(
            "[ledger] 全库合计：调度台 {} ｜ 自己的 {}",
            full.console_usage_all_projects.as_ref().unwrap().all_tokens(),
            full.user_usage_all_projects.as_ref().unwrap().all_tokens()
        );
        // 金额绝不能出现
        assert!(fast.cost_note.contains("没有成本字段"));
        // 快路径：全库合计必须是 None（没算就别说算了）
        assert!(fast.console_usage_all_projects.is_none());
        assert!(!fast.full);
    }

    /// **子代理的用量不在主转录里** —— 它在 `<sessionId>/subagents/agent-*.jsonl`。
    /// 本机实测：主转录里 `isSidechain:true` 出现 0 次，而 subagents 下有 906 个文件。
    /// 所以"不下钻"不是漏一点，而是**结构性看不见**。
    #[test]
    fn subagent_transcripts_are_read_and_attributed() {
        let cwd = tmp("subagent");
        let root = tmp("rootsub");
        let proj = root.join(encode_project_dir(&cwd));
        std::fs::create_dir_all(&proj).unwrap();
        // 主转录
        std::fs::write(
            proj.join("sess1.jsonl"),
            assistant("main-model", 100, 10, 0, false),
        )
        .unwrap();
        // 子代理转录（独立目录、独立文件）
        let sub = proj.join("sess1").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("agent-abc.jsonl"),
            format!(
                r#"{{"type":"assistant","isSidechain":true,"attributionAgent":"researcher","message":{{"model":"sub-model","usage":{{"input_tokens":500,"output_tokens":50}}}}}}"#
            ),
        )
        .unwrap();

        let l = scan_with_roots(&cwd, &[root.clone()]);
        assert_eq!(l.main_usage.input, 100, "主对话");
        assert_eq!(l.sidechain_usage.input, 500, "子代理必须被读到（否则结构性看不见）");
        assert_eq!(l.console_usage.input, 600, "合计要含子代理");
        assert_eq!(l.console_usage.sessions, 1, "子代理文件不算独立会话");
        let who = l.by_agent.iter().find(|(a, _)| a == "researcher").expect("要能归属到执行者");
        assert_eq!(who.1.input, 500);
        assert_eq!(who.1.messages, 1);
        let text = ledger_text(&l);
        assert!(text.contains("子代理按执行者"), "{text}");
        assert!(text.contains("researcher"));
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有 `attributionAgent` 时退回 `agentId`，都没有就明确标"未标注执行者"。
    #[test]
    fn subagent_attribution_falls_back_and_never_invents_a_name() {
        let cwd = tmp("attrib");
        let root = tmp("rootattrib");
        let sub = root
            .join(encode_project_dir(&cwd))
            .join("sess1")
            .join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("agent-x.jsonl"),
            r#"{"type":"assistant","isSidechain":true,"agentId":"worker-7","message":{"model":"m","usage":{"input_tokens":9,"output_tokens":1}}}"#,
        )
        .unwrap();
        let l = scan_with_roots(&cwd, &[root.clone()]);
        assert!(l.by_agent.iter().any(|(a, _)| a == "worker-7"), "退回 agentId");

        // 两个都没有
        let cwd2 = tmp("attrib2");
        let root2 = tmp("rootattrib2");
        let sub2 = root2.join(encode_project_dir(&cwd2)).join("sess1").join("subagents");
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(
            sub2.join("agent-y.jsonl"),
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"m","usage":{"input_tokens":9,"output_tokens":1}}}"#,
        )
        .unwrap();
        let l2 = scan_with_roots(&cwd2, &[root2.clone()]);
        assert!(
            l2.by_agent.iter().any(|(a, _)| a == "(未标注执行者)"),
            "不能编一个执行者名字出来：{:?}",
            l2.by_agent
        );
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&cwd2);
        let _ = std::fs::remove_dir_all(&root2);
    }

    /// 快路径必须**不读**非本项目的内容，慢路径才算全库 —— 且两者都如实标注。
    #[test]
    fn fast_path_skips_other_projects_and_says_so() {
        let cwd = tmp("fastpath");
        let root = tmp("rootfast");
        write_transcript(&root, &encode_project_dir(&cwd), "s1", &[&assistant("m", 100, 1, 0, false)]);
        write_transcript(&root, "-other-project", "s2", &[&assistant("m", 999, 9, 0, false)]);

        let fast = scan_with_roots_opts(&cwd, &[root.clone()], false);
        assert_eq!(fast.console_usage.input, 100);
        assert!(fast.console_usage_all_projects.is_none(), "快路径不该算全库");
        assert_eq!(fast.console_all_session_files, 2, "但文件数要数准（便宜且永远准确）");
        let t = ledger_text(&fast);
        assert!(t.contains("未算"), "没算就要说没算：{t}");

        let full = scan_with_roots_opts(&cwd, &[root.clone()], true);
        assert_eq!(full.console_usage_all_projects.as_ref().unwrap().input, 1099);
        assert!(full.full);
        assert!(ledger_text(&full).contains("全库合计"));
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 「这条活花了多少钱」：运行记录带任务 id 时，账本按任务归集。
    #[test]
    fn cost_is_aggregated_per_task_not_just_per_run() {
        let cwd = tmp("pertask");
        let mk = |label: &str, agent: &str, cost: f64| crate::engine_run::RunEntry {
            at: "2026-10-14T00:00:00Z".to_string(),
            agent: agent.to_string(),
            provider_id: "main".to_string(),
            model: "m".to_string(),
            cost_usd: cost,
            input_tokens: 100,
            output_tokens: 5,
            duration_ms: 100,
            num_turns: 1,
            ok: true,
            session_id: String::new(),
            label: label.to_string(),
            context_window: None,
            error: String::new(),
        };
        // 同一条任务跑了两次；另一条任务跑了一次
        crate::engine_run::append_run(&cwd, &mk("write-draft", "dev", 0.10)).unwrap();
        crate::engine_run::append_run(&cwd, &mk("write-draft", "dev", 0.20)).unwrap();
        crate::engine_run::append_run(&cwd, &mk("fix-bug", "dev", 0.05)).unwrap();

        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.cost_by_task.len(), 2);
        assert_eq!(l.cost_by_task[0].0, "write-draft");
        assert!((l.cost_by_task[0].1 - 0.30).abs() < 1e-9);
        assert_eq!(l.cost_by_task[0].2, 2);
        assert_eq!(l.cost_by_task[1].0, "fix-bug");
        assert!((l.cost_usd_total - 0.35).abs() < 1e-9, "总额仍按次累加");

        let text = ledger_text(&l);
        eprintln!("===== 账本（按任务）=====\n{text}\n===== end =====");
        assert!(text.contains("write-draft $0.300000（2 次）"), "{text}");
        assert!(text.contains("fix-bug $0.050000（1 次）"), "{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// ★ 失败也要在账本上留痕 —— 只看金额会把"跑三次都失败"看成"什么都没发生"。
    #[test]
    fn a_failed_run_is_counted_even_though_it_cost_nothing() {
        let cwd = tmp("failed");
        let mk = |ok: bool, cost: f64, err: &str| crate::engine_run::RunEntry {
            at: "2026-10-14T00:00:00Z".to_string(),
            agent: "dev".to_string(),
            provider_id: "prov-a".to_string(),
            model: "m".to_string(),
            cost_usd: cost,
            input_tokens: 0,
            output_tokens: 0,
            duration_ms: 10,
            num_turns: 0,
            ok,
            session_id: String::new(),
            label: "slot:1".to_string(),
            context_window: None,
            error: err.to_string(),
        };
        crate::engine_run::append_run(&cwd, &mk(false, 0.0, "引擎结束了但没有 result 事件")).unwrap();
        crate::engine_run::append_run(&cwd, &mk(false, 0.0, "引擎结束了但没有 result 事件")).unwrap();
        crate::engine_run::append_run(&cwd, &mk(true, 0.05, "")).unwrap();

        let l = scan_with_roots(&cwd, &[PathBuf::from("/nope")]);
        assert_eq!(l.runs_total, 3, "三次都算运行");
        assert_eq!(l.runs_failed, 2, "两次失败要计数");
        assert!((l.cost_usd_total - 0.05).abs() < 1e-9, "金额只按真实的算");
        let text = ledger_text(&l);
        assert!(text.contains("失败 2 次"), "要说出来：{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
