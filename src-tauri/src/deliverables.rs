//! 产出与证据化验收（目标 ④）。
//!
//! 核心原则（产品规格 §4.1）：
//! **产出 = 真文件 + 证据 + 验收状态**，而且三者都不能由模型自述替代：
//!
//! - **真文件**：存在性与大小由我们 `stat` —— 模型说"我生成了 out.html"不作数；
//! - **证据**：按类型**分级**（能不能机器验证），并把"模型自述"明确判为**不是证据**；
//! - **验收状态**：只有人能改，落在 `<cwd>/.onecode/deliverables/<id>.status.json`。
//!
//! 为什么要分级而不是"跑一遍命令看绿不绿"：自动执行模型写下的任意命令是一条
//! 提权路径。本版**不自动执行**，只如实标注"这条我们可以机器验证"（grade =
//! MachineRunnable），把执行留给**人在审查时显式触发**。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// git 单条改动。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GitChange {
    pub path: String,
    /// `M` / `A` / `D` / `??` 等（取自 porcelain）
    pub status: String,
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    pub binary: bool,
}

/// 工作区快照 —— **由我们算，模型无法伪造**。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GitSnapshot {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub changes: Vec<GitChange>,
    /// git 不可用 / 超时 / 非仓库时的说明（不静默）
    pub error: Option<String>,
}

impl Default for GitSnapshot {
    fn default() -> Self {
        Self { is_repo: false, branch: None, changes: Vec::new(), error: None }
    }
}

/// 证据的可信度分级。**"模型自述"必须落到 `NotEvidence`** —— 这条是产品规格里的硬规矩
/// （ACE 实测：本地判官假通过率 100%）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceGrade {
    /// 我们**能**机器验证（本版不自动跑，需人在审查时触发）
    MachineRunnable,
    /// 我们能自己量出来并与声明交叉核对（git 改动、文件大小）
    Measured,
    /// 我们核不了（外部事实：发了个请求、发布成功）
    Unverifiable,
    /// 不是证据
    NotEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EvidenceFact {
    /// 原始 kind（exec / artifact / diff / external / self-report / …）
    pub kind: String,
    pub grade: EvidenceGrade,
    /// 人可读的说明（含我们实际核对到了什么）
    pub detail: String,
    pub command: Option<String>,
}

/// 一条命令的**实测**结果（我们跑的，不是模型声称的）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommandResult {
    pub command: String,
    /// `None` = 超时被杀，没有退出码
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// 合并后的输出尾部（有上限，超了会带截断标记）
    pub output_tail: String,
}

/// 一次验证的汇总。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    pub at: String,
    pub results: Vec<CommandResult>,
}

impl Verification {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| !r.timed_out && r.exit_code == Some(0)).count()
    }
    pub fn failed(&self) -> usize {
        self.results.len() - self.passed()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileFact {
    pub path: String,
    pub exists: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Deliverable {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// 声明的文件 —— 存在性与大小**由我们核对**
    pub files: Vec<FileFact>,
    pub evidence: Vec<EvidenceFact>,
    /// `draft` | `pending` | `accepted` | `rejected`
    pub status: String,
    pub status_note: Option<String>,
    /// 记录本身的问题（JSON 坏了 / 缺字段 / 声明的文件不存在）。
    /// **不静默**：坏记录照样出现在列表里，人才能发现。
    pub problem: Option<String>,
    /// 我们**实测**过的证据结果（人触发验证后写入）。None = 还没验过 ——
    /// "声明了证据"与"证据通过了"必须是两件可区分的事。
    pub verification: Option<Verification>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeliverableScan {
    pub cwd: String,
    pub git: GitSnapshot,
    pub deliverables: Vec<Deliverable>,
    /// 目录里读不出来的记录数（如实计数）
    pub unreadable: usize,
}

/// 产出记录的约定目录（相对 cwd）。
pub const DIR: &str = ".onecode/deliverables";

/// 注入给员工的**产出协议**正文。
///
/// 只此一份实现：`agents::compose_system_prompt_with_skills` 注入的就是它，
/// 文档里写的也是它 —— 避免"协议说明与实现各写一份"。
pub fn protocol_section() -> String {
    format!(
        r#"## 产出协议（完成一项工作时必须做）

把这次交付**写成一份产出记录**，路径 `<cwd>/{DIR}/<id>.json`（`<id>` 用简短英文/数字，如 `2026-10-14-wechat-draft`）：

```json
{{
  "title": "一句标题",
  "summary": "一句话说明你做了什么、给谁用",
  "files": ["相对或绝对路径，必须是真实存在的文件"],
  "evidence": [
    {{"kind": "exec", "command": "node tools/check.js out.html", "note": "可选，本机已跑过"}},
    {{"kind": "diff", "note": "改了哪几个文件"}},
    {{"kind": "external", "note": "外部事实，如 HTTP 200"}}
  ]
}}
```

规矩：

- **`files` 里只能写真实存在的文件** —— 我们会去 `stat`，写不存在的文件会被标出来，等于自己打脸。
- **证据要可执行或可测量**。`exec` 是唯一我们能机器验证的类型（所以尽量给命令）；
  `diff`/`artifact` 我们能自己量；`external` 我们核不了，只能如实转述。
- **不要用"我认为完成了"当证据** —— 自述在我们这里不算证据，会被标成 `not-evidence`。
- 不写记录 = 这次工作没有产出记录，人会看不到成果。
"#
    )
}

/// 记录 id 白名单：不接受路径分隔符与 `..`（id 会被拼进路径）。
pub fn is_safe_record_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

// ── git ─────────────────────────────────────────────────────────────

/// 跑一个命令并限时。返回 `(exit_ok, stdout)`；超时会杀掉子进程。
///
/// 与 `cc_status::run_bash_with_timeout` 的区别：这里**走 argv、不走 shell**
/// （git 参数我们自己拼，不需要 shell 解析；顺便免掉一层注入面），
/// 而且需要**退出码**而不只是 stdout，所以没有复用那个函数。
fn run_argv(cmd: &str, args: &[&str], cwd: &Path, timeout: Duration) -> (bool, String) {
    let mut child = match Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("spawn failed: {e}")),
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return (false, format!("timeout after {:?}", timeout));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return (false, format!("wait failed: {e}")),
        }
    }
    match child.wait_with_output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            if out.status.success() {
                (true, stdout)
            } else {
                // 失败时**必须带上 stderr**：git 的报错只写在 stderr，只留 stdout
                // 就会得到一句空说明（"不是 git 仓库："）—— 那等于静默，
                // 与本模块"任何异常都要能显示出来"的原则相反。
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let detail = if err.is_empty() { stdout.trim().to_string() } else { err };
                (
                    false,
                    if detail.is_empty() {
                        format!("{cmd} 退出码 {:?}", out.status.code())
                    } else {
                        detail
                    },
                )
            }
        }
        Err(e) => (false, format!("output failed: {e}")),
    }
}

