//! 任务板（目标 ⑤ 的第一块）。
//!
//! 一条任务 = **定义**（`.onecode/tasks/<id>.json`）+ **人的裁决**（`<id>.human.json`）。
//! 为什么拆成两份，而不是一个 `status` 字段：
//!
//! agent 有文件写权限，它完全可以自己往任务文件里写 `"status": "done"`。
//! 如果状态只有一份，**agent 就能自己宣布任务完成** —— 这与 ④ 里"模型自述不算证据"
//! 是同一条规矩。所以：
//!
//! - **文件 = 声明**（谁写的都行：董事长、@ceo、员工自己上报进度）；
//! - **sidecar = 人的裁决**，只有人触发的命令能写；
//! - 界面上两者**都显示**，生效状态以人为准，并标明来源。
//!
//! 任务与产出的关联是**派生**的（每次扫描时按 id 现查），不落盘复制 ——
//! 复制一份状态就会分叉。

use std::path::{Path, PathBuf};

use crate::deliverables;

/// 任务定义的目录（相对 cwd）。
pub const DIR: &str = ".onecode/tasks";

/// 合法状态。`done` 只有人能裁决。
pub const STATUSES: [&str; 6] = ["todo", "doing", "blocked", "review", "done", "dropped"];

/// 任务引用的产出（现场查出来的事实，不落盘）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeliverableRef {
    pub id: String,
    /// 产出记录是否存在（引用了一个不存在的 id 是最常见的笔误）
    pub exists: bool,
    pub status: String,
    /// `Some((通过, 总数))` = 本机实测过；`None` = 未验证
    pub verified: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// 负责的员工 id（`@name` 的 name）；空 = 未指派
    pub owner: String,
    /// 文件里写的状态（**声明**）
    pub declared_status: String,
    /// 人的裁决（sidecar）；`None` = 人还没裁决
    pub human_status: Option<String>,
    /// 生效状态：人有裁决以人为准
    pub status: String,
    /// `human` | `declared`
    pub status_source: String,
    pub note: String,
    /// ★ **任务级模型覆盖**（目标 ②「按**任务**自选模型」）。
    /// 空 = 跟随员工花名册 / 全局。有值时它在优先级上等同"显式指定"，
    /// 也就是**压过员工花名册** —— 一条任务要换模型，通常正是因为这条活特殊。
    pub provider_id: String,
    pub model: String,
    pub blocked_by: Vec<String>,
    pub deliverables: Vec<String>,
    /// 记录本身的问题（JSON 坏了 / 非法状态 / 自引用 / 引用不存在的产出…）
    pub problem: Option<String>,
    /// 产出关联的现场事实
    pub deliverable_refs: Vec<DeliverableRef>,
    /// 关联的产出是否**全部**被接受（没有关联产出时为 false —— 不把"没产出"当"完成"）
    pub all_deliverables_accepted: bool,
    /// 还没完成的阻塞项（不存在或未完成的都算）
    pub unresolved_blockers: Vec<String>,
}

