//! Agent 身份解析与注入 —— 「@name 真派发」的后端。
//!
//! 目标（见 docs/2026-10-14-console-product-spec.md §3.2）：
//! `@architect` 应当**开出一个以该身份运行的 Slot**（自己的 system prompt、
//! 自己的 transcript、自己的权限姿态），而不是往同一个 PTY 里打 `@architect ` 文本。
//!
//! 本模块只负责两件事：
//! 1. 把 `.claude/agents/<id>.md` 解析成 [`AgentDef`]；
//! 2. 把身份拼成 system prompt，并写成一个**文件**交给引擎
//!    （不放进 argv：避免长文本被截断，也避免内容出现在 `ps` 里）。
//!
//! 注入方式由后端能力决定（`backend::BackendProfile::system_prompt_file_flag`）。
//! 后端未声明该能力时返回 [`Injection::Unsupported`]，**调用方必须记录/显示降级**，
//! 不允许静默假装已经注入。

use std::path::{Path, PathBuf};

use crate::cc_status::{fm_field, frontmatter_split, read_text};

/// 一个 agent 定义（frontmatter + 正文）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentDef {
    /// 文件名（不含 `.md`），也是 `@` 引用的 id。
    pub id: String,
    /// frontmatter `name`
    pub name: String,
    pub description: String,
    pub tools: String,
    pub model: String,
    pub color: String,
    pub icon: String,
    /// frontmatter `permission:`（**本产品的扩展键**，Claude Code 没有这一项）。
    /// 取值：`bypassPermissions` / `acceptEdits` / `default` / `planMode`。
    /// 空 = 不干预，沿用后端默认（不改变既有行为）。
    pub permission: String,
    /// frontmatter `skills:`（**本产品的扩展键**）—— 能力组合里的"技能"。
    /// 逗号或空格分隔的技能名。
    pub skills: String,
    /// 定义来自哪个 scope：`project` | `onecode` | `user`。
    pub scope: String,
    /// frontmatter 之后的正文 —— 这**就是**该 agent 的 system prompt。
    pub body: String,
}

impl AgentDef {
    /// 把 frontmatter 的 `tools:` 解析成工具白名单。
    ///
    /// Claude Code 的官方说明是"逗号或空格分隔"（`--allowedTools` 帮助文本原文：
    /// 'Comma or space-separated list of tool names to allow'），且工具名可能带
    /// 括号限定，如 `Bash(git:*)` —— 所以有逗号就按逗号切，否则按空白切，
    /// **不在括号内切**。
    pub fn allowed_tools(&self) -> Vec<String> {
        split_names(&self.tools)
    }

    /// 把 frontmatter 的 `skills:` 解析成技能名列表。
    pub fn skill_names(&self) -> Vec<String> {
        split_names(&self.skills)
    }
}

/// 逗号优先、否则按空白切分的名字列表。有逗号就按逗号切（保护 `Bash(git:*)` 这类
/// 带括号限定的 token 不被空格切开）。
fn split_names(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = if raw.contains(',') {
        raw.split(',').collect()
    } else {
        raw.split_whitespace().collect()
    };
    parts
        .into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 一个已找到的技能（目录项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub source: PathBuf,
    pub body_bytes: usize,
}

/// 正文已内联进 system prompt 的技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinedSkill {
    pub name: String,
    pub source: PathBuf,
    pub body: String,
    pub bytes: usize,
}

/// 技能加载结果 —— **必须区分三种情况**，任何一种都不能静默：
/// 没找到 / 已内联 / 因超预算而"给路径让模型按需读"。
///
/// 为什么不是"超预算就丢掉"：本机实测（2026-10-14）真实技能体量远超预期 ——
/// `~/.agents/skills` 最大 **127KB**、多个 70~112KB；项目 `.claude/skills` 最大 33KB。
/// 一个 127KB 的 SKILL.md ≈ 32k token，**整体注入会把上下文吃掉一大块**；
/// 而**截断**会给出半截指令，比不加载更糟。
/// 所以采用引擎自己的**渐进披露**：目录（名字+描述）永远给，正文按预算内联，
/// 超预算的给出**文件路径**，让模型需要时用自己的 Read 工具去读。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillLoadReport {
    /// 所有找到的技能（名字 + 描述 + 路径），永远注入
    pub catalog: Vec<SkillEntry>,
    /// 正文已内联的技能
    pub inlined: Vec<InlinedSkill>,
    /// 找到但正文超预算、只给路径的技能
    pub deferred: Vec<SkillEntry>,
    /// 在所有约定根目录里都找不到的技能名
    pub missing: Vec<String>,
}