/// 输出尾部上限：够人看出失败原因，又不会把对话框撑爆。
const OUTPUT_TAIL_BYTES: usize = 2000;

/// 超时杀完之后，最多再等多久收输出。取值很小是刻意的：这里等的是"管道关闭"，
/// 不是"命令跑完"。有孙代赖着不走时宁可少收点输出，也不能把验证卡住。
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(400);

/// 用 shell 跑一条**人已确认过**的命令，限时，合并 stdout/stderr。
///
/// 为什么这里用 shell 而 git 那边用 argv：证据命令可能是 `a | b`、`node x.js && …`，
/// 只有 shell 能表达。因此调用方**必须先让人确认**（见 `deliverable_verify` 命令的
/// 对话框）—— 这也是本模块**不提供"自动验证"**的原因：自动执行模型写下的命令，
/// 等于让模型决定我们在本机跑什么。
///
/// 超时清理复用 `pty::kill_process_tree`：只杀直接子进程会留下孙代（被 launchd
/// 收养继续跑），那是这个仓已经踩过的坑。
fn run_shell(command: &str, cwd: &Path, timeout: Duration) -> CommandResult {
    let started = Instant::now();
    let mut child = match Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                command: command.to_string(),
                exit_code: None,
                timed_out: false,
                duration_ms: 0,
                output_tail: format!("无法启动 bash：{e}"),
            }
        }
    };
    let pid = child.id();

    // ★ 输出必须用**独立读取线程**收，不能用 `wait_with_output()`：
    // 后者会一直读到管道 EOF，而**孙进程还握着管道写端** —— 于是"超时杀掉了
    // 直接子进程"之后它仍会阻塞到孙代自己退出（实测：600ms 的超时等了 31.7s）。
    // 换句话说：用 wait_with_output 的超时**不是真的超时**。
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let spawn_reader = |pipe: Option<Box<dyn Read + Send>>, sink: Arc<Mutex<Vec<u8>>>| {
        std::thread::spawn(move || {
            if let Some(mut p) = pipe {
                let mut tmp = [0u8; 8192];
                while let Ok(n) = p.read(&mut tmp) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut b) = sink.lock() {
                        b.extend_from_slice(&tmp[..n]);
                        if b.len() > 64 * 1024 {
                            break; // 上限保护：别让一条爱刷日志的命令吃掉内存
                        }
                    }
                }
            }
        })
    };
    let t_out = spawn_reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), out_buf.clone());
    let t_err = spawn_reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), err_buf.clone());

    let mut timed_out = false;
    let mut exit_code = None;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                exit_code = st.code();
                break;
            }
            Ok(None) => {
                if started.elapsed() > timeout {
                    timed_out = true;
                    let _ = child.kill();
                    // 整棵树清扫：否则 shell 死了、孙代活着（还会握着管道）
                    crate::pty::kill_process_tree(pid);
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                exit_code = None;
                let _ = e;
                break;
            }
        }
    }

    // 收输出：**有界等待**。若还有孙代握着管道（例如 ps 不可用导致树清扫降级），
    // 我们最多等 OUTPUT_DRAIN_GRACE，然后带着已收到的内容返回 —— 绝不为了一条
    // 赖着不退的命令把整个验证卡住。
    // 有界排空：**唯一实现**在 `crate::proc`（这里曾因 join() 假超时 31.7 秒）
    let drained = crate::proc::drain_bounded(&[&t_out, &t_err], OUTPUT_DRAIN_GRACE);
    let mut out = String::from_utf8_lossy(&out_buf.lock().map(|b| b.clone()).unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_buf.lock().map(|b| b.clone()).unwrap_or_default()).into_owned();
    if !err.trim().is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&err);
    }
    out.push_str(crate::proc::drain_note(drained));

    if out.len() > OUTPUT_TAIL_BYTES {
        let cut = out.len() - OUTPUT_TAIL_BYTES;
        // 从字符边界切，避免切坏 UTF-8
        let mut start = cut;
        while start < out.len() && !out.is_char_boundary(start) {
            start += 1;
        }
        out = format!("…（前面 {cut} 字节省略）\n{}", &out[start..]);
    }

    CommandResult {
        command: command.to_string(),
        exit_code,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        output_tail: out.trim_end().to_string(),
    }
}