impl Task {
    /// 这条任务给出的**路线覆盖**（目标 ②「按任务自选模型」）。
    ///
    /// 只此一处实现：命令层拿它去填"显式指定"，而显式指定在
    /// `providers::resolve_route_for_agent` 里**压过员工花名册** ——
    /// 一条任务要换模型，通常正是因为这条活特殊。
    ///
    /// 返回 `(provider, model)`；空串归一成 `None`（"没写"与"写了空"是一回事）。
    pub fn route_override(&self) -> (Option<&str>, Option<&str>) {
        let p = self.provider_id.trim();
        let m = self.model.trim();
        (
            if p.is_empty() { None } else { Some(p) },
            if m.is_empty() { None } else { Some(m) },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TaskBoard {
    pub cwd: String,
    pub tasks: Vec<Task>,
    /// 读不出来的任务记录数（坏 JSON / 非法 id）
    pub unreadable: usize,
}

/// 注入给员工的**任务协议**正文。
///
/// 为什么让员工写任务文件而不是我们替它写：这个公司的入口是"董事长只跟 @ceo 说话"，
/// 派活天然是对话。员工把任务落成文件，人才能在板上看见并进行裁决。
pub fn protocol_section() -> String {
    format!(
        r#"## 任务协议（被派了活就要做）

被派了一项工作，把它写成一条任务记录：`<cwd>/{DIR}/<id>.json`（`<id>` 简短英文/数字，如 `2026-10-14-wechat-draft`）：

```json
{{
  "title": "一句能看懂的任务名",
  "owner": "负责人员的 id（就是你自己的名字，不带 @）",
  "status": "todo | doing | blocked | review",
  "note": "可选的进展说明",
  "blocked_by": ["卡住你的任务 id"],
  "deliverables": ["这次交付的产出记录 id"]
}}
```

规矩：

- 开始干活时写 `doing`，干完等验收写 **`review`**；**不要写 `done`** ——
  完成由人裁决，你自己写的 `done` 在我们这里只是"自报"，不会生效（界面会标出来）。
- 交付了东西就把产出记录的 id 填进 `deliverables`，人才能顺着任务找到证据。
- 卡住了就写 `blocked` 并在 `blocked_by` 里写清被谁卡住。
- 不写任务记录 = 这项活在板上不存在，董事长看不到你在做什么。
"#
    )
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn sidecar_path(cwd: &Path, id: &str) -> PathBuf {
    cwd.join(DIR).join(format!("{id}.human.json"))
}

/// 人的裁决：读 sidecar（`status` / `note`）。
fn read_human(cwd: &Path, id: &str) -> Option<(String, Option<String>)> {
    let raw = read_text(&sidecar_path(cwd, id))?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let status = v.get("status")?.as_str()?.to_string();
    if !STATUSES.contains(&status.as_str()) {
        return None;
    }
    let note = v
        .get("note")
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Some((status, note))
}

/// 解析一条任务，并用**现场事实**校正。
///
/// 与产出记录同样的原则：解析失败**不返回 None**，而是返回带 `problem` 的条目 ——
/// 坏记录必须看得见。
fn parse_task(
    id: &str,
    raw: &str,
    human: Option<(String, Option<String>)>,
    deliverables_by_id: &std::collections::HashMap<String, DeliverableRef>,
    known_ids: &std::collections::HashSet<String>,
) -> Task {
    let mut problem: Option<String> = None;
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Task {
                id: id.to_string(),
                title: id.to_string(),
                owner: String::new(),
                declared_status: "todo".to_string(),
                human_status: human.as_ref().map(|h| h.0.clone()),
                status: human.map(|h| h.0).unwrap_or_else(|| "todo".to_string()),
                status_source: "declared".to_string(),
                note: String::new(),
                provider_id: String::new(),
                model: String::new(),
                blocked_by: Vec::new(),
                deliverables: Vec::new(),
                problem: Some(format!("JSON 解析失败：{e}")),
                deliverable_refs: Vec::new(),
                all_deliverables_accepted: false,
                unresolved_blockers: Vec::new(),
            }
        }
    };

    let get_str = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let get_arr = |k: &str| -> Vec<String> {
        match value.get(k) {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    };

    let title = {
        let t = get_str("title");
        if t.is_empty() {
            problem = Some("缺少 title".to_string());
            id.to_string()
        } else {
            t
        }
    };
    let owner = get_str("owner").trim_start_matches('@').to_string();

    let declared_status = {
        let s = get_str("status").trim().to_lowercase();
        if s.is_empty() {
            "todo".to_string()
        } else if STATUSES.contains(&s.as_str()) {
            s
        } else {
            problem = Some(match problem.take() {
                Some(p) => format!("{p}；无法识别的 status {s:?}"),
                None => format!("无法识别的 status {s:?}（只接受 {}）", STATUSES.join("/")),
            });
            "todo".to_string()
        }
    };

    let blocked_by = get_arr("blocked_by");
    let deliverables = get_arr("deliverables");

    // 自引用：任务把自己列为阻塞项 → 永远卡住
    if blocked_by.iter().any(|b| b == id) {
        problem = Some(match problem.take() {
            Some(p) => format!("{p}；blocked_by 引用了自己（会永远卡住）"),
            None => "blocked_by 引用了自己（会永远卡住）".to_string(),
        });
    }

    // 关联产出：现场查，不复制状态
    let mut deliverable_refs: Vec<DeliverableRef> = Vec::new();
    for d in &deliverables {
        match deliverables_by_id.get(d) {
            Some(r) => deliverable_refs.push(r.clone()),
            None => {
                deliverable_refs.push(DeliverableRef {
                    id: d.clone(),
                    exists: false,
                    status: "missing".to_string(),
                    verified: None,
                });
                problem = Some(match problem.take() {
                    Some(p) => format!("{p}；引用的产出不存在：{d}"),
                    None => format!("引用的产出不存在：{d}"),
                });
            }
        }
    }

    let all_accepted = !deliverable_refs.is_empty()
        && deliverable_refs.iter().all(|r| r.exists && r.status == "accepted");

    // 阻塞项：不存在或未完成都算没解决
    let unresolved: Vec<String> = blocked_by
        .iter()
        .filter(|b| {
            if !known_ids.contains(*b) {
                return true; // 引用不存在的任务 → 永远解决不了，如实报出
            }
            // 由调用方在组装时按生效状态过滤；这里先保留，组装后统一算
            true
        })
        .cloned()
        .collect();

    let human_status = human.as_ref().map(|h| h.0.clone());
    let (status, status_source) = match &human_status {
        Some(h) => (h.clone(), "human".to_string()),
        None => (declared_status.clone(), "declared".to_string()),
    };

    // 自报完成：agent 可以往文件里写 done，但我们把它标出来 —— 生效状态仍按声明走，
    // 由人决定要不要裁决，**不静默改写**（人手工编辑文件的情况也存在）。
    if human_status.is_none() && declared_status == "done" {
        problem = Some(match problem.take() {
            Some(p) => format!("{p}；状态 done 来自任务文件本身（未经人裁决）"),
            None => "状态 done 来自任务文件本身（未经人裁决）".to_string(),
        });
    }

    let _ = human; // human 的 note 暂不参与展示（保留给下一步）

    Task {
        id: id.to_string(),
        title,
        owner,
        declared_status,
        human_status,
        status,
        status_source,
        note: get_str("note"),
        provider_id: get_str("provider_id").trim().to_string(),
        model: get_str("model").trim().to_string(),
        blocked_by,
        deliverables,
        problem,
        deliverable_refs,
        all_deliverables_accepted: all_accepted,
        unresolved_blockers: unresolved,
    }
}

/// 扫描一个工作区的任务板。
pub fn scan(cwd: &Path) -> TaskBoard {
    let dir = cwd.join(DIR);

    // 产出状态现场查一次，供所有任务共享（避免每个任务重扫一遍 git）
    let ds = deliverables::scan(cwd);
    let mut deliverables_by_id: std::collections::HashMap<String, DeliverableRef> =
        std::collections::HashMap::new();
    for d in &ds.deliverables {
        deliverables_by_id.insert(
            d.id.clone(),
            DeliverableRef {
                id: d.id.clone(),
                exists: true,
                status: d.status.clone(),
                verified: d.verification.as_ref().map(|v| (v.passed(), v.results.len())),
            },
        );
    }

    // 先收原始记录，才能算出"哪些 blocked_by 是已完成的"
    let mut raws: Vec<(String, String, Option<(String, Option<String>)>)> = Vec::new();
    let mut unreadable = 0usize;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".human.json") {
                continue; // sidecar 不是任务
            }
            let id = name.trim_end_matches(".json").to_string();
            if !deliverables::is_safe_record_id(&id) {
                log::warn!("[tasks] skip unsafe task id {id:?}");
                unreadable += 1;
                continue;
            }
            let Some(raw) = read_text(&path) else {
                unreadable += 1;
                continue;
            };
            raws.push((id.clone(), raw, read_human(cwd, &id)));
        }
    }