/// 技能根目录，按优先级（项目 → onecode 隔离全局 → 用户全局）。
///
/// 说明（本机实测得出）：
/// - Claude Code 自己只认 `<project>/.claude/skills` 与 `~/.claude/skills`
///   （官方帮助串原文："`.claude/skills/ in your project or ~/.claude/skills/`"），
///   **不读 `.agents/skills`**；
/// - 而 `.agents/skills` 是 Codex / Gemini CLI / OpenCode 的共同根。
/// 所以这里把两套都收进来：**用户放在 `.agents/skills` 的技能也能被员工的技能组合用到**
/// （本机 `~/.agents/skills` 有 53 个，而 Claude Code 对它们完全无感）。
fn skill_dirs(project_cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(cwd) = project_cwd {
        dirs.push(cwd.join(".claude").join("skills"));
        dirs.push(cwd.join(".agents").join("skills"));
    }
    // 引擎原生 user 目录（被 CLAUDE_CONFIG_DIR 重定位到这里）
    dirs.push(crate::providers::cc_config_dir().join("skills"));
    if let Some(home) = home_dir() {
        dirs.push(home.join(".claude").join("skills"));
        dirs.push(home.join(".agents").join("skills"));
    }
    dirs
}

/// 加载技能：约定形态 `<root>/<name>/SKILL.md`，兼容扁平 `<root>/<name>.md`。
///
/// `inline_budget_bytes` 是**正文内联总量上限**（目录不受它限制）。
/// 超预算的技能不会被丢弃，而是进入 `deferred`（给路径让模型按需读）。
pub fn resolve_skills(
    project_cwd: Option<&Path>,
    names: &[String],
    inline_budget_bytes: usize,
) -> SkillLoadReport {
    let mut report = SkillLoadReport::default();
    if names.is_empty() {
        return report;
    }
    let dirs = skill_dirs(project_cwd);
    let mut used = 0usize;

    for name in names {
        if !is_safe_id(name) {
            log::warn!("[agents] rejected unsafe skill name {name:?}");
            report.missing.push(name.clone());
            continue;
        }
        let mut found: Option<(PathBuf, String)> = None;
        for dir in &dirs {
            let candidates = [dir.join(name).join("SKILL.md"), dir.join(format!("{name}.md"))];
            for path in candidates {
                if let Some(content) = read_text(&path) {
                    found = Some((path, content));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        let Some((path, content)) = found else {
            log::warn!("[agents] skill {name:?} not found in any skill root");
            report.missing.push(name.clone());
            continue;
        };

        // 技能正文：frontmatter 之后的部分 + description 头
        let (fm, body) = frontmatter_split(&content).unwrap_or_default();
        let description = fm_field(&fm, "description");
        let body = body.trim().to_string();

        let mut text = format!("## Skill: {name}\n");
        if !description.is_empty() {
            text.push_str(&format!("{description}\n"));
        }
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
            text.push('\n');
        }

        let entry = SkillEntry {
            name: name.clone(),
            description,
            source: path.clone(),
            body_bytes: text.len(),
        };

        if used + text.len() > inline_budget_bytes {
            // 超预算 → 不丢弃，改为"给路径让模型按需读"（引擎自己的渐进披露）
            log::info!(
                "[agents] skill {name:?} deferred ({}B > remaining inline budget; used {used}); path handed to the model",
                text.len()
            );
            report.catalog.push(entry.clone());
            report.deferred.push(entry);
            continue;
        }
        used += text.len();
        log::info!("[agents] skill {name:?} inlined from {}", path.display());
        report.catalog.push(entry);
        report.inlined.push(InlinedSkill {
            name: name.clone(),
            source: path,
            bytes: text.len(),
            body: text,
        });
    }

    report
}

/// 由已加载技能与正文拼出注入文本的**稳定文件名键**。
///
/// 为什么不是直接用 agent id 当文件名：同一个 agent 在不同项目里会解析到
/// **不同的**技能文件（项目优先），两个槽并发 spawn 会写同一个路径 → 内容互相覆盖。
/// 用正文的 FNV-1a 摘要做后缀，内容不同就是不同文件，内容相同则共享（无害）。
pub fn prompt_file_key(agent_id: &str, prompt: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in prompt.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{agent_id}-{:016x}", hash)
}

/// id 白名单：字母数字与 `._-`，且不允许 `..`。
///
/// id 来自前端，必须当作不可信输入 —— 它会被拼进文件路径。
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 候选目录，按优先级：项目 → onecode 隔离全局 → 用户全局。
///
/// 说明：进程以 `CLAUDE_CONFIG_DIR=<onecode cc-config>` 启动，所以 claude 自身
/// 只看前两个 scope；第三个（`$HOME/.claude/agents`）是为了兼容用户已有的全局
/// agent —— 我们是自己把正文注入进去的，所以不必依赖引擎去发现它。
fn candidate_dirs(project_cwd: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut dirs: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(cwd) = project_cwd {
        dirs.push((cwd.join(".claude").join("agents"), "project"));
    }
    dirs.push((crate::providers::cc_config_dir().join("agents"), "onecode"));
    if let Some(home) = home_dir() {
        dirs.push((home.join(".claude").join("agents"), "user"));
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// 解析 agent 定义。找不到返回 `None`。
pub fn resolve_agent(project_cwd: Option<&Path>, id: &str) -> Option<AgentDef> {
    if !is_safe_id(id) {
        log::warn!("[agents] rejected unsafe agent id: {id:?}");
        return None;
    }
    for (dir, scope) in candidate_dirs(project_cwd) {
        let path = dir.join(format!("{id}.md"));
        let Some(content) = read_text(&path) else {
            continue;
        };
        match parse_agent(id, scope, &content) {
            Some(def) => {
                log::info!("[agents] resolved @{id} from {scope} ({})", path.display());
                return Some(def);
            }
            None => {
                log::warn!("[agents] {id} at {} has no usable frontmatter, skipped", path.display());
            }
        }
    }
    log::warn!("[agents] @{id} not found in any scope");
    None
}

/// 从文件内容解析 agent。frontmatter 必须有非空 `name`，否则视为无效
/// （与 `cc_status::load_agents` 的判定保持一致）。
pub fn parse_agent(id: &str, scope: &str, content: &str) -> Option<AgentDef> {
    let (fm, body) = frontmatter_split(content)?;
    let name = fm_field(&fm, "name");
    if name.is_empty() {
        return None;
    }
    Some(AgentDef {
        id: id.to_string(),
        name,
        description: fm_field(&fm, "description"),
        tools: fm_field(&fm, "tools"),
        model: fm_field(&fm, "model"),
        color: fm_field(&fm, "color"),
        icon: fm_field(&fm, "icon"),
        permission: fm_field(&fm, "permission"),
        skills: fm_field(&fm, "skills"),
        scope: scope.to_string(),
        body,
    })
}

/// 拼出注入给模型的 system prompt，**并把员工的技能组合附加在后面**。
///
/// 正文为空时用 name/description 兜底 —— 否则"以某身份启动"会变成一个空操作。
///
/// 技能为什么要走"注入 system prompt"而不是引擎原生加载：
/// 本机官方 bundle 里**没有任何"指定技能目录"的 flag**（只有 `--add-dir`），
/// 而 Claude Code 读技能的目录（`<project>/.claude/skills`、`~/.claude/skills`）
/// 都是**全局共享**的、无法按槽区分。所以 per-slot 的技能组合只能靠注入。
/// 这是**如实的能力降级**，不是假装引擎原生支持。
pub fn compose_system_prompt_with_skills(def: &AgentDef, skills: &SkillLoadReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Agent: {}\n", def.name));
    if !def.description.is_empty() {
        out.push_str(&format!("{}\n", def.description));
    }
    if def.body.trim().is_empty() {
        // 正文缺失：至少把身份说清楚，并如实标注，避免模型以为自己有专属指令。
        out.push_str(
            "\n(此 agent 定义只有 frontmatter、没有正文指令。按上面的身份与描述工作。)\n",
        );
    } else {
        out.push('\n');
        out.push_str(def.body.trim());
        out.push('\n');
    }

    if !skills.catalog.is_empty() {
        // ── 技能：渐进披露 ────────────────────────────────────────────
        // 1) 目录永远给（名字 + 描述 + 路径）—— 便宜，且让模型知道"有哪些能力可用"；
        // 2) 正文在预算内直接内联，省掉一次工具往返；
        // 3) 超预算的技能只给路径，模型需要时用自己的 Read 工具读 ——
        //    比截断（给出半截指令）诚实，也比整体注入（吃掉几万 token）省。
        out.push_str("\n---\n\n# 技能\n\n");
        out.push_str("下面是这个员工的技能目录。**正文已内联的可以直接用**；");
        out.push_str("标注了「需读取」的，在真正要用它之前先用 Read 工具读那个 SKILL.md。");
        out.push_str("与上面的身份冲突时，以身份为准。\n\n");
        for e in &skills.catalog {
            let marker = if skills.inlined.iter().any(|s| s.name == e.name) {
                "正文已内联"
            } else {
                "需读取"
            };
            out.push_str(&format!(
                "- **{}** — {}{}\n",
                e.name,
                if e.description.is_empty() { "(无描述)" } else { &e.description },
                format_args!("（{marker}）")
            ));
        }
        if !skills.deferred.is_empty() {
            out.push_str("\n需读取的技能文件路径：\n");
            for e in &skills.deferred {
                out.push_str(&format!(
                    "- {} — `{}`（{} 字节）\n",
                    e.name,
                    e.source.display(),
                    e.body_bytes
                ));
            }
        }
        if !skills.inlined.is_empty() {
            out.push_str("\n## 已内联的技能正文\n\n");
            for s in &skills.inlined {
                out.push_str(s.body.trim_end());
                out.push_str("\n\n");
            }
        }
    }

    // ── 产出协议（目标 ④）────────────────────────────────────────────
    // 引擎不会自动告诉我们"这次交付了什么"，所以**必须教会员工写产出记录**，
    // 否则"管理产出"就只能靠捡 git 改动。协议正文只有一份实现
    // （`deliverables::protocol_section`），文档与实现不会漂移。
    out.push_str("\n---\n\n");
    out.push_str(&crate::deliverables::protocol_section());

    // ── 任务协议（目标 ⑤）────────────────────────────────────────────
    // 同理：不教会员工写任务记录，"管理工作"在板上就是空的。
    // 协议里明确写了"不要自己写 done" —— 完成由人裁决。
    out.push_str("\n---\n\n");
    out.push_str(&crate::tasks::protocol_section());
    out
}

/// 把 system prompt 写进 `<dir>/<key>.md`（原子替换：先写临时文件再 rename）。
///
/// `key` 必须已通过 [`is_safe_id`] 校验。
pub fn write_prompt_file(dir: &Path, key: &str, text: &str) -> std::io::Result<PathBuf> {
    if !is_safe_id(key) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe prompt key: {key:?}"),
        ));
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{key}.md"));
    let tmp = dir.join(format!(".{key}.md.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: Architect\ndescription: 技术选型\ntools: Read, Bash\nmodel: opus\ncolor: indigo\nicon: 🏛\n---\n你是架构师，先评审再动手。";

    #[test]
    fn parse_agent_reads_frontmatter_and_body() {
        let d = parse_agent("architect", "project", SAMPLE).expect("应解析成功");
        assert_eq!(d.name, "Architect");
        assert_eq!(d.tools, "Read, Bash");
        assert_eq!(d.color, "indigo");
        assert_eq!(d.body, "你是架构师，先评审再动手。");
    }

    #[test]
    fn parse_agent_rejects_missing_name() {
        assert!(parse_agent("x", "project", "---\ndescription: no name\n---\nbody").is_none());
        assert!(parse_agent("x", "project", "no frontmatter").is_none());
    }

    #[test]
    fn parse_agent_tolerates_empty_leading_block() {
        // 真实文件形态（~/.claude/agents/dev.md）
        let c = "---\n---\nname: dev\ndescription: AI 开发者\n---\n正文";
        let d = parse_agent("dev", "user", c).expect("应跳过空块后解析成功");
        assert_eq!(d.name, "dev");
        assert_eq!(d.body, "正文");
    }

    #[test]
    fn compose_includes_name_and_body() {
        let d = parse_agent("architect", "project", SAMPLE).unwrap();
        let p = compose_system_prompt_with_skills(&d, &SkillLoadReport::default());
        assert!(p.contains("# Agent: Architect"));
        assert!(p.contains("你是架构师"));
    }

    #[test]
    fn compose_falls_back_when_body_empty() {
        let d = parse_agent("x", "project", "---\nname: X\n---\n").unwrap();
        let p = compose_system_prompt_with_skills(&d, &SkillLoadReport::default());
        assert!(p.contains("# Agent: X"));
        assert!(p.contains("没有正文指令"));
    }

    #[test]
    fn id_guard_blocks_traversal_and_separators() {
        assert!(is_safe_id("architect"));
        assert!(is_safe_id("vp-eng.v2"));
        assert!(!is_safe_id(""));
        assert!(!is_safe_id(".."));
        assert!(!is_safe_id("../../etc/passwd"));
        assert!(!is_safe_id("a/b"));
        assert!(!is_safe_id("a\\b"));
        assert!(!is_safe_id("a b"));
    }

    #[test]
    fn resolve_rejects_unsafe_id() {
        assert!(resolve_agent(None, "../evil").is_none());
    }

    // ── 员工级权限姿态（目标 ③：员工 = 身份 + 模型 + 权限） ─────────────

    #[test]
    fn allowed_tools_splits_commas_and_whitespace() {
        let d = parse_agent("x", "p", "---\nname: X\ntools: Read, Write, Bash\n---\nb").unwrap();
        assert_eq!(d.allowed_tools(), vec!["Read", "Write", "Bash"]);

        let d2 = parse_agent("x", "p", "---\nname: X\ntools: Read Write Bash\n---\nb").unwrap();
        assert_eq!(d2.allowed_tools(), vec!["Read", "Write", "Bash"]);
    }

    #[test]
    fn allowed_tools_keeps_parenthesised_specs_intact() {
        // 官方示例形态：Bash(git:*) Edit —— 括号内不能切
        let d = parse_agent("x", "p", "---\nname: X\ntools: Bash(git:*), Edit\n---\nb").unwrap();
        assert_eq!(d.allowed_tools(), vec!["Bash(git:*)", "Edit"]);
    }

    #[test]
    fn allowed_tools_empty_when_not_declared() {
        let d = parse_agent("x", "p", "---\nname: X\n---\nb").unwrap();
        assert!(d.allowed_tools().is_empty());
        let d2 = parse_agent("x", "p", "---\nname: X\ntools:   \n---\nb").unwrap();
        assert!(d2.allowed_tools().is_empty());
    }

    #[test]
    fn permission_field_is_parsed_verbatim() {
        let d = parse_agent("x", "p", "---\nname: X\npermission: acceptEdits\n---\nb").unwrap();
        assert_eq!(d.permission, "acceptEdits");
        let d2 = parse_agent("x", "p", "---\nname: X\n---\nb").unwrap();
        assert_eq!(d2.permission, "", "未声明即空 —— 不干预");
    }

    /// 端到端（用**本机真实 agent 文件**）：frontmatter → 参数。
    ///
    /// 覆盖两个纯函数接起来的那一段：真实的 `tools:` 必须变成真实的白名单参数，
    /// 且默认的 `--permission-mode bypassPermissions` 必须被整段替换掉
    /// （否则白名单在 bypass 下形同虚设 —— 就是"门装了但没锁"）。
    /// 无该目录时跳过（CI 上不适用）。
    #[test]
    fn real_agent_declaration_becomes_real_args() {
        let Some(home) = home_dir() else { return };
        let dir = home.join(".claude").join("agents");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let profile = crate::backend::resolve_or_default("claude-code");

        let mut checked = 0;
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            let content = std::fs::read_to_string(&path).unwrap();
            let Some(def) = parse_agent(&id, "user", &content) else { continue };
            let tools = def.allowed_tools();
            if tools.is_empty() {
                continue; // 没声明 tools 的不在这个测试范围内
            }
            let posture = crate::backend::Posture::parse(&def.permission)
                .unwrap_or(crate::backend::Posture::Inherit);
            let mut args: Vec<String> = profile.default_args.iter().map(|s| s.to_string()).collect();
            let out = crate::backend::apply_posture_args(&profile, posture, &tools, &mut args);

            assert_eq!(out.tools_applied, tools, "真实 {id}: 白名单必须完整落地");
            assert!(out.mode_applied.is_some(), "真实 {id}: 声明了 tools 就必须收窄档位");
            assert!(
                !args.contains(&"bypassPermissions".to_string()),
                "真实 {id}: 默认的 bypass 必须被替换，否则白名单无效"
            );
            assert_eq!(
                args.iter().filter(|a| *a == "--allowedTools").count(),
                1,
                "真实 {id}: 白名单 flag 只能出现一次"
            );
            eprintln!("[agents] real e2e: {id} -> posture={:?} tools={:?}", out.mode_applied, out.tools_applied);
            checked += 1;
        }
        eprintln!("[agents] real e2e checked {checked} agent(s)");
    }

    #[test]
    fn write_prompt_file_is_atomic_and_guarded() {
        let dir = std::env::temp_dir().join(format!("dsh-agents-test-{}", std::process::id()));
        let p = write_prompt_file(&dir, "architect", "hello").expect("写入应成功");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        // 覆盖写：内容替换而不是追加
        write_prompt_file(&dir, "architect", "world").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "world");
        // 临时文件不残留
        assert!(!dir.join(".architect.md.tmp").exists());
        assert!(write_prompt_file(&dir, "../evil", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 针对**本机真实文件**的门控测试：`~/.claude/agents/*.md` 必须能解析出非空 name。
    ///
    /// 存在的理由：正是 `~/.claude/agents/dev.md` 开头那个多余的 `---/---` 空块
    /// 让旧解析器把 frontmatter 解析成空、整个 agent 被静默跳过。这个测试把
    /// "真实文件必须可用"钉住。无该目录时跳过（CI 上不适用）。
    #[test]
    fn real_user_agents_parse() {
        let Some(home) = home_dir() else { return };
        let dir = home.join(".claude").join("agents");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return; // 无该目录 → 跳过
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            let content = std::fs::read_to_string(&path).unwrap();
            let def = parse_agent(&id, "user", &content)
                .unwrap_or_else(|| panic!("真实 agent {id} 解析失败（frontmatter 无 name？）"));
            assert!(!def.name.is_empty(), "真实 agent {id} 的 name 为空");
            checked += 1;
        }
        eprintln!("[agents] real-user-agents parse check: {checked} file(s)");
    }

    // ── 能力组合：技能（frontmatter `skills:` → 注入正文） ─────────────

    fn tmp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-skills-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_skill(dir: &Path, name: &str, desc: &str, body: &str) {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn skill_names_split_like_tools() {
        let d = parse_agent("x", "p", "---\nname: X\nskills: a, b, c\n---\nb").unwrap();
        assert_eq!(d.skill_names(), vec!["a", "b", "c"]);
        let d2 = parse_agent("x", "p", "---\nname: X\nskills: a b c\n---\nb").unwrap();
        assert_eq!(d2.skill_names(), vec!["a", "b", "c"]);
        let d3 = parse_agent("x", "p", "---\nname: X\n---\nb").unwrap();
        assert!(d3.skill_names().is_empty());
    }

    #[test]
    fn resolve_skills_loads_from_project_root() {
        let root = tmp_root("proj");
        let skills_dir = root.join(".claude").join("skills");
        write_skill(&skills_dir, "alpha", "第一个技能", "做 A 的步骤");
        let report = resolve_skills(Some(&root), &["alpha".to_string()], 32 * 1024);
        assert_eq!(report.missing, Vec::<String>::new());
        assert_eq!(report.inlined.len(), 1);
        assert_eq!(report.inlined[0].name, "alpha");
        assert!(report.inlined[0].body.contains("做 A 的步骤"));
        assert!(report.inlined[0].body.contains("第一个技能"), "description 要带上");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_skill_wins_over_agents_root() {
        // 优先级：<cwd>/.claude/skills 先于 <cwd>/.agents/skills
        let root = tmp_root("prio");
        write_skill(&root.join(".agents").join("skills"), "beta", "agents 版", "AGENTS 正文");
        write_skill(&root.join(".claude").join("skills"), "beta", "claude 版", "CLAUDE 正文");
        let report = resolve_skills(Some(&root), &["beta".to_string()], 32 * 1024);
        assert!(report.inlined[0].body.contains("CLAUDE 正文"), "项目 .claude 应优先");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn agents_root_is_searched_too() {
        // 这是关键：Claude Code 自己不读 .agents/skills，但我们的员工技能组合要读它
        let root = tmp_root("agentsroot");
        write_skill(&root.join(".agents").join("skills"), "gamma", "d", "GAMMA 正文");
        let report = resolve_skills(Some(&root), &["gamma".to_string()], 32 * 1024);
        assert!(report.inlined[0].body.contains("GAMMA 正文"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flat_skill_md_is_accepted() {
        let root = tmp_root("flat");
        let dir = root.join(".claude").join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("delta.md"), "---\nname: delta\n---\nFLAT 正文").unwrap();
        let report = resolve_skills(Some(&root), &["delta".to_string()], 32 * 1024);
        assert!(report.inlined[0].body.contains("FLAT 正文"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_skills_are_reported_not_silently_dropped() {
        let root = tmp_root("miss");
        let report = resolve_skills(
            Some(&root),
            &["definitely-not-a-real-skill-xyz".to_string()],
            32 * 1024,
        );
        assert!(report.inlined.is_empty());
        assert_eq!(report.missing, vec!["definitely-not-a-real-skill-xyz"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsafe_skill_name_is_rejected_as_missing() {
        let root = tmp_root("unsafe");
        let report = resolve_skills(Some(&root), &["../evil".to_string()], 32 * 1024);
        assert!(report.inlined.is_empty());
        assert_eq!(report.missing, vec!["../evil"], "路径穿越必须走 missing，不得读取");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn over_budget_skills_are_skipped_and_reported() {
        let root = tmp_root("budget");
        let dir = root.join(".claude").join("skills");
        write_skill(&dir, "small", "d", "小");
        write_skill(&dir, "huge", "d", &"X".repeat(5000));
        // 预算只够第一个
        let report = resolve_skills(Some(&root), &["small".to_string(), "huge".to_string()], 3000);
        assert_eq!(report.inlined.len(), 1);
        assert_eq!(report.inlined[0].name, "small");
        assert_eq!(
            report.deferred.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            vec!["huge"],
            "超预算必须走 deferred（给路径），不能静默丢"
        );
        assert!(report.catalog.iter().any(|c| c.name == "huge"), "目录里仍要有它");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compose_appends_loaded_skills_after_the_persona() {
        let d = parse_agent("x", "p", "---\nname: Arch\n---\n你是架构师。").unwrap();
        let report = {
            let root = tmp_root("compose");
            write_skill(&root.join(".claude").join("skills"), "alpha", "d", "技能正文");
            let r = resolve_skills(Some(&root), &["alpha".to_string()], 32 * 1024);
            let _ = std::fs::remove_dir_all(&root);
            r
        };
        let p = compose_system_prompt_with_skills(&d, &report);
        let persona_at = p.find("你是架构师").expect("身份必须在");
        let skill_at = p.find("技能正文").expect("技能必须在");
        assert!(persona_at < skill_at, "技能要附在身份之后");
        assert!(p.contains("## Skill: alpha"));
    }

    #[test]
    fn prompt_file_key_is_stable_and_content_sensitive() {
        // 稳定性：同内容 → 同文件名（避免每次 spawn 都堆积新文件）
        assert_eq!(prompt_file_key("arch", "abc"), prompt_file_key("arch", "abc"));
        // 敏感性：内容不同 → 文件名不同（否则两个项目会互相覆盖）
        assert_ne!(prompt_file_key("arch", "abc"), prompt_file_key("arch", "abd"));
        // 不同 agent 同内容也要区分
        assert_ne!(prompt_file_key("arch", "abc"), prompt_file_key("dev", "abc"));
        // 文件名必须过安全校验（它会被拼进路径）
        assert!(is_safe_id(&prompt_file_key("arch", "abc")));
    }

    /// 真实数据：本机 `~/.agents/skills` 里的技能必须能被解析到。
    /// 这条把"那 53 个 Claude Code 看不见的技能，我们的员工能用上"钉住。
    #[test]
    fn real_agents_skills_are_resolvable() {
        let Some(home) = home_dir() else { return };
        let dir = home.join(".agents").join("skills");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("SKILL.md").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        if names.is_empty() {
            return;
        }
        names.sort();
        let pick = vec![names[0].clone(), names[names.len() - 1].clone()];
        let report = resolve_skills(None, &pick, 64 * 1024);
        eprintln!("[agents] real skills: loaded={:?} missing={:?} deferred={:?}",
            report.inlined.iter().map(|s| &s.name).collect::<Vec<_>>(),
            report.missing,
            report.deferred.iter().map(|s| &s.name).collect::<Vec<_>>());
        assert_eq!(report.missing, Vec::<String>::new(), "真实技能必须都能解析到");
        assert_eq!(report.inlined.len(), pick.len());
        for s in &report.inlined {
            assert!(s.bytes > 0);
            assert!(s.body.contains("## Skill:"));
        }
    }

    /// 最强的一条端到端（全部用**本机真实数据**）：
    /// 真实 agent 定义 + 真实技能 → 最终注入给模型的 system prompt。
    ///
    /// 无真实数据时跳过（CI 上不适用）。
    #[test]
    fn real_agent_plus_real_skill_composes_into_one_prompt() {
        let Some(home) = home_dir() else { return };
        let agents = home.join(".claude").join("agents");
        let Ok(entries) = std::fs::read_dir(&agents) else { return };
        // 找一个真实 agent
        let mut def = None;
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            let content = std::fs::read_to_string(&path).unwrap();
            if let Some(d) = parse_agent(&id, "user", &content) {
                def = Some(d);
                break;
            }
        }
        let Some(def) = def else { return };

        // 找一个真实技能（~/.agents/skills 里的第一个）
        let sdir = home.join(".agents").join("skills");
        let Ok(sentries) = std::fs::read_dir(&sdir) else { return };
        let mut skill_name = None;
        for e in sentries.flatten() {
            if e.path().join("SKILL.md").exists() {
                skill_name = Some(e.file_name().to_string_lossy().into_owned());
                break;
            }
        }
        let Some(skill_name) = skill_name else { return };

        let report = resolve_skills(None, &[skill_name.clone()], 64 * 1024);
        assert_eq!(report.missing, Vec::<String>::new());

        let prompt = compose_system_prompt_with_skills(&def, &report);
        assert!(prompt.contains(&format!("# Agent: {}", def.name)), "身份必须在");
        assert!(prompt.contains(&format!("## Skill: {skill_name}")), "技能必须在");
        // 身份要排在技能之前（冲突时以身份为准）
        assert!(prompt.find(&format!("# Agent: {}", def.name)).unwrap()
            < prompt.find(&format!("## Skill: {skill_name}")).unwrap());
        // 文件名键可用且稳定
        let key = prompt_file_key(&def.id, &prompt);
        assert!(is_safe_id(&key));
        assert_eq!(key, prompt_file_key(&def.id, &prompt));
        eprintln!(
            "[agents] real compose: agent={} skill={} prompt={}B key={key}",
            def.id, skill_name, prompt.len()
        );
    }

    // ── 渐进披露：目录永远给，正文按预算内联，超预算给路径 ──────────────

    #[test]
    fn deferred_skill_keeps_catalog_entry_and_hands_over_the_path() {
        let root = tmp_root("defer");
        let dir = root.join(".claude").join("skills");
        write_skill(&dir, "small", "小技能", "小正文");
        write_skill(&dir, "big", "大技能", &"Y".repeat(8000));
        let report = resolve_skills(Some(&root), &["small".to_string(), "big".to_string()], 2000);

        // 目录里两个都在（模型知道有什么能力可用）
        assert_eq!(report.catalog.len(), 2);
        assert_eq!(report.inlined.len(), 1);
        assert_eq!(report.deferred.len(), 1);
        assert_eq!(report.deferred[0].name, "big");

        let d = parse_agent("x", "p", "---\nname: X\n---\n身份正文").unwrap();
        let prompt = compose_system_prompt_with_skills(&d, &report);
        // 未内联的技能必须给出**路径**，让模型自己读
        assert!(prompt.contains("需读取"), "目录要标出未内联");
        assert!(
            prompt.contains(&report.deferred[0].source.display().to_string()),
            "必须把 SKILL.md 路径交给模型"
        );
        assert!(prompt.contains("小正文"), "内联的技能正文要在");
        assert!(!prompt.contains("YYYYYYYY"), "超预算的正文不该被塞进来");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn catalog_is_injected_even_when_nothing_fits_the_budget() {
        let root = tmp_root("nofit");
        write_skill(&root.join(".claude").join("skills"), "huge", "很大的技能", &"Z".repeat(9000));
        let report = resolve_skills(Some(&root), &["huge".to_string()], 10);
        assert!(report.inlined.is_empty());
        assert_eq!(report.catalog.len(), 1, "一个都内联不了时目录仍必须给");
        let d = parse_agent("x", "p", "---\nname: X\n---\nb").unwrap();
        let prompt = compose_system_prompt_with_skills(&d, &report);
        assert!(prompt.contains("huge"));
        assert!(prompt.contains("很大的技能"), "描述也要给，模型才能判断要不要读");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真实数据：本机最大的那个技能在**生产预算**下必须走 deferred（给路径），
    /// 而不是被丢弃、也不是被截断塞进来。
    #[test]
    fn real_largest_skill_defers_under_production_budget() {
        let Some(home) = home_dir() else { return };
        let dir = home.join(".agents").join("skills");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        // 找最大的一个
        let mut biggest: Option<(String, u64)> = None;
        for e in entries.flatten() {
            let sk = e.path().join("SKILL.md");
            if let Ok(md) = std::fs::metadata(&sk) {
                let n = e.file_name().to_string_lossy().into_owned();
                if biggest.as_ref().map(|(_, s)| md.len() > *s).unwrap_or(true) {
                    biggest = Some((n, md.len()));
                }
            }
        }
        let Some((name, size)) = biggest else { return };
        if size <= 32 * 1024 {
            return; // 本机没有超预算的技能，跳过
        }
        let report = resolve_skills(None, &[name.clone()], 32 * 1024);
        assert_eq!(report.missing, Vec::<String>::new());
        assert_eq!(report.catalog.len(), 1, "必须在目录里");
        assert!(report.inlined.is_empty(), "真实大技能不该被整体内联（{size}B）");
        assert_eq!(report.deferred.len(), 1, "必须走 deferred 给路径");
        assert_eq!(report.deferred[0].body_bytes as u64 > 32 * 1024, true);
        eprintln!(
            "[agents] real largest skill: {name} = {size}B -> deferred (path handed to model)"
        );
    }

    /// 产出协议必须真的出现在注入文本里 —— 否则员工根本不知道要写产出记录，
    /// 「管理产出」就只能靠捡 git 改动。
    #[test]
    fn composed_prompt_carries_the_deliverable_protocol() {
        let d = parse_agent("x", "p", "---\nname: X\n---\n身份正文").unwrap();
        let p = compose_system_prompt_with_skills(&d, &SkillLoadReport::default());
        assert!(p.contains("产出协议"), "协议标题必须在");
        assert!(p.contains(crate::deliverables::DIR), "协议要给出真实路径");
        assert!(p.contains("不是证据") || p.contains("not-evidence"), "要写明自述不算证据");
        // 身份在前，协议在后（冲突时以身份为准）
        assert!(p.find("身份正文").unwrap() < p.find("产出协议").unwrap());
    }

    /// 任务协议也必须进注入文本，且要写明"不要自己写 done"。
    #[test]
    fn composed_prompt_carries_the_task_protocol_too() {
        let d = parse_agent("x", "p", "---\nname: X\n---\n身份正文").unwrap();
        let p = compose_system_prompt_with_skills(&d, &SkillLoadReport::default());
        assert!(p.contains("任务协议"), "任务协议标题必须在");
        assert!(p.contains(crate::tasks::DIR), "要给真实路径");
        assert!(p.contains("不要写 `done`") || p.contains("不要写 done"), "必须写明不能自报完成");
        // 身份 → 产出协议 → 任务协议
        let a = p.find("身份正文").unwrap();
        let b = p.find("产出协议").unwrap();
        let c = p.find("任务协议").unwrap();
        assert!(a < b && b < c, "顺序应是 身份 → 产出协议 → 任务协议");
    }
}