/// 挑一份要处理的产出：指定 `id` 就找它；否则取第一份**待处理**的
/// （已接受 / 已驳回的跳过）。
///
/// 只此一处实现 —— 审查与验证两个入口必须挑到**同一份**，
/// 否则"人看的是 A、跑的是 B"。
pub fn pick_target<'a>(scan: &'a DeliverableScan, id: Option<&str>) -> Option<&'a Deliverable> {
    match id {
        Some(want) => scan.deliverables.iter().find(|d| d.id == want),
        None => scan
            .deliverables
            .iter()
            .find(|d| d.status != "accepted" && d.status != "rejected")
            .or_else(|| scan.deliverables.first()),
    }
}

/// 这份产出声明的**可执行**证据命令（`kind: exec` 且 `command` 非空，去重保序）。
///
/// "给人看的清单"与"实际跑的清单"必须是同一份 —— 所以命令层的确认对话框也走这里。
pub fn exec_commands(cwd: &Path, id: &str) -> Result<Vec<String>, String> {
    if !is_safe_record_id(id) {
        return Err(format!("非法记录 id：{id:?}"));
    }
    let path = cwd.join(DIR).join(format!("{id}.json"));
    let raw = read_text(&path).ok_or_else(|| format!("找不到产出记录：{}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("记录解析失败：{e}"))?;
    let mut commands: Vec<String> = Vec::new();
    if let Some(serde_json::Value::Array(items)) = value.get("evidence") {
        for e in items {
            if e.get("kind").and_then(|v| v.as_str()) != Some("exec") {
                continue;
            }
            if let Some(c) = e.get("command").and_then(|v| v.as_str()) {
                let c = c.trim().to_string();
                if !c.is_empty() && !commands.contains(&c) {
                    commands.push(c);
                }
            }
        }
    }
    Ok(commands)
}

/// **跑一遍这份产出声明的可执行证据**，把实测结果写回 sidecar。
///
/// 只处理 `kind == "exec"` 且带 `command` 的条目。没有可跑的条目就返回 Err ——
/// 如实告诉调用方"这份产出没有可验证的证据"，而不是给一个空报告冒充成功。
pub fn verify_evidence(
    cwd: &Path,
    id: &str,
    per_command_timeout: Duration,
) -> Result<Verification, String> {
    // "哪些命令可跑"只有一处实现：命令层的确认对话框也用 exec_commands，
    // 免得"给人看的清单"和"实际跑的清单"漂移（那是最危险的一种不一致）。
    let commands = exec_commands(cwd, id)?;
    if commands.is_empty() {
        return Err("这份产出没有声明可执行的证据（kind: exec + command）".to_string());
    }

    let mut results = Vec::new();
    for c in &commands {
        log::info!("[deliverable] verify {id}: $ {c}");
        results.push(run_shell(c, cwd, per_command_timeout));
    }
    let verification = Verification { at: chrono::Utc::now().to_rfc3339(), results };

    update_sidecar(
        cwd,
        id,
        serde_json::json!({ "verification": serde_json::to_value(&verification).map_err(|e| e.to_string())? }),
    )?;
    Ok(verification)
}

/// 采集工作区快照（**我们算的**，不采信模型自述）。
pub fn git_snapshot(cwd: &Path) -> GitSnapshot {
    let mut snap = GitSnapshot::default();
    let timeout = Duration::from_secs(10);

    let (ok, out) = run_argv("git", &["rev-parse", "--is-inside-work-tree"], cwd, timeout);
    if !ok || out.trim() != "true" {
        snap.error = Some(if ok { "不在 git 仓库内".to_string() } else { out.trim().to_string() });
        return snap;
    }
    snap.is_repo = true;

    let (_, branch) = run_argv("git", &["rev-parse", "--abbrev-ref", "HEAD"], cwd, timeout);
    let branch = branch.trim();
    if !branch.is_empty() && branch != "HEAD" {
        snap.branch = Some(branch.to_string());
    }

    // porcelain 拿状态；numstat 拿增删行数（两者分开，porcelain 的格式更稳）
    let (_, porcelain) = run_argv("git", &["status", "--porcelain"], cwd, timeout);
    let mut changes: Vec<GitChange> = Vec::new();
    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let path = line[3..].trim().to_string();
        // rename 形态 "old -> new"：取箭头右边
        let path = path.rsplit(" -> ").next().unwrap_or(&path).to_string();
        changes.push(GitChange { path, status, added: None, deleted: None, binary: false });
    }

    let (_, numstat) = run_argv("git", &["diff", "--numstat", "HEAD"], cwd, timeout);
    for line in numstat.lines() {
        let mut it = line.split('\t');
        let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else { continue };
        if let Some(c) = changes.iter_mut().find(|c| c.path == p) {
            if a == "-" || d == "-" {
                c.binary = true;
            } else {
                c.added = a.parse().ok();
                c.deleted = d.parse().ok();
            }
        }
    }
    snap.changes = changes;
    snap
}

// ── 记录读取 ────────────────────────────────────────────────────────

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// 按 kind 分级。**未知 kind 一律 NotEvidence**（宁可保守，不可把自述当证据）。
fn grade_of(kind: &str) -> EvidenceGrade {
    match kind {
        "exec" => EvidenceGrade::MachineRunnable,
        "diff" | "artifact" => EvidenceGrade::Measured,
        "external" => EvidenceGrade::Unverifiable,
        _ => EvidenceGrade::NotEvidence,
    }
}

/// 把一份声明记录（JSON）解析成 [`Deliverable`]，并用真实文件系统校正。
///
/// 解析失败**不返回 None**：返回一个带 `problem` 的条目 —— 坏记录必须可见。
fn parse_record(id: &str, raw: &str, cwd: &Path, sidecar: &serde_json::Value) -> Deliverable {
    let status_str = sidecar
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("draft")
        .to_string();
    let status_note = sidecar
        .get("note")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let verification = sidecar
        .get("verification")
        .and_then(|v| serde_json::from_value::<Verification>(v.clone()).ok());
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Deliverable {
                id: id.to_string(),
                title: id.to_string(),
                summary: String::new(),
                files: Vec::new(),
                evidence: Vec::new(),
                status: status_str,
                status_note,
                problem: Some(format!("JSON 解析失败：{e}")),
                verification: None,
            }
        }
    };

    let get_str = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut problem: Option<String> = None;

    // 文件：存在性与大小由我们核对
    let mut files: Vec<FileFact> = Vec::new();
    match value.get("files") {
        Some(serde_json::Value::Array(arr)) => {
            for f in arr {
                let Some(p) = f.as_str() else { continue };
                let abs = if Path::new(p).is_absolute() { PathBuf::from(p) } else { cwd.join(p) };
                match std::fs::metadata(&abs) {
                    Ok(md) if md.is_file() => {
                        files.push(FileFact { path: p.to_string(), exists: true, bytes: Some(md.len()) })
                    }
                    _ => {
                        files.push(FileFact { path: p.to_string(), exists: false, bytes: None });
                        problem = Some(match problem.take() {
                            Some(prev) => format!("{prev}；声明的文件不存在：{p}"),
                            None => format!("声明的文件不存在：{p}"),
                        });
                    }
                }
            }
        }
        None => {
            problem = Some("缺少 files 字段（产出必须能指出真实文件）".to_string());
        }
        Some(_) => {
            problem = Some("files 必须是字符串数组".to_string());
        }
    }

    // 证据：分级 + 交叉核对
    let mut evidence: Vec<EvidenceFact> = Vec::new();
    if let Some(serde_json::Value::Array(arr)) = value.get("evidence") {
        for e in arr {
            let kind = e.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let grade = grade_of(&kind);
            let command = e.get("command").and_then(|v| v.as_str()).map(|s| s.to_string());
            let note = e.get("note").and_then(|v| v.as_str()).unwrap_or("");
            let detail = match grade {
                EvidenceGrade::MachineRunnable => format!(
                    "可机器验证（用「验证证据」跑一遍；不会自动执行）{}",
                    if note.is_empty() { String::new() } else { format!("：{note}") }
                ),
                EvidenceGrade::Measured => {
                    format!("可测量（我们会自己核对）{}", if note.is_empty() { String::new() } else { format!("：{note}") })
                }
                EvidenceGrade::Unverifiable => {
                    format!("我们核不了（外部事实）{}", if note.is_empty() { String::new() } else { format!("：{note}") })
                }
                EvidenceGrade::NotEvidence => format!(
                    "不是证据（自述/未知类型）{}",
                    if note.is_empty() { String::new() } else { format!("：{note}") }
                ),
            };
            evidence.push(EvidenceFact { kind, grade, detail, command });
        }
    }

    if get_str("summary").is_empty() && problem.is_none() {
        problem = Some("缺少 summary".to_string());
    }

    Deliverable {
        id: id.to_string(),
        title: {
            let t = get_str("title");
            if t.is_empty() { id.to_string() } else { t }
        },
        summary: get_str("summary"),
        files,
        evidence,
        status: status_str,
        status_note,
        problem,
        verification,
    }
}