    // 生效状态表（用于算阻塞是否解决）
    let mut effective: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (id, raw, human) in &raws {
        let v: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
        let declared = v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("todo")
            .trim()
            .to_lowercase();
        let st = human
            .as_ref()
            .map(|h| h.0.clone())
            .unwrap_or(declared);
        effective.insert(id.clone(), st);
    }
    let known_ids: std::collections::HashSet<String> = effective.keys().cloned().collect();

    let mut tasks: Vec<Task> = raws
        .iter()
        .map(|(id, raw, human)| {
            parse_task(id, raw, human.clone(), &deliverables_by_id, &known_ids)
        })
        .collect();

    // 组装完再算"未解决的阻塞项"：已完成（done/dropped）的阻塞项才算解决
    for t in tasks.iter_mut() {
        t.unresolved_blockers = t
            .blocked_by
            .iter()
            .filter(|b| {
                !matches!(effective.get(*b).map(|s| s.as_str()), Some("done") | Some("dropped"))
            })
            .cloned()
            .collect();
        if !t.unresolved_blockers.is_empty() {
            let note = format!("有 {} 个未解决的阻塞项", t.unresolved_blockers.len());
            t.problem = Some(match t.problem.take() {
                Some(p) => format!("{p}；{note}"),
                None => note,
            });
        }
    }

    // 板上前排：未完成的在前；再按有无问题；最后按 id
    tasks.sort_by(|a, b| {
        let done = |t: &Task| matches!(t.status.as_str(), "done" | "dropped") as u8;
        done(a)
            .cmp(&done(b))
            .then_with(|| (a.problem.is_none() as u8).cmp(&(b.problem.is_none() as u8)))
            .then_with(|| a.id.cmp(&b.id))
    });

    TaskBoard {
        cwd: cwd.to_string_lossy().into_owned(),
        tasks,
        unreadable,
    }
}

