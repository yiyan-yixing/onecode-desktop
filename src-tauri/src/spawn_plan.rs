//! spawn 计划：把"点击一个员工 → 到底起什么进程、带什么参数与环境"整段做成**纯函数**。
//!
//! 为什么单独成模块：这段逻辑以前住在 `commands::pty_spawn` 的函数体里 —— 而它是**整个
//! 产品最核心的一条链路**（身份注入 / 权限姿态 / 技能组合 / 每槽路由 / failover 全在这里
//! 汇合），却因为依赖 `State<...>` 而**一个测试都没有**。抽出来之后它可以被直接单测，
//! 也可以**真的用来起一个桩引擎**、验证参数与环境确实到达了进程。
//!
//! 与旧实现的两个差别（都是刻意的）：
//! 1. **不就地改 catalog**。failover 历史要记什么，通过 [`SpawnPlan::switch_record`]
//!    交回调用方 —— 纯函数才好测。
//! 2. 一处的提示文案顺手改正：`permission` 取值列表里原来还写着 `planMode`，
//!    而 CLI 实测只接受 `plan`（第 11 刀修掉了值，这句文案当时漏了）。

use std::collections::HashMap;
use std::path::Path;

use crate::backend::BackendProfile;
use crate::config::AppConfig;
use crate::providers::{self, ProviderCatalog};

/// 员工技能注入的**正文内联**总字节上限（技能目录不受此限制）。
///
/// 32 KiB ≈ 8k token。这个数不是拍的 —— 本机实测（2026-10-14）技能体量：
/// `~/.agents/skills` 最大 **127 KB**、多个 70~112 KB；项目 `.claude/skills` 最大 33 KB。
/// 也就是说**真实技能普遍超出 32 KiB**，所以"超预算"是**常态路径**而非异常：
/// 目录（名字+描述+路径）永远注入，正文在预算内内联，超出的给路径让模型用
/// Read 工具按需读 —— 引擎自己的**渐进披露**，比截断诚实、比整体注入省。
pub const AGENT_SKILL_INLINE_BUDGET_BYTES: usize = 32 * 1024;

/// 调用方给的东西（全部来自 IPC 参数）。
#[derive(Debug, Clone, Default)]
pub struct SpawnRequest {
    pub backend: Option<String>,
    pub cmd: Option<String>,
    pub args: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    /// 员工身份（`@name`）
    pub agent: Option<String>,
    /// 每槽路由：指定 provider
    pub provider_id: Option<String>,
    /// 模型覆盖
    pub model: Option<String>,
}

/// 一次 spawn 的**完整计划**（除了 `Channel`/`State` 那两样没法脱离 Tauri 的东西）。
#[derive(Debug, Clone)]
pub struct SpawnPlan {
    pub cmd: String,
    pub args: Vec<String>,
    /// 引擎说话的方式：`print` / `acp`。**运行方必须按它分流** ——
    /// ACP 引擎不吃 `-p`，塞进去只会得到一个说不清的错误。
    pub protocol: String,
    /// 你**点名**要的引擎 id（可能不存在）
    pub backend_requested: String,
    /// 点名那个 id 不存在、于是用了别的引擎 —— **必须如实说出来**。
    /// （第 26 刀真机验证：日志里 `backend=proj-only-xyz cmd=claude`，
    /// 点名的和实际跑的完全不是一个东西，而没有任何提示。）
    pub backend_fallback: bool,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub backend: String,

    // ── 路由 ──
    pub provider_id: Option<String>,
    pub model: String,
    pub route_source: String,
    pub provider_fallback: bool,
    pub switched_from: Option<String>,
    pub switch_reason: Option<String>,
    /// 需要写回 failover 历史时的 `(from, to, reason)`；纯函数不自己改 catalog
    pub switch_record: Option<(String, String, String)>,

    // ── 员工 ──
    pub agent: Option<String>,
    pub agent_injected: bool,
    pub posture: Option<String>,
    pub posture_inferred: bool,
    pub allowed_tools: Vec<String>,
    pub posture_note: Option<String>,
    pub skills_loaded: Vec<String>,
    pub skills_missing: Vec<String>,
    pub skills_deferred: Vec<String>,

    /// 如实记录过程中的告警（日志之外，返回值里也留一份，便于测试断言）
    pub warnings: Vec<String>,
}

/// 计划的一句话描述，给人看/给日志用。**一处实现** ——
/// "到底要跑什么"在排查时是第一个要看的东西，而两条运行路径（`engine_run` /
/// `structured_send`）都必须说同一句话。
pub fn describe(p: &SpawnPlan) -> String {
    let mut s = format!(
        "backend={} protocol={} cmd={} args={:?}",
        p.backend, p.protocol, p.cmd, p.args
    );
    if p.backend_fallback {
        s.push_str(&format!(" ⚠ 点名的是 {}（不存在，已退回）", p.backend_requested));
    }
    s
}