/// 读 sidecar（不解析成具体结构 —— 里面同时住着**验收状态**与**验证结果**，
/// 用整体 JSON 读写才不会互相覆盖）。
fn read_sidecar(cwd: &Path, id: &str) -> serde_json::Value {
    let path = cwd.join(DIR).join(format!("{id}.status.json"));
    read_text(&path)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// **读-改-写** sidecar：只动 `patch` 里的键，其余键原样保留。
///
/// 这是唯一的写入口。分开整体重写会让后写的抹掉先写的 —— 验收状态与验证结果
/// 互相覆盖，而且不报错。
fn update_sidecar(cwd: &Path, id: &str, patch: serde_json::Value) -> Result<PathBuf, String> {
    if !is_safe_record_id(id) {
        return Err(format!("非法记录 id：{id:?}"));
    }
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let mut cur = read_sidecar(cwd, id);
    if !cur.is_object() {
        cur = serde_json::json!({});
    }
    let (Some(obj), Some(patch_obj)) = (cur.as_object_mut(), patch.as_object()) else {
        return Err("sidecar 必须是 JSON 对象".to_string());
    };
    for (k, v) in patch_obj {
        obj.insert(k.clone(), v.clone());
    }
    let path = dir.join(format!("{id}.status.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&cur).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入失败：{e}"))?;
    Ok(path)
}

/// 扫描一个工作区的全部产出记录。
pub fn scan(cwd: &Path) -> DeliverableScan {
    let dir = cwd.join(DIR);
    let git = git_snapshot(cwd);
    let mut deliverables: Vec<Deliverable> = Vec::new();
    let mut unreadable = 0usize;

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // 跳过状态 sidecar
            if name.ends_with(".status.json") {
                continue;
            }
            let id = name.trim_end_matches(".json").to_string();
            if !is_safe_record_id(&id) {
                log::warn!("[deliverables] skip unsafe record id {id:?}");
                unreadable += 1;
                continue;
            }
            let Some(raw) = read_text(&path) else {
                unreadable += 1;
                continue;
            };
            let sidecar = read_sidecar(cwd, &id);
            deliverables.push(parse_record(&id, &raw, cwd, &sidecar));
        }
    }

    // 待处理的排前面：先有问题的，再按 id
    deliverables.sort_by(|a, b| {
        let ka = (a.status == "accepted") as u8;
        let kb = (b.status == "accepted") as u8;
        ka.cmp(&kb).then_with(|| a.id.cmp(&b.id))
    });

    DeliverableScan {
        cwd: cwd.to_string_lossy().into_owned(),
        git,
        deliverables,
        unreadable,
    }
}