/// 任务板的**人看到的样子**（纯函数，便于单测）。
///
/// 诚实原则与产出一致：先摆问题，再摆事实；"自报完成"必须能一眼看出来。
pub fn board_text(board: &TaskBoard) -> String {
    if board.tasks.is_empty() {
        return format!(
            "当前项目还没有任务。\n\n任务记录放在 {}/{}/<id>.json —— 被派了活时员工会写，\n你也可以让 @ceo 记下来。",
            board.cwd, DIR
        );
    }
    let mut s = format!("共 {} 条任务\n", board.tasks.len());
    if board.unreadable > 0 {
        s.push_str(&format!("（另有 {} 份记录读不出来）\n", board.unreadable));
    }
    for t in &board.tasks {
        let owner = if t.owner.is_empty() { "(未指派)".to_string() } else { format!("@{}", t.owner) };
        // 状态来源要标出来：agent 自报的 done 和人裁决的 done 不是一回事
        let marker = match (t.status.as_str(), t.status_source.as_str()) {
            ("done", "human") => "✅ done（人裁决）",
            ("done", _) => "⚠ done（自报，未经人裁决）",
            ("dropped", "human") => "🗑 dropped（人裁决）",
            ("blocked", _) => "⛔ blocked",
            ("review", _) => "👀 review（待验收）",
            ("doing", _) => "▶ doing",
            _ => "· todo",
        };
        s.push_str(&format!("\n[{}] {}  {}  {}", marker, t.id, t.title, owner));
        if !t.note.is_empty() {
            s.push_str(&format!("\n      备注：{}", t.note));
        }
        if !t.model.is_empty() || !t.provider_id.is_empty() {
            s.push_str(&format!(
                "\n      模型：{}（任务级覆盖，压过员工花名册）",
                if t.model.is_empty() { t.provider_id.clone() } else { format!("{} / {}", t.provider_id, t.model) }
            ));
        }
        if !t.deliverable_refs.is_empty() {
            s.push_str("\n      产出：");
            let parts: Vec<String> = t
                .deliverable_refs
                .iter()
                .map(|r| {
                    if !r.exists {
                        return format!("{} ❌不存在", r.id);
                    }
                    let v = match r.verified {
                        Some((p, n)) if p == n => format!("，证据 {p}/{n} ✅"),
                        Some((p, n)) => format!("，证据 {p}/{n} ❌"),
                        None => "，证据未验证".to_string(),
                    };
                    format!("{} [{}{}]", r.id, r.status, v)
                })
                .collect();
            s.push_str(&parts.join("  "));
            if t.all_deliverables_accepted {
                // "人接受过"与"证据验过"是两件事：证据没验过时必须说出来，
                // 否则这一行会被读成"证据也过了"。
                let unverified = t
                    .deliverable_refs
                    .iter()
                    .filter(|r| r.verified.is_none())
                    .count();
                s.push_str("\n      → 关联产出全部**已被人验收**");
                if unverified > 0 {
                    s.push_str(&format!("；但其中 {unverified} 个的**证据还没验证过**"));
                }
                s.push_str("，可以考虑关闭");
            }
        }
        if !t.unresolved_blockers.is_empty() {
            s.push_str(&format!("\n      被卡于：{}", t.unresolved_blockers.join(", ")));
        }
        if let Some(p) = &t.problem {
            s.push_str(&format!("\n      ⚠ {p}"));
        }
        s.push('\n');
    }
    s
}

/// 新建一条任务（**人触发**）。返回任务 id。
///
/// id 由标题 slug + 序号组成：可读、可排序、且**保证路径安全**
/// （只保留 `[a-z0-9-]`，天然挡住 `..` 与 `/`）。
///
/// `provider_id` / `model` 是**任务级模型覆盖**（目标 ② 的"按任务自选模型"）；
/// 两个都留空 = 跟随员工花名册 / 全局。没有单独的 `create` 包装 —— 多一个几乎
/// 同名的入口只会让人猜"该用哪个"（规则 7：同一件事别有两处实现）。
#[allow(clippy::too_many_arguments)]
pub fn create_full(
    cwd: &Path,
    title: &str,
    owner: &str,
    note: &str,
    blocked_by: &[String],
    deliverables: &[String],
    provider_id: &str,
    model: &str,
) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("任务标题不能为空".to_string());
    }
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;

    let mut slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    // 中文标题会全被折成 '-'：那就退化成一个基于时间的 id，而不是空 id
    let base = if slug.is_empty() {
        format!("t-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    } else {
        slug.chars().take(48).collect::<String>()
    };

    let mut id = base.clone();
    let mut n = 1;
    while dir.join(format!("{id}.json")).exists() {
        n += 1;
        id = format!("{base}-{n}");
        if n > 999 {
            return Err("同名任务太多，换个标题".to_string());
        }
    }
    if !deliverables::is_safe_record_id(&id) {
        return Err(format!("生成的 id 不安全：{id:?}"));
    }

    let body = serde_json::json!({
        "title": title,
        "owner": owner.trim().trim_start_matches('@'),
        "status": "todo",
        "note": note,
        "provider_id": provider_id.trim(),
        "model": model.trim(),
        "blocked_by": blocked_by,
        "deliverables": deliverables,
    });
    let path = dir.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入失败：{e}"))?;
    log::info!("[tasks] created {} at {}", id, path.display());
    Ok(id)
}