/// 调度台需要按协议分流 —— 判断**只有这一处**。
impl SpawnPlan {
    pub fn is_acp(&self) -> bool {
        self.protocol.trim().eq_ignore_ascii_case("acp")
    }
}


/// 生成 spawn 计划。**纯函数**：不改 catalog、不碰文件系统之外的状态
/// （唯一副作用是按需创建 cwd 与写 system prompt 文件 —— 这两件事本来就是它的职责）。
pub fn plan(req: &SpawnRequest, config: &AppConfig, catalog: &ProviderCatalog) -> SpawnPlan {
    let effective_backend = req
        .backend
        .as_deref()
        .unwrap_or(&config.default_backend)
        .to_string();
    // ★ 带上**项目层**：`<cwd>/.onecode/engines.json` 可以新增引擎（不能覆盖已有的）
    let profile = crate::backend::resolve_in_project(
        &effective_backend,
        req.cwd.as_deref().map(std::path::Path::new),
    );

    // ★ 点名的引擎到底找没找到？找不到时 `resolve_in_project` 会**静默**退回
    // claude-code —— 那意味着"你要 X，跑的是 Y"。这里把这件事变成显式事实。
    let backend_fallback = profile.id != effective_backend;
    if backend_fallback {
        log::warn!(
            "[spawn] 引擎 {effective_backend:?} 不存在（也可能是项目层定义被拒），已退回 {:?}",
            profile.id
        );
    }

    let cmd = req.cmd.clone().unwrap_or_else(|| profile.cmd.clone());
    let mut args = req
        .args
        .clone()
        .unwrap_or_else(|| profile.default_args.clone());
    let cwd = {
        let c = req.cwd.clone().unwrap_or_else(|| config.default_cwd.clone());
        // 确保默认工作目录存在
        std::fs::create_dir_all(&c).ok();
        c
    };

    let mut warnings: Vec<String> = Vec::new();
    if backend_fallback {
        warnings.push(format!(
            "引擎 {effective_backend:?} 不存在（或项目层定义被拒），已改用内置的 {:?}",
            profile.id
        ));
    }

    // ── 每槽路由（按员工自选模型 + failover）──
    // 清旧值 + 写新值的顺序由 `apply_route_env` 单一实现保证（不在这里再抄一遍）。
    let mut env = req.env.clone().unwrap_or_default();
    let route = providers::resolve_route_for_agent(
        catalog,
        config,
        req.agent.as_deref(),
        req.provider_id.as_deref(),
        req.model.as_deref(),
    );
    if route.fallback {
        warnings.push(format!(
            "请求的供应商 {:?} 不在目录里，已回退到 {:?}",
            req.provider_id, route.provider_id
        ));
    }
    if let Some(why) = &route.switch_reason {
        warnings.push(format!("路线切换：{why}"));
    }
    providers::apply_route_env(&profile.env_key_map, &route, &mut env);
    // AUTH_TOKEN 优先级：extra_env 显式声明 ANTHROPIC_AUTH_TOKEN 时丢弃 API_KEY，
    // 避免 claude 报 auth 冲突（DeepSeek 官方推荐 AUTH_TOKEN 模式）。
    if env.contains_key("ANTHROPIC_AUTH_TOKEN") {
        env.remove("ANTHROPIC_API_KEY");
    }
    let switch_record = match (
        route.switched_from.as_deref(),
        route.provider_id.as_deref(),
    ) {
        (Some(from), Some(to)) => Some((
            from.to_string(),
            to.to_string(),
            route
                .switch_reason
                .clone()
                .unwrap_or_else(|| "failover".to_string()),
        )),
        _ => None,
    };

    // ── 员工身份注入（@name 真派发）──
    // 解析 agent 定义 → 把身份写成 system prompt **文件** → 用后端声明的 flag 传入。
    // 用文件而不是 argv：长正文不受 argv 长度限制，且正文不会出现在 `ps` 里。
    // 后端没声明该能力时**不假装成功** —— agent 值照样记录，但 injected=false。
    let mut agent_injected = false;
    let mut posture_applied: Option<String> = None;
    let mut posture_inferred = false;
    let mut allowed_tools: Vec<String> = Vec::new();
    let mut posture_note: Option<String> = None;
    let mut skills_loaded: Vec<String> = Vec::new();
    let mut skills_missing: Vec<String> = Vec::new();
    let mut skills_deferred: Vec<String> = Vec::new();

    if let Some(agent_id) = req.agent.as_deref() {
        if !crate::agents::is_safe_id(agent_id) {
            warnings.push(format!("agent id 不合法，已忽略：{agent_id:?}"));
        } else {
            match crate::agents::resolve_agent(Some(Path::new(&cwd)), agent_id) {
                Some(def) => {
                    apply_employee(&def, agent_id, &profile, &cwd, &mut args, &mut EmployeeOutcome {
                        agent_injected: &mut agent_injected,
                        posture_applied: &mut posture_applied,
                        posture_inferred: &mut posture_inferred,
                        allowed_tools: &mut allowed_tools,
                        posture_note: &mut posture_note,
                        skills_loaded: &mut skills_loaded,
                        skills_missing: &mut skills_missing,
                        skills_deferred: &mut skills_deferred,
                        warnings: &mut warnings,
                    });
                }
                None => warnings.push(format!(
                    "agent @{agent_id} 在项目/onecode/用户三处都没找到，按普通终端启动"
                )),
            }
        }
    }

    SpawnPlan {
        cmd,
        args,
        protocol: profile.protocol.clone(),
        backend_requested: effective_backend.clone(),
        backend_fallback,
        cwd,
        env,
        // **实际用的**引擎 id（不是点名的）—— 日志与界面都以此为准
        backend: profile.id.clone(),
        provider_id: route.provider_id.clone(),
        model: route.model.clone(),
        route_source: route.source.as_str().to_string(),
        provider_fallback: route.fallback,
        switched_from: route.switched_from.clone(),
        switch_reason: route.switch_reason.clone(),
        switch_record,
        agent: req.agent.clone(),
        agent_injected,
        posture: posture_applied,
        posture_inferred,
        allowed_tools,
        posture_note,
        skills_loaded,
        skills_missing,
        skills_deferred,
        warnings,
    }
}