/// 人审查时看到的文本 —— 纯函数，便于单测（它是"人到底看到了什么"的唯一定义）。
///
/// 排版原则：**先摆事实，再摆证据，最后摆问题**。问题永远放最显眼的位置，
/// 因为"声明的文件不存在"这类事最容易被忽略。
pub fn review_text(scan: &DeliverableScan, d: &Deliverable) -> String {
    let mut s = String::new();
    s.push_str(&format!("{}\n", d.title));
    if d.id != d.title {
        s.push_str(&format!("记录 ID：{}\n", d.id));
    }
    if !d.summary.is_empty() {
        s.push_str(&format!("{}\n", d.summary));
    }
    s.push_str(&format!("验收状态：{}\n", d.status));

    if let Some(p) = &d.problem {
        s.push_str(&format!("\n⚠ 问题：{p}\n"));
    }

    s.push_str("\n── 声明的文件（存在性由本机核对）──\n");
    if d.files.is_empty() {
        s.push_str("（没有声明任何文件）\n");
    }
    for f in &d.files {
        if f.exists {
            s.push_str(&format!("✅ {} （{} 字节）\n", f.path, f.bytes.unwrap_or(0)));
        } else {
            s.push_str(&format!("❌ {} （不存在）\n", f.path));
        }
    }

    s.push_str("\n── 证据（按可否机器验证分级）──\n");
    if d.evidence.is_empty() {
        s.push_str("（没有声明任何证据）\n");
    }
    for e in &d.evidence {
        let mark = match e.grade {
            EvidenceGrade::MachineRunnable => "🔧",
            EvidenceGrade::Measured => "📐",
            EvidenceGrade::Unverifiable => "❔",
            EvidenceGrade::NotEvidence => "🚫",
        };
        s.push_str(&format!("{} [{}] {}\n", mark, e.kind, e.detail));
        if let Some(c) = &e.command {
            s.push_str(&format!("      $ {c}\n"));
        }
    }

    s.push_str("\n── 本机实测的验证结果 ──\n");
    match &d.verification {
        None => {
            s.push_str("（还没验过。上面 [exec] 那些命令是**声明**，不是实测 —— 未验证。）\n");
        }
        Some(v) => {
            s.push_str(&format!(
                "{} 条命令：{} 通过 / {} 未通过（{}）\n",
                v.results.len(),
                v.passed(),
                v.failed(),
                v.at
            ));
            for r in &v.results {
                let status = if r.timed_out {
                    "⏱ 超时".to_string()
                } else {
                    match r.exit_code {
                        Some(0) => "✅ 退出码 0".to_string(),
                        Some(c) => format!("❌ 退出码 {c}"),
                        None => "❌ 无退出码（被杀）".to_string(),
                    }
                };
                s.push_str(&format!("{} ({}ms) $ {}\n", status, r.duration_ms, r.command));
                if !r.output_tail.is_empty() {
                    for line in r.output_tail.lines().take(12) {
                        s.push_str(&format!("      | {line}\n"));
                    }
                }
            }
        }
    }

    s.push_str("\n── 本机自己量到的工作区改动 ──\n");
    if !scan.git.is_repo {
        s.push_str(&format!(
            "（不是 git 仓库：{}）\n",
            scan.git.error.as_deref().unwrap_or("未知原因")
        ));
    } else if scan.git.changes.is_empty() {
        s.push_str("（工作区无改动）\n");
    } else {
        let branch = scan.git.branch.as_deref().unwrap_or("(detached)");
        s.push_str(&format!("分支 {}，共 {} 个文件改动：\n", branch, scan.git.changes.len()));
        for c in scan.git.changes.iter().take(20) {
            let stat = match (c.added, c.deleted) {
                (Some(a), Some(dl)) => format!("+{a}/-{dl}"),
                _ if c.binary => "binary".to_string(),
                _ => String::new(),
            };
            s.push_str(&format!("  {} {} {}\n", c.status, c.path, stat));
        }
        if scan.git.changes.len() > 20 {
            s.push_str(&format!("  …还有 {} 个\n", scan.git.changes.len() - 20));
        }
    }

    if scan.unreadable > 0 {
        s.push_str(&format!("\n（另有 {} 份记录读不出来）\n", scan.unreadable));
    }

    s.push_str("\n接受这份产出吗？");
    s
}