/// **人的裁决**：写生效状态。只有人触发的命令能调用。
///
/// `done` 只能从这里来 —— agent 往任务文件里写 `done` 只会被标成"自报"。
pub fn set_human_status(
    cwd: &Path,
    id: &str,
    status: &str,
    note: Option<&str>,
) -> Result<PathBuf, String> {
    if !deliverables::is_safe_record_id(id) {
        return Err(format!("非法任务 id：{id:?}"));
    }
    if !STATUSES.contains(&status) {
        return Err(format!("非法状态：{status:?}（只接受 {}）", STATUSES.join("/")));
    }
    if !cwd.join(DIR).join(format!("{id}.json")).exists() {
        return Err(format!("任务不存在：{id}"));
    }
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let path = sidecar_path(cwd, id);
    let body = serde_json::json!({
        "status": status,
        "note": note.unwrap_or(""),
        "at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(&path, serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入失败：{e}"))?;
    Ok(path)
}

/// 改任务的**定义**（负责人 / 备注 / 关联产出 / **任务级模型**）。
/// `None` = 该项不动。状态请走 [`set_human_status`]。
#[allow(clippy::too_many_arguments)]
pub fn update_definition_full(
    cwd: &Path,
    id: &str,
    owner: Option<&str>,
    note: Option<&str>,
    deliverables: Option<&[String]>,
    provider_id: Option<&str>,
    model: Option<&str>,
) -> Result<PathBuf, String> {
    if !deliverables::is_safe_record_id(id) {
        return Err(format!("非法任务 id：{id:?}"));
    }
    let path = cwd.join(DIR).join(format!("{id}.json"));
    let raw = read_text(&path).ok_or_else(|| format!("任务不存在：{id}"))?;
    let mut v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("任务文件解析失败：{e}"))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| "任务文件必须是 JSON 对象".to_string())?;
    if let Some(o) = owner {
        obj.insert(
            "owner".to_string(),
            serde_json::json!(o.trim().trim_start_matches('@')),
        );
    }
    if let Some(n) = note {
        obj.insert("note".to_string(), serde_json::json!(n));
    }
    if let Some(d) = deliverables {
        obj.insert("deliverables".to_string(), serde_json::json!(d));
    }
    if let Some(p) = provider_id {
        obj.insert("provider_id".to_string(), serde_json::json!(p.trim()));
    }
    if let Some(m) = model {
        obj.insert("model".to_string(), serde_json::json!(m.trim()));
    }
    std::fs::write(&path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入失败：{e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-tasks-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_task(cwd: &Path, id: &str, body: &str) {
        let dir = cwd.join(DIR);
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
            crate::deliverables::set_status(cwd, id, s, None).unwrap();
        }
    }

    #[test]
    fn scan_reads_a_task_and_normalises_owner() {
        let cwd = tmp("basic");
        write_task(
            &cwd,
            "t1",
            r#"{"title":"写稿","owner":"@wechat-publisher","status":"doing","note":"进行中"}"#,
        );
        let b = scan(&cwd);
        assert_eq!(b.tasks.len(), 1);
        let t = &b.tasks[0];
        assert_eq!(t.title, "写稿");
        assert_eq!(t.owner, "wechat-publisher", "@ 要去掉");
        assert_eq!(t.status, "doing");
        assert_eq!(t.status_source, "declared");
        assert!(t.human_status.is_none());
        assert!(t.problem.is_none(), "problem={:?}", t.problem);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn human_ruling_wins_over_the_declared_status() {
        let cwd = tmp("human");
        write_task(&cwd, "t1", r#"{"title":"X","status":"review"}"#);
        set_human_status(&cwd, "t1", "done", Some("验收通过")).unwrap();
        let b = scan(&cwd);
        let t = &b.tasks[0];
        assert_eq!(t.status, "done", "人以人为准");
        assert_eq!(t.status_source, "human");
        assert_eq!(t.human_status.as_deref(), Some("done"));
        assert_eq!(t.declared_status, "review", "声明值要保留下来给人看");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// **核心治理性质**：agent 自己往文件里写 `done`，不能变成"已完成"。
    #[test]
    fn agent_self_reported_done_is_flagged_not_trusted() {
        let cwd = tmp("selfdone");
        write_task(&cwd, "t1", r#"{"title":"X","status":"done"}"#);
        let b = scan(&cwd);
        let t = &b.tasks[0];
        assert!(t.human_status.is_none());
        assert!(
            t.problem.as_deref().unwrap_or("").contains("未经人裁决"),
            "自报完成必须被标出来：{:?}",
            t.problem
        );
        // 而且不能被当成"已完成的任务"从待办里消失 —— 排序上它仍在未完成组
        assert_eq!(t.status_source, "declared");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn human_sidecar_is_not_listed_as_a_task() {
        let cwd = tmp("sidecar");
        write_task(&cwd, "t1", r#"{"title":"X","status":"todo"}"#);
        set_human_status(&cwd, "t1", "doing", None).unwrap();
        let b = scan(&cwd);
        assert_eq!(b.tasks.len(), 1, "human sidecar 不能被当成第二条任务");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn broken_json_is_visible() {
        let cwd = tmp("broken");
        write_task(&cwd, "bad", "{ not json");
        let b = scan(&cwd);
        assert_eq!(b.tasks.len(), 1);
        assert!(b.tasks[0].problem.as_deref().unwrap_or("").contains("JSON"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn illegal_status_is_reported_and_falls_back_to_todo() {
        let cwd = tmp("badstatus");
        write_task(&cwd, "t1", r#"{"title":"X","status":"finished"}"#);
        let b = scan(&cwd);
        assert_eq!(b.tasks[0].status, "todo");
        assert!(b.tasks[0].problem.as_deref().unwrap_or("").contains("finished"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn self_referencing_blocker_is_reported() {
        let cwd = tmp("selfblock");
        write_task(&cwd, "t1", r#"{"title":"X","blocked_by":["t1"]}"#);
        let b = scan(&cwd);
        assert!(b.tasks[0].problem.as_deref().unwrap_or("").contains("引用了自己"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn blockers_resolve_only_when_effectively_done() {
        let cwd = tmp("blockers");
        write_task(&cwd, "a", r#"{"title":"A"}"#);
        write_task(&cwd, "b", r#"{"title":"B","blocked_by":["a"]}"#);
        let b1 = scan(&cwd);
        let tb = b1.tasks.iter().find(|t| t.id == "b").unwrap();
        assert_eq!(tb.unresolved_blockers, vec!["a"], "a 没完成 → b 被卡");

        set_human_status(&cwd, "a", "done", None).unwrap();
        let b2 = scan(&cwd);
        let tb2 = b2.tasks.iter().find(|t| t.id == "b").unwrap();
        assert!(tb2.unresolved_blockers.is_empty(), "a 由人裁决完成后 → b 不再被卡");

        // 引用一个不存在的任务 → 永远解决不了，必须报出来
        write_task(&cwd, "c", r#"{"title":"C","blocked_by":["ghost"]}"#);
        let b3 = scan(&cwd);
        let tc = b3.tasks.iter().find(|t| t.id == "c").unwrap();
        assert_eq!(tc.unresolved_blockers, vec!["ghost"]);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn deliverable_links_are_resolved_live_and_missing_ones_reported() {
        let cwd = tmp("links");
        write_deliverable(&cwd, "d1", Some("accepted"));
        write_deliverable(&cwd, "d2", None);
        write_task(&cwd, "t1", r#"{"title":"X","deliverables":["d1","d2","ghost"]}"#);
        let b = scan(&cwd);
        let t = &b.tasks[0];
        assert_eq!(t.deliverable_refs.len(), 3);
        let r1 = t.deliverable_refs.iter().find(|r| r.id == "d1").unwrap();
        assert!(r1.exists);
        assert_eq!(r1.status, "accepted");
        assert!(
            t.deliverable_refs.iter().any(|r| r.id == "ghost" && !r.exists),
            "不存在的产出引用要标出来"
        );
        assert!(t.problem.as_deref().unwrap_or("").contains("ghost"));
        assert!(!t.all_deliverables_accepted, "d2 还没验收");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn all_accepted_is_false_when_no_deliverables_are_linked() {
        // 不把"没有关联产出"当成"完成" —— 那会让空任务看起来可以关
        let cwd = tmp("nodeliv");
        write_task(&cwd, "t1", r#"{"title":"X"}"#);
        let b = scan(&cwd);
        assert!(!b.tasks[0].all_deliverables_accepted);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn all_accepted_becomes_true_only_after_every_link_is_accepted() {
        let cwd = tmp("allacc");
        write_deliverable(&cwd, "d1", Some("accepted"));
        write_task(&cwd, "t1", r#"{"title":"X","deliverables":["d1"]}"#);
        assert!(scan(&cwd).tasks[0].all_deliverables_accepted);

        // 验收状态改成驳回 → 立刻不再是"全部通过"（派生事实，不落盘分叉）
        crate::deliverables::set_status(&cwd, "d1", "rejected", None).unwrap();
        assert!(!scan(&cwd).tasks[0].all_deliverables_accepted);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn create_makes_a_safe_id_and_avoids_collisions() {
        let cwd = tmp("create");
        let id1 = create_full(&cwd, "Write the WeChat draft", "dev", "", &[], &[], "", "").unwrap();
        assert_eq!(id1, "write-the-wechat-draft");
        assert!(deliverables::is_safe_record_id(&id1));
        let id2 = create_full(&cwd, "Write the WeChat draft", "dev", "", &[], &[], "", "").unwrap();
        assert_eq!(id2, "write-the-wechat-draft-2", "同名要避让，不能互相覆盖");

        // 纯中文标题会被折成空 slug → 退化成时间 id，而不是空 id / 不安全 id
        let id3 = create_full(&cwd, "写一篇公众号稿", "dev", "", &[], &[], "", "").unwrap();
        assert!(!id3.is_empty());
        assert!(deliverables::is_safe_record_id(&id3), "中文标题也要产出安全 id：{id3}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn create_rejects_empty_title_and_traversal_is_impossible() {
        let cwd = tmp("createbad");
        assert!(create_full(&cwd, "   ", "dev", "", &[], &[], "", "").is_err());
        // 标题里带路径穿越 → 会被 slug 折掉，不可能逃出目录
        let id = create_full(&cwd, "../../etc/passwd", "dev", "", &[], &[], "", "").unwrap();
        assert!(deliverables::is_safe_record_id(&id));
        assert!(!id.contains("..") && !id.contains('/'));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn set_human_status_validates_input_and_existence() {
        let cwd = tmp("sethuman");
        write_task(&cwd, "t1", r#"{"title":"X"}"#);
        assert!(set_human_status(&cwd, "t1", "whatever", None).is_err());
        assert!(set_human_status(&cwd, "../evil", "done", None).is_err());
        assert!(set_human_status(&cwd, "ghost", "done", None).is_err(), "不能给不存在的任务写裁决");
        assert!(set_human_status(&cwd, "t1", "done", None).is_ok());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn update_definition_changes_owner_and_links_only() {
        let cwd = tmp("updatedef");
        write_task(&cwd, "t1", r#"{"title":"X","status":"doing","owner":"a"}"#);
        update_definition_full(&cwd, "t1", Some("@b"), Some("新备注"), Some(&["d9".to_string()]), None, None).unwrap();
        let b = scan(&cwd);
        let t = &b.tasks[0];
        assert_eq!(t.owner, "b");
        assert_eq!(t.note, "新备注");
        assert_eq!(t.deliverables, vec!["d9"]);
        assert_eq!(t.status, "doing", "改定义不该动状态");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn finished_tasks_sort_last_and_problem_tasks_first() {
        let cwd = tmp("sort");
        write_task(&cwd, "a", r#"{"title":"A"}"#);
        write_task(&cwd, "b", r#"{"title":"B"}"#);
        write_task(&cwd, "c", r#"{"title":"C"}"#);
        set_human_status(&cwd, "a", "done", None).unwrap();
        let b = scan(&cwd);
        assert_eq!(b.tasks.last().unwrap().id, "a", "已完成的排最后");

        // 有问题的排在同组前面
        write_task(&cwd, "z", "{bad");
        let b2 = scan(&cwd);
        let open: Vec<&str> = b2
            .tasks
            .iter()
            .filter(|t| t.status != "done")
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(open[0], "z", "有问题的排最前：{open:?}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn protocol_section_names_the_dir_and_the_done_rule() {
        let p = protocol_section();
        assert!(p.contains(DIR));
        assert!(p.contains("不要写 `done`") || p.contains("不要写 done"));
        assert!(p.contains("review"), "要告诉员工干完写 review 等验收");
    }

    #[test]
    fn board_text_flags_self_reported_done_visibly() {
        let cwd = tmp("boardtext");
        write_task(&cwd, "a", r#"{"title":"自报的","status":"done"}"#);
        write_task(&cwd, "b", r#"{"title":"人裁决的","owner":"dev","status":"review","deliverables":["d1"]}"#);
        write_deliverable(&cwd, "d1", Some("accepted"));
        let text = board_text(&scan(&cwd));
        eprintln!("===== 任务板（人看到的）=====\n{text}\n===== end =====");
        assert!(text.contains("自报，未经人裁决"), "自报完成必须一眼可见：\n{text}");
        // 关联产出全通过时给出"可以考虑关闭"的提示
        assert!(text.contains("已被人验收"));
        // 但证据没验过必须一并说明 —— 不能让人读成"证据也过了"
        assert!(text.contains("证据还没验证过"), "{text}");
        assert!(text.contains("@dev"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn board_text_on_empty_board_says_where_records_live() {
        let cwd = tmp("boardempty");
        let text = board_text(&scan(&cwd));
        assert!(text.contains(DIR), "空板也要告诉人记录放哪：{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── 任务级模型（目标 ②「按任务自选模型」）──────────────────────────

    #[test]
    fn a_task_can_carry_its_own_model() {
        let cwd = tmp("taskmodel");
        write_task(
            &cwd,
            "t1",
            r#"{"title":"X","owner":"dev","provider_id":"glm","model":"glm-4-plus"}"#,
        );
        let b = scan(&cwd);
        let t = &b.tasks[0];
        assert_eq!(t.provider_id, "glm");
        assert_eq!(t.model, "glm-4-plus");
        assert_eq!(t.route_override(), (Some("glm"), Some("glm-4-plus")));
        // 板子上要看得出这是任务级覆盖
        let text = board_text(&b);
        eprintln!("===== 任务板（任务级模型）=====\n{text}\n===== end =====");
        assert!(text.contains("模型：glm / glm-4-plus"), "{text}");
        assert!(text.contains("任务级覆盖"), "要说清它压过花名册：{text}");
        assert!(!text.contains("**"), "对话框是纯文本，不该出现 markdown 星号：{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_task_without_a_model_gives_no_override() {
        let cwd = tmp("nomodel");
        write_task(&cwd, "t1", r#"{"title":"X"}"#);
        let b = scan(&cwd);
        assert_eq!(b.tasks[0].route_override(), (None, None));
        assert!(!board_text(&b).contains("任务级覆盖"), "没写就不该显示");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn blank_strings_are_normalised_to_no_override() {
        // "写了空" 与 "没写" 必须等价，否则会出现"看起来设了其实没设"
        let cwd = tmp("blankmodel");
        write_task(&cwd, "t1", r#"{"title":"X","provider_id":"  ","model":""}"#);
        let b = scan(&cwd);
        assert_eq!(b.tasks[0].route_override(), (None, None));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn create_and_update_can_set_the_task_model() {
        let cwd = tmp("setmodel");
        let id = create_full(&cwd, "写稿", "dev", "", &[], &[], "glm", "glm-4-plus").unwrap();
        let b = scan(&cwd);
        let t = b.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.route_override(), (Some("glm"), Some("glm-4-plus")));

        // 改模型；只传 provider 时 model 不动（None = 该项不动）
        update_definition_full(&cwd, &id, None, None, None, Some("deepseek"), None).unwrap();
        let b2 = scan(&cwd);
        let t2 = b2.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t2.provider_id, "deepseek");
        assert_eq!(t2.model, "glm-4-plus", "只改 provider 时 model 不该被动");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 端到端（跨模块）：任务写了模型 → 变成"显式指定" → **压过员工花名册**。
    #[test]
    fn a_task_model_beats_the_employee_roster_end_to_end() {
        use std::collections::HashMap as Map;
        let cwd = tmp("beats");
        write_task(
            &cwd,
            "t1",
            r#"{"title":"特殊活","owner":"dev","provider_id":"glm","model":"glm-special"}"#,
        );
        let task = scan(&cwd).tasks[0].clone();

        // 员工花名册把 dev 绑到别处
        let mut cat = crate::providers::ProviderCatalog::default();
        let mk = |id: &str, m: &str| crate::providers::Provider {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("https://{id}.example/v1"),
            api_key: format!("key-{id}"),
            model: m.to_string(),
            extra_env: Map::new(),
            context_window: None,
            created_at: String::new(),
        };
        cat.providers = vec![mk("roster-prov", "roster-model"), mk("glm", "glm-4")];
        cat.roster.insert(
            "dev".into(),
            crate::providers::RosterEntry { provider_id: "roster-prov".into(), model: None },
        );

        let mut config = crate::config::AppConfig::default();
        config.default_backend = "claude-code".to_string();
        config.default_cwd = cwd.to_string_lossy().into_owned();

        // 按任务给出的覆盖去构造"显式指定"
        let (p, m) = task.route_override();
        let req = crate::spawn_plan::SpawnRequest {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            agent: Some(task.owner.clone()),
            provider_id: p.map(str::to_string),
            model: m.map(str::to_string),
            ..Default::default()
        };
        let plan = crate::spawn_plan::plan(&req, &config, &cat);
        assert_eq!(plan.route_source, "requested", "任务级覆盖等同于显式指定");
        assert_eq!(plan.provider_id.as_deref(), Some("glm"), "必须压过花名册");
        assert_eq!(plan.model, "glm-special");
        assert_eq!(plan.env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("key-glm"));
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