/// 员工那一段的各种产出。参数多，用一个结构收着，免得函数签名长得读不懂。
struct EmployeeOutcome<'a> {
    agent_injected: &'a mut bool,
    posture_applied: &'a mut Option<String>,
    posture_inferred: &'a mut bool,
    allowed_tools: &'a mut Vec<String>,
    posture_note: &'a mut Option<String>,
    skills_loaded: &'a mut Vec<String>,
    skills_missing: &'a mut Vec<String>,
    skills_deferred: &'a mut Vec<String>,
    warnings: &'a mut Vec<String>,
}

fn apply_employee(
    def: &crate::agents::AgentDef,
    agent_id: &str,
    profile: &BackendProfile,
    cwd: &str,
    args: &mut Vec<String>,
    out: &mut EmployeeOutcome<'_>,
) {
    // ── 员工级权限姿态（frontmatter 的 `permission:` + `tools:`）──
    // 未识别必须告警并退回"不干预"：静默当 Inherit 会让用户以为门开了、其实没开。
    let posture = match crate::backend::Posture::parse(&def.permission) {
        Some(p) => p,
        None => {
            let msg = format!(
                "无法识别的 permission 值 {:?}（支持 bypassPermissions / acceptEdits / default / plan / dontAsk），已按不干预处理",
                def.permission
            );
            *out.posture_note = Some(msg.clone());
            out.warnings.push(format!("@{agent_id}: {msg}"));
            crate::backend::Posture::Inherit
        }
    };
    let tools = def.allowed_tools();
    let outcome = crate::backend::apply_posture_args(profile, posture, &tools, args);
    *out.posture_applied = outcome.mode_applied.map(str::to_string);
    *out.posture_inferred = outcome.mode_inferred;
    *out.allowed_tools = outcome.tools_applied.clone();
    if let Some(reason) = &outcome.unsupported_reason {
        let merged = match out.posture_note.take() {
            Some(prev) => format!("{prev}；{reason}"),
            None => reason.clone(),
        };
        *out.posture_note = Some(merged);
        out.warnings.push(format!("@{agent_id}: {reason}"));
    }

    match &profile.system_prompt_file_flag {
        Some(flag) => {
            // ── 能力组合：员工的技能（frontmatter `skills:`）──
            // 引擎没有"按槽指定技能目录"的 flag，所以技能正文注入 system prompt。
            let names = def.skill_names();
            let report = crate::agents::resolve_skills(
                Some(Path::new(cwd)),
                &names,
                AGENT_SKILL_INLINE_BUDGET_BYTES,
            );
            *out.skills_loaded = report.inlined.iter().map(|s| s.name.clone()).collect();
            *out.skills_deferred = report.deferred.iter().map(|s| s.name.clone()).collect();
            *out.skills_missing = report.missing.clone();
            if !out.skills_missing.is_empty() {
                let missing = out.skills_missing.clone();
                out.warnings
                    .push(format!("@{agent_id}: 技能找不到 {missing:?}"));
            }

            let prompt = crate::agents::compose_system_prompt_with_skills(def, &report);
            // 文件名带正文摘要：同一 agent 在不同项目会解析到不同技能，
            // 用 agent id 当文件名会让两个槽互相覆盖。
            let key = crate::agents::prompt_file_key(agent_id, &prompt);
            let dir = providers::agent_prompts_dir();
            match crate::agents::write_prompt_file(&dir, &key, &prompt) {
                Ok(path) => {
                    args.push(flag.clone());
                    args.push(path.to_string_lossy().into_owned());
                    *out.agent_injected = true;
                }
                Err(e) => out
                    .warnings
                    .push(format!("@{agent_id}: system prompt 文件写入失败：{e}")),
            }
        }
        None => out.warnings.push(format!(
            "引擎 {} 没有声明 system-prompt-file 能力，@{agent_id} 身份**没有注入**",
            profile.id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendSource;

    fn config_with(cwd: &str) -> AppConfig {
        let mut c = AppConfig::default();
        c.default_backend = "claude-code".to_string();
        c.default_cwd = cwd.to_string();
        c.base_url = "https://global.example/v1".to_string();
        c.api_key = "key-global".to_string();
        c.model = "global-model".to_string();
        c.active_provider_id = Some("main".to_string());
        c
    }

    fn provider(id: &str, model: &str) -> crate::providers::Provider {
        crate::providers::Provider {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("https://{id}.example/v1"),
            api_key: format!("key-{id}"),
            model: model.to_string(),
            extra_env: HashMap::new(),
            context_window: None,
            created_at: String::new(),
        }
    }

    fn catalog() -> ProviderCatalog {
        let mut c = ProviderCatalog::default();
        c.providers = vec![provider("main", "main-model"), provider("roster-prov", "roster-model")];
        c.active_provider_id = Some("main".to_string());
        c
    }

    /// 指向临时 prompts 目录的守卫。
    ///
    /// 为什么需要：默认落盘目录是 `~/.onecode/cc-config/agent-prompts`，属于
    /// **工作区之外**（本沙箱直接拒绝写入，真实场景则是"测试不该碰用户目录"）。
    /// `ONECODE_AGENT_PROMPTS_DIR` 这个缝让测试写自己的临时目录。
    /// env 是进程级的，所以用一把本地锁把用到它的测试串起来。
    static PROMPT_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct TempPromptDir {
        _guard: std::sync::MutexGuard<'static, ()>,
        dir: std::path::PathBuf,
    }
    impl Drop for TempPromptDir {
        fn drop(&mut self) {
            std::env::remove_var("ONECODE_AGENT_PROMPTS_DIR");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn temp_prompt_dir(tag: &str) -> TempPromptDir {
        let guard = PROMPT_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp(&format!("prompts-{tag}"));
        std::env::set_var("ONECODE_AGENT_PROMPTS_DIR", &dir);
        TempPromptDir { _guard: guard, dir }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-plan-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_agent(cwd: &std::path::Path, id: &str, body: &str) {
        let dir = cwd.join(".claude").join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.md")), body).unwrap();
    }

    fn write_skill(cwd: &std::path::Path, name: &str, text: &str) {
        let dir = cwd.join(".claude").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 说明\n---\n{text}"),
        )
        .unwrap();
    }

    // ── 基础 ──────────────────────────────────────────────────────────

    #[test]
    fn a_plain_terminal_gets_the_profile_defaults() {
        let cwd = tmp("plain");
        let p = plan(&SpawnRequest::default(), &config_with(&cwd.to_string_lossy()), &catalog());
        assert_eq!(p.backend, "claude-code");
        assert_eq!(p.cmd, "claude");
        assert_eq!(
            p.args,
            vec!["--permission-mode", "bypassPermissions", "--dangerously-skip-permissions"],
            "没指定 agent 时必须一字不动地沿用后端默认（零回归）"
        );
        assert!(!p.agent_injected);
        // 「没指定 → 用全局快照」= 用 AppConfig 里的那份，而不是目录里某条 provider
        assert_eq!(p.model, "global-model");
        assert_eq!(p.route_source, "global");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn explicit_args_and_cmd_win_over_the_profile() {
        let cwd = tmp("explicit");
        let req = SpawnRequest {
            cmd: Some("/bin/echo".to_string()),
            args: Some(vec!["hi".to_string()]),
            ..Default::default()
        };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());
        assert_eq!(p.cmd, "/bin/echo");
        assert_eq!(p.args, vec!["hi"]);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── 路由 ──────────────────────────────────────────────────────────

    #[test]
    fn the_roster_route_reaches_the_env() {
        let cwd = tmp("roster");
        let mut cat = catalog();
        cat.roster.insert(
            "dev".into(),
            crate::providers::RosterEntry {
                provider_id: "roster-prov".into(),
                model: Some("roster-model-2".into()),
            },
        );
        let req = SpawnRequest { agent: Some("dev".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &cat);
        assert_eq!(p.route_source, "roster");
        assert_eq!(p.provider_id.as_deref(), Some("roster-prov"));
        assert_eq!(p.env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("key-roster-prov"));
        assert_eq!(p.env.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("https://roster-prov.example/v1"));
        assert_eq!(p.env.get("ANTHROPIC_MODEL").map(String::as_str), Some("roster-model-2"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_failover_is_reported_and_handed_back_for_the_history() {
        let cwd = tmp("failover");
        let mut cat = catalog();
        // 花名册绑一个没凭据的供应商 → 应沿撤退队列切走
        let mut broken = provider("broken", "b-model");
        broken.api_key = String::new();
        cat.providers.push(broken);
        cat.failover_queue = vec!["main".to_string()];
        cat.roster.insert(
            "dev".into(),
            crate::providers::RosterEntry { provider_id: "broken".into(), model: None },
        );
        let req = SpawnRequest { agent: Some("dev".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &cat);

        assert_eq!(p.provider_id.as_deref(), Some("main"));
        assert_eq!(p.switched_from.as_deref(), Some("broken"));
        assert!(p.warnings.iter().any(|w| w.contains("路线切换")), "{:?}", p.warnings);
        // ★ 纯函数：历史要**交回调用方**去记，自己不动 catalog
        let rec = p.switch_record.expect("failover 必须给出要记的历史");
        assert_eq!(rec.0, "broken");
        assert_eq!(rec.1, "main");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── 员工：身份 / 姿态 / 技能 ──────────────────────────────────────

    #[test]
    fn an_agent_with_permission_and_tools_reaches_the_args() {
        let _pd = temp_prompt_dir("agent");
        let cwd = tmp("agent");
        write_agent(
            &cwd,
            "dev",
            "---\nname: Dev\ndescription: 工程师\npermission: plan\ntools: Read, Bash(git:*)\n---\n你是工程师。",
        );
        let req = SpawnRequest { agent: Some("dev".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());

        assert!(p.agent_injected, "身份必须真的注入");
        assert_eq!(p.posture.as_deref(), Some("plan"));
        assert_eq!(p.allowed_tools, vec!["Read", "Bash(git:*)"]);
        assert!(p.posture_note.is_none(), "{:?}", p.posture_note);
        // 注入 flag 与姿态参数都在 args 里
        let join = p.args.join(" ");
        assert!(join.contains("--append-system-prompt-file"), "{join}");
        assert!(join.contains("--permission-mode plan"), "{join}");
        assert!(join.contains("--allowedTools Read,Bash(git:*)"), "{join}");
        assert!(!join.contains("planMode"), "非法值绝不能出现：{join}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 端到端：prompt 文件里必须同时有**身份**、**技能**与**两份协议**。
    /// 这条链路（frontmatter → 解析 → 组装 → 落盘 → 传参）以前从没被一起测过。
    #[test]
    fn the_written_prompt_file_carries_identity_skills_and_protocols() {
        let _pd = temp_prompt_dir("prompt");
        let cwd = tmp("prompt");
        write_skill(&cwd, "alpha", "做 A 的步骤");
        write_agent(
            &cwd,
            "dev",
            "---\nname: Dev\ndescription: 工程师\nskills: alpha\n---\n你是工程师，只做本分事。",
        );
        let req = SpawnRequest { agent: Some("dev".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());

        assert_eq!(p.skills_loaded, vec!["alpha"]);
        assert!(p.skills_missing.is_empty());

        // 把 --append-system-prompt-file 后面那个路径取出来，真读一遍
        let i = p.args.iter().position(|a| a == "--append-system-prompt-file").expect("要有 flag");
        let path = &p.args[i + 1];
        let text = std::fs::read_to_string(path).expect("prompt 文件必须真存在");
        assert!(text.contains("你是工程师，只做本分事。"), "身份正文");
        assert!(text.contains("## Skill: alpha") && text.contains("做 A 的步骤"), "技能正文");
        assert!(text.contains("产出协议"), "产出协议要注入");
        assert!(text.contains("任务协议"), "任务协议要注入");
        assert!(text.contains(".onecode/deliverables"), "协议要给真实路径");
        assert!(text.contains("不要写 `done`") || text.contains("不要写 done"), "不能自报完成");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_unknown_permission_value_is_reported_and_leaves_the_default_alone() {
        let _pd = temp_prompt_dir("badperm");
        let cwd = tmp("badperm");
        write_agent(&cwd, "dev", "---\nname: Dev\npermission: sudo\n---\nx");
        let req = SpawnRequest { agent: Some("dev".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());

        let note = p.posture_note.clone().unwrap_or_default();
        assert!(note.contains("sudo"), "要说清是哪个值不认识：{note}");
        // ★ 文案里不能再出现 planMode（CLI 实测不接受它）
        assert!(!note.contains("planMode"), "提示文案里不该再列非法值：{note}");
        assert!(note.contains("plan"), "要列出真正可用的取值：{note}");
        assert!(p.posture.is_none(), "不认识就不干预");
        // 默认权限参数原样保留（零回归）
        assert!(p.args.contains(&"bypassPermissions".to_string()));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_missing_agent_is_reported_and_spawns_a_plain_terminal() {
        let cwd = tmp("noagent");
        let req = SpawnRequest { agent: Some("nobody".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());
        assert!(!p.agent_injected);
        assert!(p.warnings.iter().any(|w| w.contains("没找到")), "{:?}", p.warnings);
        assert_eq!(p.agent.as_deref(), Some("nobody"), "值照样记录，便于排查");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_unsafe_agent_id_is_refused_before_touching_the_filesystem() {
        let cwd = tmp("unsafe");
        let req = SpawnRequest { agent: Some("../../evil".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());
        assert!(!p.agent_injected);
        assert!(p.warnings.iter().any(|w| w.contains("不合法")), "{:?}", p.warnings);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_engine_without_the_injection_flag_says_so_instead_of_pretending() {
        let _pd = temp_prompt_dir("nocap");
        let cwd = tmp("nocap");
        write_agent(&cwd, "dev", "---\nname: Dev\n---\nx");
        let mut cat = catalog();
        let mut oc = crate::backend::builtin_profiles()
            .into_iter()
            .find(|p| p.id == "opencode")
            .unwrap();
        assert_eq!(oc.source, BackendSource::Builtin);
        oc.id = "opencode".to_string();
        cat.providers.clear();
        let req = SpawnRequest {
            backend: Some("opencode".to_string()),
            agent: Some("dev".to_string()),
            ..Default::default()
        };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &cat);
        assert!(!p.agent_injected, "引擎没这个能力就不能假装注入");
        assert!(
            p.warnings.iter().any(|w| w.contains("没有声明") && w.contains("身份")),
            "{:?}",
            p.warnings
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn auth_token_wins_over_api_key() {
        let cwd = tmp("auth");
        let mut cat = catalog();
        // 供应商显式声明 AUTH_TOKEN
        let mut prov = provider("main", "m");
        prov.extra_env = HashMap::from([
            ("ANTHROPIC_AUTH_TOKEN".to_string(), "tok".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "should-be-removed".to_string()),
        ]);
        cat.providers = vec![prov];
        // 显式指定该 provider：不指定就走全局快照，而全局快照的 extra_env 来自 AppConfig
        let req = SpawnRequest { provider_id: Some("main".to_string()), ..Default::default() };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &cat);
        assert_eq!(p.env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str), Some("tok"));
        assert!(!p.env.contains_key("ANTHROPIC_API_KEY"), "AUTH_TOKEN 模式必须丢掉 API_KEY");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn the_default_cwd_is_created() {
        let base = tmp("mkcwd");
        let target = base.join("does").join("not").join("exist");
        let req = SpawnRequest { cwd: Some(target.to_string_lossy().into_owned()), ..Default::default() };
        let p = plan(&req, &config_with(&base.to_string_lossy()), &catalog());
        assert!(std::path::Path::new(&p.cwd).is_dir(), "默认工作目录要确保存在");
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── 端到端：计划 → 真的起一个进程 → 参数与环境确实到达 ──────────────
    //
    // 这是这套测试里唯一一条**跨越进程边界**的：前面那些只断言"计划里有什么"，
    // 这条断言"计划真的被交给了操作系统"。桩引擎把 argv 与几个关键 env 落盘，
    // 测试再读回来核对 —— 身份 / 权限 / 工具白名单 / 路由凭据 全在这一条链上。

    /// 写一个桩引擎：把收到的 argv 与关键 env 落到 `$STUB_OUT`。
    fn write_stub_engine(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("stub-engine");
        std::fs::write(
            &path,
            r#"#!/bin/sh
: > "$STUB_OUT"
for a in "$@"; do printf 'ARG %s\n' "$a" >> "$STUB_OUT"; done
printf 'ENV ANTHROPIC_API_KEY=%s\n' "$ANTHROPIC_API_KEY" >> "$STUB_OUT"
printf 'ENV ANTHROPIC_BASE_URL=%s\n' "$ANTHROPIC_BASE_URL" >> "$STUB_OUT"
printf 'ENV ANTHROPIC_MODEL=%s\n' "$ANTHROPIC_MODEL" >> "$STUB_OUT"
printf 'ENV PATH_PRESENT=%s\n' "$([ -n "$PATH" ] && echo yes || echo no)" >> "$STUB_OUT"
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn end_to_end_identity_posture_tools_and_credentials_reach_the_process() {
        let _pd = temp_prompt_dir("e2e");
        let cwd = tmp("e2e");
        write_skill(&cwd, "alpha", "做 A 的步骤");
        write_agent(
            &cwd,
            "dev",
            "---\nname: Dev\ndescription: 工程师\npermission: plan\ntools: Read, Bash(git:*)\nskills: alpha\n---\n你是工程师，只做本分事。",
        );

        // 花名册把这个员工绑到一个特定供应商
        let mut cat = catalog();
        cat.roster.insert(
            "dev".into(),
            crate::providers::RosterEntry {
                provider_id: "roster-prov".into(),
                model: Some("roster-model-2".into()),
            },
        );

        let stub = write_stub_engine(&cwd);
        let dump = cwd.join("dump.txt");
        let req = SpawnRequest {
            cmd: Some(stub.to_string_lossy().into_owned()),
            args: Some(Vec::new()), // 用桩引擎自己的参数，不掺 claude 的
            cwd: Some(cwd.to_string_lossy().into_owned()),
            agent: Some("dev".to_string()),
            ..Default::default()
        };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &cat);

        // ★ 真的起进程
        let status = std::process::Command::new(&p.cmd)
            .args(&p.args)
            .envs(&p.env)
            .env("STUB_OUT", &dump)
            .current_dir(&p.cwd)
            .status()
            .expect("桩引擎应该能启动");
        assert!(status.success(), "桩引擎退出码异常：{status:?}");

        let text = std::fs::read_to_string(&dump).expect("桩引擎必须留下 dump");
        eprintln!("===== 桩引擎实际收到的东西 =====\n{text}\n===== end =====");
        let args: Vec<&str> = p.args.iter().map(String::as_str).collect();
        let join = args.join(" ");

        // 1) 身份：flag + 文件路径到达进程
        assert!(join.contains("--append-system-prompt-file"), "argv 里要有注入 flag：{join}");
        // 2) 权限姿态：必须是 CLI 真正接受的 `plan`
        assert!(join.contains("--permission-mode plan"), "argv 里要有姿态：{join}");
        assert!(!join.contains("planMode"), "非法值绝不能到达进程：{join}");
        // 3) 工具白名单
        assert!(join.contains("--allowedTools Read,Bash(git:*)"), "argv 里要有白名单：{join}");
        // 4) 路由凭据真的进了环境（花名册那条路线）
        assert!(text.contains("ENV ANTHROPIC_API_KEY=key-roster-prov"), "{text}");
        assert!(text.contains("ENV ANTHROPIC_BASE_URL=https://roster-prov.example/v1"), "{text}");
        assert!(text.contains("ENV ANTHROPIC_MODEL=roster-model-2"), "{text}");
        // 5) 调用方原有的环境没被冲掉（PATH 这类必须留着）
        assert!(text.contains("ENV PATH_PRESENT=yes"), "{text}");
        // 6) 桩引擎把收到的参数也落了盘 —— 与计划逐一对应
        for a in &p.args {
            assert!(text.contains(&format!("ARG {a}")), "这个参数没到达进程：{a}");
        }

        // 7) 注入的正文真的在文件里（身份 + 技能 + 两份协议）
        let i = p.args.iter().position(|a| a == "--append-system-prompt-file").unwrap();
        let prompt = std::fs::read_to_string(&p.args[i + 1]).expect("prompt 文件");
        assert!(prompt.contains("你是工程师，只做本分事。"));
        assert!(prompt.contains("做 A 的步骤"));
        assert!(prompt.contains("产出协议") && prompt.contains("任务协议"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_plain_terminal_reaches_the_process_with_only_the_profile_defaults() {
        let cwd = tmp("e2e-plain");
        let stub = write_stub_engine(&cwd);
        let dump = cwd.join("dump2.txt");
        let req = SpawnRequest {
            cmd: Some(stub.to_string_lossy().into_owned()),
            args: Some(vec!["--permission-mode".to_string(), "bypassPermissions".to_string()]),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let p = plan(&req, &config_with(&cwd.to_string_lossy()), &catalog());
        let status = std::process::Command::new(&p.cmd)
            .args(&p.args)
            .envs(&p.env)
            .env("STUB_OUT", &dump)
            .current_dir(&p.cwd)
            .status()
            .unwrap();
        assert!(status.success());
        let text = std::fs::read_to_string(&dump).unwrap();
        assert!(text.contains("ARG --permission-mode"));
        assert!(text.contains("ARG bypassPermissions"));
        assert!(!text.contains("append-system-prompt-file"), "没指定 agent 就不该注入身份");
        // 没有指定 provider → 用全局快照的凭据
        assert!(text.contains("ENV ANTHROPIC_API_KEY=key-global"), "{text}");
        assert!(text.contains("ENV ANTHROPIC_MODEL=global-model"), "{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── ★ 项目层引擎：`plan()` 必须真的用上它 ──────────────────────────

    #[test]
    fn a_project_engine_is_actually_used_by_the_plan() {
        let cwd = std::env::temp_dir().join(format!(
            "dsh-sp-proj-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&cwd);
        std::fs::create_dir_all(cwd.join(".onecode")).unwrap();
        std::fs::write(
            cwd.join(".onecode/engines.json"),
            r#"{"engines":[{"id":"proj-only-xyz","cmd":"/bin/echo","default_args":["HI"],"display_name":"项目专属"}]}"#,
        )
        .unwrap();

        let mut config = crate::config::AppConfig::default();
        config.default_backend = "proj-only-xyz".to_string();
        config.default_cwd = cwd.to_string_lossy().into_owned();
        let req = SpawnRequest {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let plan = plan(&req, &config, &crate::providers::ProviderCatalog::default());
        assert_eq!(plan.backend, "proj-only-xyz");
        assert_eq!(plan.cmd, "/bin/echo", "命令必须来自项目层定义");
        assert!(plan.args.iter().any(|a| a == "HI"), "参数也要来自项目层：{:?}", plan.args);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 反过来：项目层**不能**把 `claude-code` 的命令改掉（安全不变量）。
    #[test]
    fn a_project_cannot_hijack_a_builtin_engine_used_by_default() {
        let cwd = std::env::temp_dir().join(format!(
            "dsh-sp-hijack-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&cwd);
        std::fs::create_dir_all(cwd.join(".onecode")).unwrap();
        std::fs::write(
            cwd.join(".onecode/engines.json"),
            r#"{"engines":[{"id":"claude-code","cmd":"/tmp/evil.sh"}]}"#,
        )
        .unwrap();

        let mut config = crate::config::AppConfig::default();
        config.default_backend = "claude-code".to_string();
        config.default_cwd = cwd.to_string_lossy().into_owned();
        let req = SpawnRequest {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let plan = plan(&req, &config, &crate::providers::ProviderCatalog::default());
        assert_eq!(plan.cmd, "claude", "克隆一个仓库不能改掉默认槽位要跑的命令");
        assert!(!plan.args.iter().any(|a| a.contains("evil")));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // ── ★ 点名 vs 实际：静默替换是不行的 ──────────────────────────────

    #[test]
    fn requesting_an_unknown_engine_is_reported_not_silently_substituted() {
        let mut config = crate::config::AppConfig::default();
        config.default_backend = "no-such-engine-xyz".to_string();
        let req = SpawnRequest { cwd: Some("/tmp".to_string()), ..Default::default() };
        let plan = plan(&req, &config, &crate::providers::ProviderCatalog::default());

        assert_eq!(plan.backend_requested, "no-such-engine-xyz");
        assert!(plan.backend_fallback, "必须知道发生了替换");
        assert_eq!(plan.backend, "claude-code", "backend 要说**实际用的**那个");
        assert!(plan.warnings.iter().any(|w| w.contains("不存在")), "{:?}", plan.warnings);
        // 日志/界面用的那一句话也要说清
        assert!(describe(&plan).contains("点名的是"), "{}", describe(&plan));
    }

    #[test]
    fn a_found_engine_is_not_marked_as_a_fallback() {
        let mut config = crate::config::AppConfig::default();
        config.default_backend = "claude-code".to_string();
        let req = SpawnRequest { cwd: Some("/tmp".to_string()), ..Default::default() };
        let plan = plan(&req, &config, &crate::providers::ProviderCatalog::default());
        assert!(!plan.backend_fallback);
        assert_eq!(plan.backend, "claude-code");
        assert!(!describe(&plan).contains("点名的是"));
    }
}