/// 写验收状态。**只有人能调用**（命令层不暴露给模型）。
pub fn set_status(cwd: &Path, id: &str, status: &str, note: Option<&str>) -> Result<PathBuf, String> {
    if !is_safe_record_id(id) {
        return Err(format!("非法记录 id：{id:?}"));
    }
    if !matches!(status, "draft" | "pending" | "accepted" | "rejected") {
        return Err(format!("非法状态：{status:?}（只接受 draft/pending/accepted/rejected）"));
    }
    // 走统一的读-改-写入口：**不能**整体重写，否则会把已有的 verification 抹掉
    update_sidecar(
        cwd,
        id,
        serde_json::json!({
            "status": status,
            "note": note.unwrap_or(""),
            "at": chrono::Utc::now().to_rfc3339(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-deliv-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_record(cwd: &Path, id: &str, body: &str) -> PathBuf {
        let dir = cwd.join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{id}.json"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn record_id_guard_blocks_traversal() {
        assert!(is_safe_record_id("2026-10-14-draft"));
        assert!(!is_safe_record_id(""));
        assert!(!is_safe_record_id("../escape"));
        assert!(!is_safe_record_id("a/b"));
        assert!(!is_safe_record_id("a\\b"));
    }

    #[test]
    fn evidence_grading_never_treats_self_report_as_evidence() {
        assert_eq!(grade_of("exec"), EvidenceGrade::MachineRunnable);
        assert_eq!(grade_of("diff"), EvidenceGrade::Measured);
        assert_eq!(grade_of("artifact"), EvidenceGrade::Measured);
        assert_eq!(grade_of("external"), EvidenceGrade::Unverifiable);
        assert_eq!(grade_of("self-report"), EvidenceGrade::NotEvidence);
        assert_eq!(grade_of("whatever"), EvidenceGrade::NotEvidence, "未知一律保守");
    }

    #[test]
    fn declared_file_is_verified_against_the_real_filesystem() {
        let cwd = tmp("files");
        std::fs::write(cwd.join("real.txt"), "hello").unwrap();
        write_record(
            &cwd,
            "rec1",
            r#"{"title":"T","summary":"S","files":["real.txt","ghost.txt"],"evidence":[{"kind":"exec","command":"true"}]}"#,
        );
        let scan = scan(&cwd);
        assert_eq!(scan.deliverables.len(), 1);
        let d = &scan.deliverables[0];
        assert_eq!(d.files.len(), 2);
        assert!(d.files.iter().any(|f| f.path == "real.txt" && f.exists && f.bytes == Some(5)));
        assert!(d.files.iter().any(|f| f.path == "ghost.txt" && !f.exists));
        // 声明了不存在的文件 → 必须被标出来，不能静默
        assert!(d.problem.as_deref().unwrap_or("").contains("ghost.txt"), "problem={:?}", d.problem);
        assert_eq!(d.evidence[0].grade, EvidenceGrade::MachineRunnable);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn broken_json_is_visible_not_dropped() {
        let cwd = tmp("broken");
        write_record(&cwd, "bad", "{ not json");
        let scan = scan(&cwd);
        assert_eq!(scan.deliverables.len(), 1, "坏记录也必须出现在列表里");
        assert!(scan.deliverables[0].problem.as_deref().unwrap_or("").contains("JSON"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn missing_files_field_is_a_problem() {
        let cwd = tmp("nofiles");
        write_record(&cwd, "r", r#"{"title":"T","summary":"S"}"#);
        let scan = scan(&cwd);
        assert!(scan.deliverables[0].problem.as_deref().unwrap_or("").contains("files"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn status_sidecar_is_read_and_never_listed_as_a_record() {
        let cwd = tmp("status");
        write_record(&cwd, "r", r#"{"title":"T","summary":"S","files":[]}"#);
        set_status(&cwd, "r", "accepted", Some("看着不错")).unwrap();
        let scan = scan(&cwd);
        assert_eq!(scan.deliverables.len(), 1, "sidecar 不能被当成第二份产出");
        assert_eq!(scan.deliverables[0].status, "accepted");
        assert_eq!(scan.deliverables[0].status_note.as_deref(), Some("看着不错"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn set_status_rejects_bad_input() {
        let cwd = tmp("badstatus");
        assert!(set_status(&cwd, "../evil", "accepted", None).is_err());
        assert!(set_status(&cwd, "ok", "whatever", None).is_err());
        assert!(set_status(&cwd, "ok", "accepted", None).is_ok());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn accepted_items_sort_last() {
        let cwd = tmp("sort");
        write_record(&cwd, "a", r#"{"title":"A","summary":"S","files":[]}"#);
        write_record(&cwd, "b", r#"{"title":"B","summary":"S","files":[]}"#);
        set_status(&cwd, "a", "accepted", None).unwrap();
        let scan = scan(&cwd);
        assert_eq!(scan.deliverables[0].id, "b", "待处理的排前面");
        assert_eq!(scan.deliverables[1].id, "a");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn git_snapshot_reports_non_repo_without_lying() {
        let cwd = tmp("norepo");
        let snap = git_snapshot(&cwd);
        assert!(!snap.is_repo);
        let err = snap.error.clone().unwrap_or_default();
        assert!(!err.trim().is_empty(), "非仓库的说明不能是空串（stderr 必须带回来），got={err:?}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn git_snapshot_measures_real_changes() {
        let cwd = tmp("repo");
        let run = |args: &[&str]| {
            let (ok, out) = run_argv("git", args, &cwd, Duration::from_secs(10));
            assert!(ok, "git {args:?} 失败：{out}");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(cwd.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "init"]);
        // 改一个已有文件 + 加一个新文件
        std::fs::write(cwd.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(cwd.join("b.txt"), "new\n").unwrap();

        let snap = git_snapshot(&cwd);
        assert!(snap.is_repo);
        assert!(snap.error.is_none());
        let a = snap.changes.iter().find(|c| c.path == "a.txt").expect("a.txt 应在改动里");
        assert_eq!(a.added, Some(1), "增行数应由 numstat 量出");
        assert!(snap.changes.iter().any(|c| c.path == "b.txt"), "未跟踪文件也要出现");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn protocol_section_names_the_dir_and_the_rules() {
        let p = protocol_section();
        assert!(p.contains(DIR), "协议必须给出真实路径");
        assert!(p.contains("not-evidence") || p.contains("不算证据"), "必须写明自述不算证据");
        assert!(p.contains("stat") || p.contains("真实存在"), "必须写明文件会被核对");
    }

    #[test]
    fn review_text_puts_facts_first_and_problems_prominent() {
        let cwd = tmp("review");
        std::fs::write(cwd.join("out.txt"), "x").unwrap();
        write_record(
            &cwd,
            "rec",
            r#"{"title":"排版成品","summary":"生成了 out.txt","files":["out.txt","missing.txt"],
                "evidence":[{"kind":"exec","command":"node check.js"},{"kind":"external","note":"HTTP 200"},
                            {"kind":"self-report","note":"我觉得好了"}]}"#,
        );
        let scan = scan(&cwd);
        let d = &scan.deliverables[0];
        let t = review_text(&scan, d);
        eprintln!("===== 人在对话框里会看到的内容 =====\n{t}\n===== end =====");
        // 事实
        assert!(t.contains("排版成品"));
        assert!(t.contains("✅ out.txt"));
        assert!(t.contains("❌ missing.txt"), "不存在的文件必须显眼");
        // 问题在文件清单之前（先摆问题）
        let problem_at = t.find("⚠ 问题").expect("必须有问题段");
        let files_at = t.find("声明的文件").unwrap();
        assert!(problem_at < files_at, "问题要排在事实之前");
        // 证据分级可见
        assert!(t.contains("[exec]"));
        assert!(t.contains("[self-report]"));
        assert!(t.contains("不是证据"), "自述必须被明确标成不是证据");
        assert!(t.contains("$ node check.js"), "命令要原样给人看");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn review_text_reports_non_repo_and_empty_workspace() {
        let cwd = tmp("reviewempty");
        write_record(&cwd, "r", r#"{"title":"T","summary":"S","files":[]}"#);
        let scan = scan(&cwd);
        let t = review_text(&scan, &scan.deliverables[0]);
        assert!(t.contains("不是 git 仓库"), "非仓库要如实说明");
        assert!(t.contains("没有声明任何证据"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── 证据验证（④ 的最后一环：声明 → 实测） ─────────────────────────

    #[test]
    fn exec_commands_only_takes_runnable_exec_evidence() {
        let cwd = tmp("execs");
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[
                {"kind":"exec","command":"true"},
                {"kind":"diff","note":"不是命令"},
                {"kind":"exec","command":"true"},
                {"kind":"exec","command":"   "},
                {"kind":"self-report","command":"rm -rf /"}]}"#,
        );
        let cmds = exec_commands(&cwd, "r").unwrap();
        assert_eq!(cmds, vec!["true"], "只取 exec、去重、忽略空白；自述里的 command 绝不能被当成要跑的命令");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn exec_commands_errors_when_record_missing() {
        let cwd = tmp("execmiss");
        assert!(exec_commands(&cwd, "nope").is_err());
        assert!(exec_commands(&cwd, "../evil").is_err());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn verify_measures_real_exit_codes_and_output() {
        let cwd = tmp("verify");
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[
                {"kind":"exec","command":"true"},
                {"kind":"exec","command":"echo hello-from-evidence"},
                {"kind":"exec","command":"exit 3"}]}"#,
        );
        let v = verify_evidence(&cwd, "r", Duration::from_secs(20)).unwrap();
        assert_eq!(v.results.len(), 3);
        assert_eq!(v.results[0].exit_code, Some(0));
        assert_eq!(v.results[1].exit_code, Some(0));
        assert!(v.results[1].output_tail.contains("hello-from-evidence"), "输出要带回来");
        assert_eq!(v.results[2].exit_code, Some(3), "非零退出码必须如实记录");
        assert_eq!(v.passed(), 2);
        assert_eq!(v.failed(), 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn verify_enforces_timeout_and_marks_it() {
        let cwd = tmp("verifyto");
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"exec","command":"sleep 30"}]}"#,
        );
        let started = Instant::now();
        let v = verify_evidence(&cwd, "r", Duration::from_millis(400)).unwrap();
        assert!(v.results[0].timed_out, "超时必须被标记，不能当成失败或成功");
        assert!(v.results[0].exit_code.is_none());
        assert!(started.elapsed() < Duration::from_secs(10), "不能真的等 30 秒");
        assert_eq!(v.failed(), 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn verify_result_persists_and_shows_in_review() {
        let cwd = tmp("verifypersist");
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"exec","command":"echo ok"}]}"#,
        );
        verify_evidence(&cwd, "r", Duration::from_secs(20)).unwrap();

        // 扫描时应该能读回来（"声明了证据"与"证据通过了"是可区分的两件事）
        let scanned = scan(&cwd);
        let d = &scanned.deliverables[0];
        let v = d.verification.as_ref().expect("验证结果必须能读回来");
        assert_eq!(v.passed(), 1);
        let shown = review_text(&scanned, d);
        assert!(shown.contains("1 通过 / 0 未通过"));
        eprintln!("===== 验过之后人看到的（节选）=====\n{shown}\n===== end =====");

        // 还没验过的产出要明确说"未验证"
        write_record(&cwd, "r2", r#"{"title":"U","summary":"S","files":[],"evidence":[]}"#);
        let scanned2 = scan(&cwd);
        let d2 = scanned2.deliverables.iter().find(|x| x.id == "r2").unwrap();
        assert!(d2.verification.is_none());
        let t2 = review_text(&scanned2, d2);
        assert!(t2.contains("未验证") || t2.contains("还没验过"), "{t2}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 回归：验收状态与验证结果住在同一个 sidecar 里，必须**互不覆盖**。
    /// 分开整体重写就会悄悄丢掉其中一个（正是 `update_sidecar` 存在的理由）。
    #[test]
    fn status_and_verification_do_not_clobber_each_other() {
        let cwd = tmp("sidecar");
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"exec","command":"true"}]}"#,
        );
        verify_evidence(&cwd, "r", Duration::from_secs(20)).unwrap();
        set_status(&cwd, "r", "accepted", Some("OK")).unwrap();

        let scanned = scan(&cwd);
        let d = &scanned.deliverables[0];
        assert_eq!(d.status, "accepted", "先写验证再写状态：状态要在");
        assert!(d.verification.is_some(), "先写验证再写状态：验证结果不能被抹掉");

        // 反向：先状态后验证
        let cwd2 = tmp("sidecar2");
        write_record(
            &cwd2,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"exec","command":"true"}]}"#,
        );
        set_status(&cwd2, "r", "rejected", None).unwrap();
        verify_evidence(&cwd2, "r", Duration::from_secs(20)).unwrap();
        let scanned2 = scan(&cwd2);
        let d2 = &scanned2.deliverables[0];
        assert_eq!(d2.status, "rejected", "先写状态再写验证：状态不能被抹掉");
        assert!(d2.verification.is_some());
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&cwd2);
    }

    #[test]
    fn verify_errors_when_nothing_runnable() {
        let cwd = tmp("verifyempty");
        write_record(&cwd, "r", r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"external","note":"x"}]}"#);
        let err = verify_evidence(&cwd, "r", Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("没有声明可执行的证据"), "要如实说没得验，不能给个空报告冒充成功：{err}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 超时必须**真的**约束墙钟时间。
    ///
    /// 曾经这里踩过一个坑：超时标记置对了，但函数仍等了 31.7 秒才返回 ——
    /// 因为 `wait_with_output()` 会一直读到管道 EOF，而**孙进程还握着管道写端**。
    /// 所以这条测试断言的硬不变量是"**耗时上界**"，而不只是 `timed_out` 标记。
    #[test]
    fn timeout_bounds_wall_clock_even_when_a_descendant_leaks() {
        let cwd = tmp("bounded");
        // `sleep X & wait`：直接子进程是我们的 bash，它再挂一个孙进程。
        // 孙代继承同一个 stdout 管道 —— 正是会卡住 wait_with_output 的形态。
        write_record(
            &cwd,
            "r",
            r#"{"title":"T","summary":"S","files":[],"evidence":[{"kind":"exec","command":"sleep 31.7 & wait"}]}"#,
        );
        let started = Instant::now();
        let v = verify_evidence(&cwd, "r", Duration::from_millis(600)).unwrap();
        let elapsed = started.elapsed();

        assert!(v.results[0].timed_out, "应当被标记为超时");
        assert!(
            elapsed < Duration::from_secs(5),
            "超时没有约束住墙钟时间：实际耗时 {elapsed:?}（期望 ≪ 31.7s）"
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 进程树清扫的**直接**证据（机器允许跑 `ps` 时才执行）。
    ///
    /// 本沙箱里 `/bin/ps` 被拒，所以这条会在沙箱里跳过；在你的机器上它会真的
    /// 去 `ps` 里查有没有残留在跑的孙进程 —— 孤儿泄漏正是这个仓踩过的坑
    /// （"退出后 claude 还在"）。
    #[test]
    fn timeout_sweeps_descendants_when_ps_is_available() {
        let probe = std::process::Command::new("/bin/ps").args(["-axo", "pid="]).output();
        let Ok(probe) = probe else { return };
        if !probe.status.success() {
            return; // ps 不可用（沙箱）→ 跳过，不假装通过
        }

        let cwd = tmp("sweep");
        let marker = "sleep 31.9";
        write_record(
            &cwd,
            "r",
            &format!(
                r#"{{"title":"T","summary":"S","files":[],"evidence":[{{"kind":"exec","command":"{marker} & wait"}}]}}"#
            ),
        );
        let v = verify_evidence(&cwd, "r", Duration::from_millis(600)).unwrap();
        assert!(v.results[0].timed_out);
        std::thread::sleep(Duration::from_millis(500));

        let ps = std::process::Command::new("/bin/ps").args(["-axo", "command="]).output().unwrap();
        let text = String::from_utf8_lossy(&ps.stdout);
        let leaked: Vec<&str> = text.lines().filter(|l| l.contains(marker)).collect();
        assert!(leaked.is_empty(), "超时后仍有残留进程（孤儿泄漏）：{leaked:?}");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
