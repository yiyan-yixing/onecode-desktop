//! ProviderStore — 供应商目录 + 切换历史（providers.json）
//!
//! M1：多供应商管理 + 手动切换（ADR-arch-model-auto-switch S4/S6 已签）。
//!
//! 设计依据：PRD phase1 §3.1 providers.json schema 草案（@dev 落盘依据）。
//! - `~/.onecode/providers.json` 独立于 desktop.json，ProviderStore 为目录单一真相；
//! - 激活 creds 仍走 ConfigManager(desktop.json) 现有链路，`pty_spawn` env 注入零改动；
//! - 切换由 SwitchManager 编排（本模块 `perform_switch`）：写 desktop.json + providers.json → 刷新托管 AppConfig → emit。
//!
//! 合规：本模块为原创实现（JSON 文件存储），非移植 cc-switch 代码；
//! 预置供应商数据取自 PRD phase1 §1.1（两档试点）。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::config::{AppConfig, ConfigManager, ConfigUpdate};

/// 切换历史上限（C4 裁定：最近 50 条，FIFO 淘汰）
pub const HISTORY_LIMIT: usize = 50;

/// 供应商托管的环境变量 key 全集（sync 隔离 settings 时管理）。
/// 跨供应商切档必须清理**不归当前供应商管理**的 key，否则上一个供应商的
/// extra_env 会残留：例 DeepSeek → 百炼（百炼 extra_env 为空）后，settings env
/// 仍带 deepseek 的 SUBAGENT_MODEL / EFFORT_LEVEL / AUTO_COMPACT_WINDOW，
/// 百炼会话会拿 deepseek 模型名去请求 → "Model not exist"（2026-08-22 实测）。
const MANAGED_PROVIDER_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];

/// 预置供应商（两档试点；extra_env = 厂商官方推荐给 Claude Code 的附加环境变量，
/// 预设选中时带出到表单，用户可改）
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    pub extra_env: &'static [(&'static str, &'static str)],
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "glm-5.2",
        name: "GLM-5.2",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        model: "glm-5.2",
        extra_env: &[],
    },
    ProviderPreset {
        id: "deepseek-v4",
        name: "DeepSeek-V4",
        base_url: "https://api.deepseek.com/anthropic",
        model: "deepseek-v4-pro[1m]",
        // DeepSeek 官方推荐 Claude Code 配置（2026-08 官网 export 原样落地）：
        // 主模型 pro[1m]，Haiku 槽 + 子代理用 flash；1M 窗口靠
        // CLAUDE_CODE_AUTO_COMPACT_WINDOW=786432（768k 触发阈值）；
        // CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 关掉向 api.anthropic.com 的
        // 非必要上报（否则国内 403 "Unable to connect to Anthropic services"）。
        extra_env: &[
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", "deepseek-v4-pro[1m]"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "deepseek-v4-pro[1m]"),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "deepseek-v4-flash[1m]"),
            ("CLAUDE_CODE_SUBAGENT_MODEL", "deepseek-v4-flash"),
            ("CLAUDE_CODE_EFFORT_LEVEL", "max"),
            ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "786432"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ],
    },
];

/// 单个供应商
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 供应商附加环境变量（DeepSeek 官方推荐配置；随 spawn 注入 + 同步隔离 settings）
    #[serde(default)]
    pub extra_env: HashMap<String, String>,
    /// 模型上下文窗口（token）。**可选**：不填就按型号后缀 / 内置表 / 保守默认推导
    /// （见 `model_context`）。留一个可填的口子是因为「内置表不可能覆盖所有第三方型号」，
    /// 而窗口填错的代价是两个方向都不好受：偏高 → provider 400；偏低 → 压缩得太早。
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub created_at: String,
}

impl Provider {
    /// 这个供应商实际生效的窗口与压缩阈值。**算出来的，不落盘**——
    /// 落盘就会和型号/覆盖值脱节（同一个坑：`pipeline` 每次整份重写产物，
    /// 缓存的值迟早是旧的）。
    pub fn window_plan(&self) -> crate::model_context::WindowPlan {
        crate::model_context::resolve(&self.model, self.context_window)
    }
}

/// 切换历史事件
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwitchEvent {
    pub time: String,
    pub from: String,
    pub to: String,
    /// consecutive_failure | timeout | rate_limited | manual
    pub reason: String,
}

/// 失败判定参数（对齐 cc-switch circuit_breaker 默认值：4 连败 / 60s 超时）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailoverParams {
    #[serde(default = "default_consecutive_failures")]
    pub consecutive_failures: u32,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
}

fn default_consecutive_failures() -> u32 {
    4
}

fn default_timeout_seconds() -> u32 {
    60
}

impl Default for FailoverParams {
    fn default() -> Self {
        Self {
            consecutive_failures: default_consecutive_failures(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

/// 花名册条目：**某个员工固定走哪条路线**（目标 ②：按员工自选模型）。
///
/// 为什么不写进 agent 的 md frontmatter：那是 Claude Code 的约定文件，
/// 里面写死别家供应商的模型名，换引擎就会把错误的模型名喂给不认识的 provider。
/// 花名册是**调度台自己的概念**，与 agent 定义解耦。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RosterEntry {
    pub provider_id: String,
    /// `None`/空 = 用该 provider 自己的默认模型
    #[serde(default)]
    pub model: Option<String>,
}

/// 供应商目录（providers.json 单一真相）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCatalog {
    pub providers: Vec<Provider>,
    /// failover 撤退路线（有序；队列首项 = 当前）
    pub failover_queue: Vec<String>,
    /// 自动切换总开关（M2 状态机消费；M1 先落盘骨架）
    pub auto_failover_enabled: bool,
    pub failover_params: FailoverParams,
    /// 切换历史（环形，上限 50 条，FIFO 淘汰）
    pub history: Vec<SwitchEvent>,
    /// 当前激活 provider id（与 desktop.json 的 active_provider_id 双写保持同步）
    pub active_provider_id: Option<String>,
    /// 员工（agent id）→ 固定路线。按员工自选模型就落在这里。
    #[serde(default)]
    pub roster: HashMap<String, RosterEntry>,
}

impl Default for ProviderCatalog {
    fn default() -> Self {
        Self {
            providers: vec![],
            failover_queue: vec![],
            auto_failover_enabled: true,
            failover_params: FailoverParams::default(),
            history: vec![],
            active_provider_id: None,
            roster: HashMap::new(),
        }
    }
}

/// 给界面的目录视图：目录本体 + **每个供应商算出来的窗口/压缩阈值**。
///
/// 为什么把窗口放进列表（而不是让前端自己算）：窗口来自型号后缀 + 内置表 +
/// 用户覆盖值三处叠加，前端复制一份口径必然漂移（`providers.rs` 与
/// `provider-switch.js` 之间已经有过一次「两边都以为对方在清理」的教训）。
/// `#[serde(flatten)]` 让 JSON 形状与原来完全兼容——前端现有代码不用改。
#[derive(Debug, Serialize)]
pub struct ProviderCatalogView {
    #[serde(flatten)]
    pub catalog: ProviderCatalog,
    /// provider_id → 窗口计划（用户不认识这个数字的来源就没法判断对不对）
    pub windows: HashMap<String, crate::model_context::WindowPlan>,
    /// provider_id → 来源说明（中文，界面直接用）
    pub window_sources: HashMap<String, &'static str>,
    /// provider_id → 这个供应商**能不能用**（静态判断：在目录里 + 有凭据；
    /// 本地档不要求凭据）。放进视图是因为前端**不该再抄一份这个判断** ——
    /// 抄一份就会与后端漂移，而本地档刚好吃这个亏（会被误显示成"无凭据"）。
    pub usable: HashMap<String, bool>,
    /// provider_id → 不可用的原因（可用时为空串）
    pub unusable_reason: HashMap<String, String>,
}

impl ProviderCatalogView {
    pub fn from_catalog(catalog: ProviderCatalog) -> Self {
        let mut windows = HashMap::new();
        let mut window_sources = HashMap::new();
        let mut usable = HashMap::new();
        let mut unusable_reason = HashMap::new();
        for p in &catalog.providers {
            let verdict = provider_usable(p);
            usable.insert(p.id.clone(), verdict.is_ok());
            unusable_reason.insert(
                p.id.clone(),
                verdict.err().unwrap_or_default(),
            );
            let plan = p.window_plan();
            windows.insert(p.id.clone(), plan);
            window_sources.insert(p.id.clone(), crate::model_context::source_label(plan.source));
        }
        Self {
            catalog,
            windows,
            window_sources,
            usable,
            unusable_reason,
        }
    }
}

/// ProviderStore：Arc<RwLock<ProviderCatalog>> 目录单一真相
#[derive(Clone)]
pub struct ProviderStore {
    inner: Arc<RwLock<ProviderCatalog>>,
}

impl ProviderStore {
    pub fn new(catalog: ProviderCatalog) -> Self {
        Self {
            inner: Arc::new(RwLock::new(catalog)),
        }
    }

    pub fn arc(&self) -> Arc<RwLock<ProviderCatalog>> {
        self.inner.clone()
    }
}

// ── 文件路径 / 加载 / 保存 ──────────────────────────────────────────

pub fn providers_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".onecode/providers.json")
}

pub fn load_from_file() -> ProviderCatalog {
    let path = providers_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cat) => return cat,
                Err(e) => {
                    log::warn!(
                        "[providers] parse {} failed: {e}, using defaults",
                        path.display()
                    );
                }
            },
            Err(e) => {
                log::warn!("[providers] read {} failed: {e}", path.display());
            }
        }
    }
    ProviderCatalog::default()
}

pub fn save_to_file(cat: &ProviderCatalog) -> Result<(), String> {
    let path = providers_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    let content =
        serde_json::to_string_pretty(cat).map_err(|e| format!("serialize failed: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("write {} failed: {e}", path.display()))?;
    log::info!("[providers] saved to {}", path.display());
    Ok(())
}

// ── 目录 CRUD ────────────────────────────────────────────────────────

/// 新增供应商（预设或自定义四要素）。
/// id 唯一性：预设用固定 slug；自定义由前端生成 slug，冲突时后端自动加后缀。
pub async fn add_provider(store: &ProviderStore, input: ProviderInput) -> Result<Provider, String> {
    let cat_arc = store.arc();
    let mut cat = cat_arc.write().await;
    let base_id = slugify(&input.name);
    let mut id = base_id.clone();
    let mut n = 1;
    while cat.providers.iter().any(|p| p.id == id) {
        id = format!("{base_id}-{n}");
        n += 1;
    }
    let provider = Provider {
        id,
        name: input.name.trim().to_string(),
        base_url: input.base_url.trim().to_string(),
        api_key: input.api_key.trim().to_string(),
        model: input.model.trim().to_string(),
        extra_env: input.extra_env.clone().unwrap_or_default(),
        context_window: input.context_window.filter(|w| *w > 0),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if provider.name.is_empty() || provider.base_url.is_empty() || provider.model.is_empty() {
        return Err("名称 / Base URL / 型号不能为空".into());
    }
    let provider_id = provider.id.clone();
    let is_first = cat.providers.is_empty();
    cat.providers.push(provider);
    // P2-3 修复：首个供应商自动激活（目录空 → 新 provider 设为当前），
    // 避免芯片「看起来已激活」但实际 spawn 仍用 desktop.json 默认值。
    // 仅改目录；desktop.json 的 creds 同步由命令层调 perform_switch 完成。
    if is_first {
        cat.active_provider_id = Some(provider_id.clone());
        cat.failover_queue.retain(|q| q != &provider_id);
        cat.failover_queue.insert(0, provider_id.clone());
    }
    save_to_file(&cat)?;
    log::info!("[providers] added {} (first={})", provider_id, is_first);
    Ok(cat.providers.iter().find(|p| p.id == provider_id).cloned().unwrap())
}

/// 编辑供应商（仅覆盖提供的字段；API Key 回显掩码由前端处理，不落日志）
pub async fn update_provider(
    store: &ProviderStore,
    cfg_mgr: &ConfigManager,
    id: String,
    updates: ProviderUpdate,
) -> Result<(), String> {
    let cat_arc = store.arc();
    let mut cat = cat_arc.write().await;
    let p = cat
        .providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("供应商不存在: {id}"))?;
    if let Some(name) = &updates.name {
        if !name.trim().is_empty() {
            p.name = name.trim().to_string();
        }
    }
    if let Some(base_url) = &updates.base_url {
        if !base_url.trim().is_empty() {
            p.base_url = base_url.trim().to_string();
        }
    }
    if let Some(api_key) = &updates.api_key {
        p.api_key = api_key.trim().to_string();
    }
    if let Some(model) = &updates.model {
        if !model.trim().is_empty() {
            p.model = model.trim().to_string();
        }
    }
    if let Some(extra_env) = &updates.extra_env {
        p.extra_env = extra_env.clone();
    }
    if let Some(window) = updates.context_window {
        // Some(None) = 清空回自动推导；0 / 明显不合理的值当清空处理，
        // 别让它把窗口压成 1（那会让每次请求都触发压缩）
        p.context_window = window.filter(|w| *w >= 8_192);
    }
    let updated = p.clone(); // iter_mut 借用结束前快照（须在再次借用 cat 前结束可变借用）
    let was_active = cat.active_provider_id.clone();
    save_to_file(&cat)?;
    // P1-2 修复：编辑当前激活供应商时，同步 creds 到 desktop.json + ConfigManager RwLock
    //（否则新 spawn / 重启继续用旧值；直接写 RwLock 避免依赖 2s watcher 兜底）
    if was_active.as_deref() == Some(id.as_str()) {
        if let Err(e) = sync_active_to_desktop(cfg_mgr, &updated).await {
            log::error!("[providers] update active sync failed: {e}");
        }
    }
    log::info!("[providers] updated {}", id);
    Ok(())
}

/// 删除供应商（约束：仅剩 1 家不可删；当前使用中不可删；清理队列/历史引用）
pub async fn delete_provider(store: &ProviderStore, id: String) -> Result<(), String> {
    let cat_arc = store.arc();
    let mut cat = cat_arc.write().await;
    if cat.providers.len() <= 1 {
        return Err("至少保留一家供应商".into());
    }
    if cat.active_provider_id.as_deref() == Some(id.as_str()) {
        return Err("当前使用中的供应商不可删除，请先切换".into());
    }
    let before = cat.providers.len();
    cat.providers.retain(|p| p.id != id);
    if cat.providers.len() == before {
        return Err(format!("供应商不存在: {id}"));
    }
    // 同步清理 failover 队列与历史中对该 id 的引用（AC-F3.5）
    cat.failover_queue.retain(|q| q != &id);
    cat.history.retain(|h| h.from != id && h.to != id);
    save_to_file(&cat)?;
    log::info!("[providers] deleted {}", id);
    Ok(())
}

/// 启动对账（P1-1）：若 providers.json 有 active_provider_id 但 desktop.json 未同步，
/// 则把该 provider 的 creds 写回 desktop.json。返回 true 表示发生修正。
/// 双写一致性兜底——切档中途写盘失败后重启自动收敛，避免两文件永久失配。
/// 另有激活供应商时无条件把 creds 同步到隔离 CC 配置（幂等）——保证升级后
/// 首次启动即使 desktop.json 已一致，也会创建/刷新 ~/.onecode/cc-config/settings.json。
pub fn reconcile_active_to_desktop(cat: &ProviderCatalog) -> Result<bool, String> {
    let Some(active_id) = &cat.active_provider_id else {
        return Ok(false);
    };
    let Some(p) = cat.providers.iter().find(|p| p.id == *active_id) else {
        return Ok(false);
    };
    // 全局隔离：无条件同步隔离 CC 配置（幂等）。路由权威在 CLAUDE_CODE_PROVIDER_
    // MANAGED_BY_HOST=1 下的进程 env，这里作为兜底 + 状态隔离的 settings 落地。
    if let Err(e) = sync_active_to_claude_settings(p) {
        log::warn!("[providers] reconcile: sync isolated CC settings failed: {e}");
        // 隔离 settings 是 claude 真正读的那份，写不进去 = 切档不生效
        crate::diagnostics::record(
            "cc-config-settings",
            e.clone(),
            "隔离配置没写进去 —— 切换供应商可能不生效",
        );
    }
    let cfg = crate::config::load_from_file();
    if cfg.active_provider_id.as_deref() == Some(active_id)
        && cfg.api_key == p.api_key
        && cfg.base_url == p.base_url
        && cfg.model == p.model
    {
        return Ok(false); // 已一致
    }
    let update = ConfigUpdate {
        default_cmd: None,
        default_args: None,
        default_cwd: None,
        max_terminals: None,
        ring_buffer_max_mb: None,
        api_key: Some(p.api_key.clone()),
        base_url: Some(p.base_url.clone()),
        model: Some(p.model.clone()),
        extra_env: Some(p.extra_env.clone()),
        wizard_completed: None,
        default_backend: None,
        active_provider_id: Some(Some(p.id.clone())),
    };
    let mut cfg = crate::config::load_from_file();
    update.apply_to(&mut cfg);
    crate::config::save_to_file(&cfg)?;
    log::info!("[providers] reconcile: synced active {} to desktop.json", p.id);
    Ok(true)
}

/// 将单个 provider 的激活 creds 同步到 desktop.json + ConfigManager RwLock
///（P1-2：编辑激活供应商时调用；直接写 RwLock 保证后续 spawn/restart 立即生效）。
pub async fn sync_active_to_desktop(cfg_mgr: &ConfigManager, p: &Provider) -> Result<(), String> {
    let update = ConfigUpdate {
        default_cmd: None,
        default_args: None,
        default_cwd: None,
        max_terminals: None,
        ring_buffer_max_mb: None,
        api_key: Some(p.api_key.clone()),
        base_url: Some(p.base_url.clone()),
        model: Some(p.model.clone()),
        extra_env: Some(p.extra_env.clone()),
        wizard_completed: None,
        default_backend: None,
        active_provider_id: Some(Some(p.id.clone())),
    };
    let cfg_arc = cfg_mgr.arc();
    let mut cfg = cfg_arc.write().await;
    update.apply_to(&mut cfg);
    crate::config::save_to_file(&cfg)?;
    // 全局隔离：切档后同步到 onecode 专属的隔离 CC 配置（settings.json env
    // 优先级最高，不写这里 = 实际仍走旧供应商）
    if let Err(e) = sync_active_to_claude_settings(p) {
        log::warn!("[providers] update active: sync isolated CC settings failed: {e}");
    }
    Ok(())
}

/// 这条路线是**怎么定下来的**。界面必须能显示 —— 否则"我明明给这个员工设了模型"
/// 与实际不符时无从排查。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteSource {
    /// 调用方显式指定（按任务/临时覆盖）
    Requested,
    /// 花名册里该员工的绑定
    Roster,
    /// 全局激活供应商
    Global,
}

impl RouteSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteSource::Requested => "requested",
            RouteSource::Roster => "roster",
            RouteSource::Global => "global",
        }
    }
}

/// 一次 spawn 实际使用的路由（供应商凭据 + 模型）。
///
/// 「自选模型」的核心判断。**刻意做成纯函数**：这条判断必须能单测，
/// 而且只能有一份实现 —— 之前踩过的坑就是同一个判断在调用点又抄了一遍
/// （改一处漏一处，编译器与测试都看不出来）。
#[derive(Clone, Debug, Serialize)]
pub struct ResolvedRoute {
    /// 实际生效的 provider id。
    /// `None` = 目录里没有可用 provider，用的是 desktop.json 的散装凭据。
    pub provider_id: Option<String>,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub extra_env: HashMap<String, String>,
    /// 调用方**指定了** provider 但目录里找不到 → 已回退到全局。
    /// 调用方必须如实显示（静默用错供应商比启动失败更难查）。
    pub fallback: bool,
    /// 路线来源（requested / roster / global）
    pub source: RouteSource,
    /// 因主选不可用而**换过**路线时的原 provider id（或 `"(全局)"`）
    pub switched_from: Option<String>,
    /// 换路线的原因（中文，界面直接显示）
    pub switch_reason: Option<String>,
}

fn route_from_provider(p: &Provider, source: RouteSource) -> ResolvedRoute {
    ResolvedRoute {
        provider_id: Some(p.id.clone()),
        base_url: p.base_url.clone(),
        api_key: p.api_key.clone(),
        model: p.model.clone(),
        extra_env: p.extra_env.clone(),
        fallback: false,
        source,
        switched_from: None,
        switch_reason: None,
    }
}

fn route_from_global(global: &AppConfig, fallback: bool) -> ResolvedRoute {
    ResolvedRoute {
        provider_id: global.active_provider_id.clone(),
        base_url: global.base_url.clone(),
        api_key: global.api_key.clone(),
        model: global.model.clone(),
        extra_env: global.extra_env.clone(),
        fallback,
        source: RouteSource::Global,
        switched_from: None,
        switch_reason: None,
    }
}

fn apply_model_override(route: &mut ResolvedRoute, requested_model: Option<&str>) {
    if let Some(m) = requested_model.map(str::trim).filter(|s| !s.is_empty()) {
        route.model = m.to_string();
    }
}

/// 一个 provider **能不能用**。返回 `Err(原因)` 时原因要能直接给人看。
///
/// 只做**静态检查**（在目录里 + 有凭据），不发网络请求：拉一次 spawn 就探一次网
/// 既慢又会被网络抖动带偏。真正的"连不上"由引擎自己的报错暴露，用户在界面上换回来即可。
/// base_url 指向本机 = **本地模型档**（Ollama / LM Studio / vLLM / llama.cpp…）。
///
/// 目标 ② 明确要求"含本地模型档"。本地推理服务**通常不需要 api_key**，
/// 如果可用性检查一律要求凭据，本地档会被判"不可用"，然后在开了 auto_failover 时
/// **被静默换到云上** —— 那正好与"我要用本地模型"相反，而且用户不会知道。
fn is_local_endpoint(base_url: &str) -> bool {
    let u = base_url.trim().to_ascii_lowercase();
    const LOCAL_HOSTS: [&str; 5] = [
        "//localhost",
        "//127.0.0.1",
        "//0.0.0.0",
        "//[::1]",
        "//host.docker.internal",
    ];
    LOCAL_HOSTS.iter().any(|h| u.contains(h))
}

fn provider_usable(p: &Provider) -> Result<(), String> {
    if p.base_url.trim().is_empty() {
        return Err(format!("{} 没填 base_url", p.name));
    }
    // 本地档不要求凭据（Ollama 之类本来就没有 key）
    if is_local_endpoint(&p.base_url) {
        return Ok(());
    }
    // 凭据可能来自 extra_env（有的供应商走自定义变量名），所以两处都看
    let has_key = !p.api_key.trim().is_empty()
        || p.extra_env.values().any(|v| !v.trim().is_empty());
    if !has_key {
        return Err(format!("{} 没有凭据（api_key 与 extra_env 都是空的）", p.name));
    }
    Ok(())
}

/// 员工级路线解析（目标 ②：**按员工自选模型** + failover）。
///
/// 优先级：
/// 1. 调用方显式指定（按任务/临时覆盖，最高）；
/// 2. **花名册**里该员工的绑定（`catalog.roster[agent_id]`）；
/// 3. 全局激活供应商。
///
// ── 熔断（② 最后一项：`failover_params` 终于被消费）──────────────────
//
// 在此之前 failover 只在**静态不可用**（没有供应商 / 没凭据）时发生；
// 系统跑起来之后某个供应商开始连败，是不会被处理的。这一节补上，
// 但判定**刻意保守** —— 误换比不换贵得多。

/// 一次运行对"当前供应商"意味着什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// 成功 —— 连败归零
    Success,
    /// 供应商层面的失败 —— 连败 +1
    ProviderFailure,
    /// 说不清 —— **既不加也不清零**
    Inconclusive,
}

/// 判定这次运行属于哪一种。
///
/// `ProviderFailure` 的条件很窄：引擎**报了执行错误**、且**一点推理都没做**
/// （输入/输出 token 全零、费用为 0）。这与"凭据/连通性坏了"的特征一致。
///
/// **超时 / 没有结算事件一律 `Inconclusive`**：第 20 刀实测过"模型想用工具时
/// 会挂住 >90 秒"，那是工具问题不是供应商问题。拿它去换供应商会把**好供应商**
/// 换掉 —— 所以宁可漏判。**误换比不换贵。**
pub fn classify_run(r: &crate::engine_run::RunReport) -> RunOutcome {
    use crate::engine_run::ExitKind;
    let Some(res) = &r.result else {
        // ★ 没有结算时，**按结束方式**再分（第 25 刀真机验证时发现的缺口：
        // 供应商连不上时引擎**压根不产生 result 事件**，于是熔断永远抓不到它）
        return match r.exit_kind {
            // 进程自己退了、一轮都没成 → 连接/凭据问题的典型特征
            ExitKind::ExitedWithoutResult => RunOutcome::ProviderFailure,
            // 超时：可能是"模型想用工具挂住了"（第 20 刀实测过）→ 换供应商没用
            ExitKind::TimedOut => RunOutcome::Inconclusive,
            // 二进制都不在：换个供应商同样救不了
            ExitKind::SpawnFailed => RunOutcome::Inconclusive,
            // 有 result 就走上分支；这里兜底
            ExitKind::Completed => RunOutcome::Inconclusive,
        };
    };
    if !res.is_error {
        return RunOutcome::Success;
    }
    let no_inference = res.usage.input_tokens == 0
        && res.usage.output_tokens == 0
        && res.total_cost_usd == 0.0;
    if no_inference {
        RunOutcome::ProviderFailure
    } else {
        // 推理做过才报错 → 是模型/应用层面的问题，不是"这个供应商连不上"
        RunOutcome::Inconclusive
    }
}

/// 连败计数器。**进程内、不落盘**：落盘会让昨天的失败影响今天，重启清零才对。
#[derive(Debug, Default)]
pub struct Breaker {
    counts: std::collections::HashMap<String, u32>,
}

impl Breaker {
    /// 记一次结果，返回该供应商**当前**连败数。
    pub fn record(&mut self, provider_id: &str, outcome: RunOutcome) -> u32 {
        if provider_id.trim().is_empty() {
            return 0;
        }
        match outcome {
            RunOutcome::Success => {
                self.counts.remove(provider_id);
                0
            }
            RunOutcome::ProviderFailure => {
                let c = self.counts.entry(provider_id.to_string()).or_insert(0);
                *c += 1;
                *c
            }
            // 说不清：不动 —— 既不加也不清零（清零会让"偶发成功"永远凑不满阈值）
            RunOutcome::Inconclusive => self.consecutive(provider_id),
        }
    }

    pub fn consecutive(&self, provider_id: &str) -> u32 {
        self.counts.get(provider_id).copied().unwrap_or(0)
    }

    pub fn reset(&mut self) {
        self.counts.clear();
    }
}

/// 进程级计数器（生产用）。测试用局部 `Breaker` 实例，不碰它。
pub fn breaker() -> &'static std::sync::Mutex<Breaker> {
    static B: std::sync::OnceLock<std::sync::Mutex<Breaker>> = std::sync::OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(Breaker::default()))
}

/// 下一个可用的供应商：先按撤退队列顺序，队列里找不到再按目录顺序。
/// **跳过自己**，也跳过不可用的（`provider_usable` 为准，判断只有一处）。
pub fn next_usable_provider(cat: &ProviderCatalog, from: &str) -> Option<String> {
    for id in &cat.failover_queue {
        if id == from {
            continue;
        }
        if let Some(p) = cat.providers.iter().find(|p| &p.id == id) {
            if provider_usable(p).is_ok() {
                return Some(p.id.clone());
            }
        }
    }
    cat.providers
        .iter()
        .find(|p| p.id != from && provider_usable(p).is_ok())
        .map(|p| p.id.clone())
}

/// 记一次运行结果；**真的连败到阈值**且开了自动换档时，复用 `perform_switch`
/// 切到下一个可用供应商，并返回切到谁了。
///
/// 复用而不是另写一套：切档要写 AppConfig + 同步隔离 settings + 队列自愈 + 发事件，
/// 任何一处漏掉都会"看起来切了其实没生效"（规则 7）。
pub async fn note_run_outcome(
    app: &AppHandle,
    cfg_mgr: &ConfigManager,
    store: &ProviderStore,
    provider_id: &str,
    outcome: RunOutcome,
) -> Option<String> {
    let (count, threshold, enabled, nxt) = {
        let cat_arc = store.arc();
        let cat = cat_arc.read().await;
        let count = {
            let mut b = breaker().lock().unwrap_or_else(|e| e.into_inner());
            b.record(provider_id, outcome)
        };
        let params = &cat.failover_params;
        let trip = count >= params.consecutive_failures.max(1);
        let nxt = if trip { next_usable_provider(&cat, provider_id) } else { None };
        (count, params.consecutive_failures, cat.auto_failover_enabled, nxt)
    };

    if outcome == RunOutcome::ProviderFailure {
        log::warn!("[failover] {provider_id} 连败 {count}/{threshold}");
    }
    if !enabled || nxt.is_none() {
        if outcome == RunOutcome::ProviderFailure && count >= threshold.max(1) {
            log::warn!("[failover] 达到阈值但无法换档（enabled={enabled}，候选={nxt:?}）");
        }
        return None;
    }
    let to = nxt?;
    match perform_switch(app, cfg_mgr, store, &to, "consecutive_failure").await {
        Ok(()) => {
            log::warn!("[failover] 连败 {count} 次 → 已切到 {to}");
            // 切完清零：新供应商从零开始计数
            breaker().lock().unwrap_or_else(|e| e.into_inner()).reset();
            Some(to)
        }
        Err(e) => {
            log::error!("[failover] 换档失败：{e}");
            None
        }
    }
}

/// 选定后做**可用性检查**；不可用且 `catalog.auto_failover_enabled` 时，
/// 依次尝试 `catalog.failover_queue`，用第一个可用的，并把
/// `switched_from` / `switch_reason` 填上（**必须如实显示**）。
/// 全部不可用 → 退回全局，同样如实报告。
pub fn resolve_route_for_agent(
    catalog: &ProviderCatalog,
    global: &AppConfig,
    agent_id: Option<&str>,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> ResolvedRoute {
    let req = requested_provider.map(str::trim).filter(|s| !s.is_empty());
    let agent = agent_id.map(str::trim).filter(|s| !s.is_empty());

    // ── 1) 定主选 ──
    //
    // `intended` = 我们**想用**的那个 provider（None = 这条路线本来就是全局快照）。
    // `broken`   = 想用的那个**根本不在目录里**的原因；它与"在目录里但没凭据"
    //              一样都是 failover 的触发条件。
    //
    // 为什么要把这两件事分开记：早先的写法是"花名册指向不存在的 provider → 直接
    // 换成全局路线"，而全局恰好可用 ⇒ 可用性检查通过 ⇒ **failover 根本不触发**，
    // 结果却把 source 标成 Roster —— 也就是"实际走了全局，却告诉你是花名册"。
    // 这类"来源标注与实际不符"正是最难查的错。
    // `origin` = 主选的身份标签（换路线时要写清从谁换过来），**显式携带**，
    // 不从错误文案里反抠字符串（那种写法一改措辞就悄悄失效）。
    let (mut route, intended, wanted_model, origin, broken): (
        ResolvedRoute,
        Option<&Provider>,
        Option<String>,
        String,
        Option<String>,
    ) = if let Some(id) = req {
        match catalog.providers.iter().find(|p| p.id == id) {
            Some(p) => (
                route_from_provider(p, RouteSource::Requested),
                Some(p),
                None,
                id.to_string(),
                None,
            ),
            None => (
                route_from_global(global, true),
                None,
                None,
                id.to_string(),
                Some(format!("指定的供应商 {id} 不在目录里")),
            ),
        }
    } else if let Some(entry) = agent.and_then(|a| catalog.roster.get(a)) {
        let id = entry.provider_id.trim().to_string();
        let model = entry.model.clone().filter(|m| !m.trim().is_empty());
        match catalog.providers.iter().find(|p| p.id == id) {
            Some(p) => (
                route_from_provider(p, RouteSource::Roster),
                Some(p),
                model,
                id,
                None,
            ),
            None => (
                route_from_global(global, true),
                None,
                model,
                id.clone(),
                Some(format!("花名册里绑的供应商 {id} 不在目录里")),
            ),
        }
    } else {
        // 本来就没人指定 → 身份标签就是全局激活的那个（拿不到就写"(全局)"）
        let label = global
            .active_provider_id
            .clone()
            .unwrap_or_else(|| "(全局)".to_string());
        (route_from_global(global, false), None, None, label, None)
    };

    // ── 2) 可用性 → failover ──
    let problem: Option<String> = match (&broken, intended) {
        (Some(b), _) => Some(b.clone()),
        (None, Some(p)) => provider_usable(p).err(),
        (None, None) => None, // 本来就是全局快照：没有"主选"可检查
    };

    if let Some(why) = problem {
        if catalog.auto_failover_enabled {
            let mut picked: Option<&Provider> = None;
            let mut skipped: Vec<String> = Vec::new();
            for id in &catalog.failover_queue {
                // 跳过已经被判定不可用的那条，别原地打转
                if intended.map(|p| &p.id == id).unwrap_or(false) || *id == origin {
                    continue;
                }
                match catalog.providers.iter().find(|p| p.id == *id) {
                    Some(p) => match provider_usable(p) {
                        Ok(()) => {
                            picked = Some(p);
                            break;
                        }
                        Err(e) => skipped.push(e),
                    },
                    None => skipped.push(format!("撤退队列里的 {id} 不在目录里")),
                }
            }
            if let Some(p) = picked {
                route = route_from_provider(p, RouteSource::Global);
                route.source = RouteSource::Global;
                route.fallback = false;
                route.switched_from = Some(origin.clone());
                route.switch_reason = Some(format!("{why} → 已按撤退队列切到 {}", p.name));
                if !skipped.is_empty() {
                    route.switch_reason = Some(format!(
                        "{}；跳过了 {}",
                        route.switch_reason.clone().unwrap_or_default(),
                        skipped.join("、")
                    ));
                }
                log::warn!("[providers] failover: {why} → {}", p.id);
            } else {
                route.switched_from = None;
                route.switch_reason = Some(format!(
                    "{why}；撤退队列里也没有可用的（{}）",
                    if skipped.is_empty() { "队列为空".to_string() } else { skipped.join("、") }
                ));
            }
        } else {
            route.switch_reason = Some(format!("{why}；自动切换已关闭"));
        }
    }

    // ── 3) 模型覆盖 ──
    // 花名册的模型只在**真的用了花名册那条路线**时生效；一旦换过供应商，
    // 就把 A 供应商的模型名硬塞给 B（第 2 轮定下的规矩：宁可留空）。
    let model_override = if route.switched_from.is_some() { None } else { wanted_model };
    let model_override = model_override.as_deref().or(requested_model);
    apply_model_override(&mut route, model_override);

    route
}

/// 花名册的一行：**绑定** + **实际解析出来的路线**。
///
/// 为什么两个都给：只给绑定，用户看到"我给 @dev 设了 glm"却不知道实际跑了别的；
/// 只给实际，用户不知道自己设过什么。两者并列，不一致才看得见。
#[derive(Debug, Serialize)]
pub struct RosterRow {
    pub agent_id: String,
    pub provider_id: String,
    pub model: Option<String>,
    /// 绑定的 provider 是否还在目录里（不在了要显眼）
    pub provider_exists: bool,
    /// 实际解析出来的路线（含 failover 结果）
    pub resolved_provider: Option<String>,
    pub resolved_model: String,
    pub resolved_source: &'static str,
    pub switched_from: Option<String>,
    pub switch_reason: Option<String>,
}

/// 花名册视图：每个员工一行。
pub fn roster_view(catalog: &ProviderCatalog, global: &AppConfig) -> Vec<RosterRow> {
    let mut rows: Vec<RosterRow> = catalog
        .roster
        .iter()
        .map(|(agent_id, entry)| {
            let r = resolve_route_for_agent(
                catalog,
                global,
                Some(agent_id),
                None,
                None,
            );
            RosterRow {
                agent_id: agent_id.clone(),
                provider_id: entry.provider_id.clone(),
                model: entry.model.clone(),
                provider_exists: catalog.providers.iter().any(|p| p.id == entry.provider_id),
                resolved_provider: r.provider_id,
                resolved_model: r.model,
                resolved_source: r.source.as_str(),
                switched_from: r.switched_from,
                switch_reason: r.switch_reason,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    rows
}

/// 花名册文本（人看到的）。纯函数，便于单测。
pub fn roster_text(rows: &[RosterRow], providers: &[Provider]) -> String {
    if rows.is_empty() {
        return "花名册还是空的。\n\n在命令面板里输入 /roster 给某个员工绑定模型；\n没绑定的员工走全局激活供应商。"
            .to_string();
    }
    let mut s = format!("花名册（{} 个员工绑定了自己的路线）\n", rows.len());
    for r in rows {
        s.push_str(&format!(
            "\n@{} → {} {}\n",
            r.agent_id,
            r.provider_id,
            if r.provider_exists { "" } else { "⚠ 这个供应商已不在目录里" }
        ));
        if let Some(m) = &r.model {
            s.push_str(&format!("      绑定模型：{m}\n"));
        }
        s.push_str(&format!("      实际：{} / {}（来源 {}）\n", r.resolved_provider.clone().unwrap_or_else(|| "(全局)".into()), r.resolved_model, r.resolved_source));
        if let Some(from) = &r.switched_from {
            s.push_str(&format!("      ⚠ 已从 {from} 切走：{}\n", r.switch_reason.clone().unwrap_or_default()));
        } else if let Some(why) = &r.switch_reason {
            s.push_str(&format!("      ⚠ {why}\n"));
        }
    }
    s.push_str("\n可选的供应商：");
    let ids: Vec<String> = providers
        .iter()
        .map(|p| {
            if provider_usable(p).is_ok() {
                p.id.clone()
            } else {
                format!("{}(不可用)", p.id)
            }
        })
        .collect();
    s.push_str(&ids.join("、"));
    s.push('\n');
    s
}

/// 记一条切换历史（环形上限 `HISTORY_LIMIT`）。failover 真的发生时由上层调用。
pub fn record_switch(cat: &mut ProviderCatalog, from: &str, to: &str, reason: &str) {
    cat.history.push(SwitchEvent {
        time: chrono::Utc::now().to_rfc3339(),
        from: from.to_string(),
        to: to.to_string(),
        reason: reason.to_string(),
    });
    while cat.history.len() > HISTORY_LIMIT {
        cat.history.remove(0);
    }
}

/// 把解析后的路由落到目标后端的环境变量键上。
///
/// **这是"每槽跑不同供应商"真正生效的地方**，所以做成纯函数并单测。
/// 语义（顺序很重要）：
/// 1. **先清空** `env_key_map` 声明的所有键 —— 否则调用方传来的陈旧凭据（例如
///    持久化下来的上一轮 env）会盖住本次路由，出现"以为换了供应商其实没换"；
/// 2. 再按 `api_key` / `base_url` / `model` 写入非空值；
/// 3. 最后合并 `extra_env`（供应商附加变量覆盖上面的默认值）。
///
/// 只碰 `env_key_map` 声明的键与 `extra_env`，不动其它环境变量。
pub fn apply_route_env(
    env_key_map: &[(String, String)],
    route: &ResolvedRoute,
    env: &mut HashMap<String, String>,
) {
    for (_, env_var) in env_key_map {
        env.remove(env_var);
    }
    let fields: [(&str, &str); 3] = [
        ("api_key", &route.api_key),
        ("base_url", &route.base_url),
        ("model", &route.model),
    ];
    for (config_key, env_var) in env_key_map {
        if let Some((_, value)) = fields.iter().find(|(k, _)| k == config_key) {
            if !value.is_empty() {
                env.insert(env_var.clone(), value.to_string());
            }
        }
    }
    for (k, v) in &route.extra_env {
        env.insert(k.clone(), v.clone());
    }
}

/// onecode 专属的 Claude Code 隔离配置目录：`~/.onecode/cc-config`。
///
/// pty 启动时通过 `CLAUDE_CONFIG_DIR` 注入，让 onecode 里跑的 claude 只读这份
/// 隔离配置，完全不经手/不依赖用户全局的 `~/.claude/settings.json`
/// （全局隔离，董事长 2026-08-22 指令：不改全局、不读全局）。
/// 员工 system prompt 文件的落盘目录。
///
/// 默认 `{cc_config_dir}/agent-prompts`。可用 `ONECODE_AGENT_PROMPTS_DIR` 覆盖 ——
/// 这个缝有**两个真实用途**，不是为测试而设的摆设：
/// 1. 隔离运行（测试 / 多实例）不该往用户真实的 `~/.onecode` 里写；
/// 2. 有人想把生成物放到别处（比如放到项目内便于查看）。
pub fn agent_prompts_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ONECODE_AGENT_PROMPTS_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    cc_config_dir().join("agent-prompts")
}

pub fn cc_config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".onecode/cc-config")
}

/// 把激活供应商的 creds 同步到隔离配置目录 `~/.onecode/cc-config/settings.json`。
///
/// Claude Code 的 settings.json env 优先级高于进程环境变量（见 pty/auth-fix
/// 注释），因此切档后必须把 base_url/api_key/model 写进这份 Claude Code 真正
/// 读取的 settings，否则注入的进程 env 会被（旧）settings 盖掉。保留 settings
/// 内其它键（API_TIMEOUT_MS / DISABLE_AUTOUPDATER 等），幂等可重复调用。
pub fn sync_active_to_claude_settings(p: &Provider) -> Result<(), String> {
    let path = cc_config_dir().join("settings.json");
    let mut settings: serde_json::Value = if path.exists() {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("read {} failed: {e}", path.display()))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let env_obj = settings
        .as_object_mut()
        .ok_or_else(|| "cc settings.json is not an object".to_string())?
        .entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "cc settings.json env is not an object".to_string())?;

    // 清理不归当前供应商管理的托管 key（防跨供应商 extra_env 残留，见上面 const）
    for k in MANAGED_PROVIDER_ENV_KEYS {
        if !p.extra_env.contains_key(*k) {
            env_obj.remove(*k);
        }
    }

    env_obj.insert(
        "ANTHROPIC_BASE_URL".into(),
        serde_json::Value::String(p.base_url.clone()),
    );
    // AUTH_TOKEN 优先：extra_env 显式声明 ANTHROPIC_AUTH_TOKEN 时，不写 API_KEY
    // （避免 claude 的 auth 冲突告警；DeepSeek 官方推荐 AUTH_TOKEN 模式）。
    // 同时清除另一字段的陈旧值（从 API_KEY 供应商切到 AUTH_TOKEN 供应商时，旧的
    // settings.json env 里可能残留 API_KEY，反之亦然），保持与激活供应商一致。
    if p.extra_env.contains_key("ANTHROPIC_AUTH_TOKEN") {
        env_obj.remove("ANTHROPIC_API_KEY");
    } else {
        env_obj.insert(
            "ANTHROPIC_API_KEY".into(),
            serde_json::Value::String(p.api_key.clone()),
        );
        env_obj.remove("ANTHROPIC_AUTH_TOKEN");
    }
    for k in [
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
    ] {
        env_obj.insert(k.to_string(), serde_json::Value::String(p.model.clone()));
    }

    // ── 声明模型真实窗口（2026-09-23 两起 400 的根因）────────────────
    //
    // 不声明的后果：Claude Code 按「未识别型号」的默认值办事，会话一路涨过
    // provider 的上限才撞墙。实测：qwen3.8-27b（真窗口 262144）会话涨到 392074。
    // CLI 自己的提示就是「set CLAUDE_CODE_MAX_CONTEXT_TOKENS to its real window」。
    //
    // 放在 extra_env 合并**之前**：用户若显式填了这个 key，以他填的为准
    //（他可能有我们不知道的信息，比如走的是代理或私有部署）。
    // 一方模型（Claude 系）**不声明**：CLI 的 `auto` 比我们的内置表准，
    // 插一杠子的唯一效果是把窗口调小。见 `model_context::is_first_party`。
    let plan = p.window_plan();
    let declare = !crate::model_context::is_first_party(&p.model, &p.base_url);
    if declare && !p.extra_env.contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS") {
        env_obj.insert(
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),
            serde_json::Value::String(plan.window.to_string()),
        );
    }
    if declare && plan.source == "default" {
        // 未识别型号：值是我们保守估的，**必须让它可被看见**——
        // 只写日志不够，界面要显示（见 `ProviderCatalogView::windows`）。
        log::warn!(
            "[providers] {} 的型号 {} 未识别，窗口按保守值 {} 处理（可在供应商里手填）",
            p.id,
            p.model,
            plan.window
        );
    }
    // 供应商附加环境变量（DeepSeek 官方推荐：subagent/effort/compact-window/模型差异化），
    // 最后合并 → 覆盖上面的默认（例如 DEFAULT_HAIKU_MODEL=flash、主模型差异化）。
    for (k, v) in &p.extra_env {
        env_obj.insert(k.clone(), serde_json::Value::String(v.clone()));
    }

    // 状态栏（2026-08-22）：自定义 statusLine 替换 claude 底部状态栏，彻底抹掉
    // "Not logged in · Run /login" 登录文案（与 pty 的非空 CLAUDE_CODE_OAUTH_TOKEN
    // 注入互为双保险），同时展示 模型/仓库分支/上下文占用/成本。
    // 脚本指向 onecode 隔离目录下的副本（~/.onecode/command/statusline-command.sh，
    // 内容与全局 ~/.claude/command 一致，但跟随 onecode 隔离——不读用户全局）。
    let root = settings
        .as_object_mut()
        .ok_or_else(|| "cc settings.json is not an object".to_string())?;
    root.insert(
        "statusLine".to_string(),
        serde_json::json!({
            "type": "command",
            "command": "bash ~/.onecode/command/statusline-command.sh"
        }),
    );
    // 强制深色主题：onecode 界面是深色（vscode-skin），claude 终端必须深色才不违和。
    // claude 会自己把 theme 写进 settings.json（状态漂移），这里每次同步兜底回 dark
    // （幂等；实测 claude 确实读这个 key——dark/light 下发射的颜色不同）。
    root.insert(
        "theme".to_string(),
        serde_json::Value::String("dark".to_string()),
    );

    // ── 自动压缩阈值 ────────────────────────────────────────────────
    //
    // **写 settings 键而不是环境变量**，两个原因：
    // ① CLI 对两者的关系是「环境变量优先，设了它就盖住设置项，并且在 /config 里
    //    提示『CLAUDE_CODE_AUTO_COMPACT_WINDOW is set and takes precedence』」——
    //    我们写设置项，用户在 /config 里还能自己调；写环境变量就把那个入口锁死了。
    // ② 设置项在界面上是可见的（来源显示「from settings」），出问题查得到。
    //
    // 阈值 = 窗口 − 补全预算 − 安全余量（见 model_context 模块头：provider 校验的是
    // 「输入 + 补全 ≤ 窗口」，只按输入算照样 400）。
    // 用户在 extra_env 里显式设了环境变量时**不写**——写了也不会生效（env 优先），
    // 留着只会让 /config 里出现「被环境变量盖住」的困惑提示。
    // 一方模型同样不写（理由见上面 `declare`）。
    if declare {
        if p.extra_env.contains_key("CLAUDE_CODE_AUTO_COMPACT_WINDOW") {
            // 用户显式指定了阈值（env 优先于设置项）→ 清掉我们上次可能写下的键，
            // 否则 settings 里会留一个**不生效**的值，看起来像配置漂移。
            root.remove("autoCompactWindow");
        } else {
            root.insert(
                "autoCompactWindow".to_string(),
                serde_json::Value::Number(plan.compact.into()),
            );
        }
    } else {
        root.remove("autoCompactWindow"); // 切回一方模型：交还给 CLI 的 auto
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    let pretty = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("serialize cc settings failed: {e}"))?;
    fs::write(&path, pretty).map_err(|e| format!("write {} failed: {e}", path.display()))?;
    log::info!(
        "[providers] synced active {} creds to isolated CC config {}",
        p.id,
        path.display()
    );
    Ok(())
}

// ── 切换编排（SwitchManager 核心）───────────────────────────────────

/// 手动切换：① 写激活态到 AppConfig(desktop.json)；② 写 active_provider_id/queue/history
/// 到 providers.json；③ 替换托管 Arc<AppConfig>（新 spawn 读新 creds）；④ emit `provider-switched`。
/// 切换激活供应商。`reason` 会写进历史与事件 —— 熔断触发的切换必须与"人手点的"
/// 区分开（否则回头看历史分不清是谁换的）。
pub async fn perform_switch(
    app: &AppHandle,
    cfg_mgr: &ConfigManager,
    store: &ProviderStore,
    provider_id: &str,
    reason: &str,
) -> Result<(), String> {
    let cat_arc = store.arc();
    let mut cat = cat_arc.write().await;

    let provider = cat
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| format!("供应商不存在: {provider_id}"))?;

    let old_active = cat.active_provider_id.clone();

    // ① 写激活态 → AppConfig(desktop.json)，走现有 ConfigManager + save_config 路径
    let update = ConfigUpdate {
        default_cmd: None,
        default_args: None,
        default_cwd: None,
        max_terminals: None,
        ring_buffer_max_mb: None,
        api_key: Some(provider.api_key.clone()),
        base_url: Some(provider.base_url.clone()),
        model: Some(provider.model.clone()),
        extra_env: Some(provider.extra_env.clone()),
        wizard_completed: None,
        default_backend: None,
        active_provider_id: Some(Some(provider_id.to_string())),
    };
    // P0-1 修复：不再用 app.manage 替换托管 Arc（Tauri 2 对已管理类型是静默 no-op）。
    // 直接把新值写入 ConfigManager 的 Arc<RwLock<AppConfig>>，pty_spawn/pty_refresh_env
    // 现在从该 RwLock 读当前值，因此后续 spawn/restart 立即读到新 creds。
    {
        let cfg_arc = cfg_mgr.arc();
        let mut cfg = cfg_arc.write().await;
        update.apply_to(&mut cfg);
        crate::config::save_to_file(&cfg)?;
    }

    // pty_spawn / pty_refresh_env 现从 ConfigManager 的 Arc<RwLock<AppConfig>> 读当前值，
    // 上面的 cfg_arc.write().await + save_to_file 已让后续 spawn/restart 读到新 creds。

    // 全局隔离：同步到 onecode 专属隔离 CC 配置（Claude Code settings.json env
    // 优先级最高，实际路由以它为准；不写 = 注入的进程 env 被全局 settings 盖掉）
    if let Err(e) = sync_active_to_claude_settings(&provider) {
        log::warn!("[providers] switch: sync isolated CC settings failed: {e}");
    }

    // ② 写目录 → providers.json
    cat.active_provider_id = Some(provider_id.to_string());
    // 队列自愈（AC-F3.6）：队列空 → 当前设为 P1；当前不在队列 → 插到队首
    if cat.failover_queue.is_empty() {
        cat.failover_queue.push(provider_id.to_string());
    } else {
        cat.failover_queue.retain(|q| q != provider_id);
        cat.failover_queue.insert(0, provider_id.to_string());
    }

    // ③ history 追加（reason 由调用方给：`manual` / `consecutive_failure`），FIFO 50
    cat.history.push(SwitchEvent {
        time: chrono::Utc::now().to_rfc3339(),
        from: old_active.clone().unwrap_or_else(|| "<none>".to_string()),
        to: provider_id.to_string(),
        reason: reason.to_string(),
    });
    if cat.history.len() > HISTORY_LIMIT {
        let drop = cat.history.len() - HISTORY_LIMIT;
        cat.history.drain(..drop);
    }
    save_to_file(&cat)?;

    // ④ emit provider-switched（前端：芯片更新 + 重启会话 + toast）
    let payload = ProviderSwitchedPayload {
        from: old_active.unwrap_or_default(),
        to: provider_id.to_string(),
        reason: reason.to_string(),
    };
    let _ = app.emit("provider-switched", payload.clone());

    log::info!(
        "[providers] switched -> {} (manual), from={:?}",
        provider_id,
        payload.from
    );
    Ok(())
}

/// `provider-switched` 事件载荷
#[derive(Clone, Debug, Serialize)]
pub struct ProviderSwitchedPayload {
    pub from: String,
    pub to: String,
    pub reason: String,
}

// ── 连通性测试（AC-F3.3）────────────────────────────────────────────

/// 测试结果（前端 F3 每行「测试」展示）
#[derive(Clone, Debug, Serialize)]
pub struct TestConnectionResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

impl TestConnectionResult {
    fn ok(latency_ms: u64) -> Self {
        Self {
            ok: true,
            latency_ms: Some(latency_ms),
            error: None,
        }
    }

    fn failure(msg: &str) -> Self {
        Self {
            ok: false,
            latency_ms: None,
            error: Some(msg.to_string()),
        }
    }
}

/// 测试供应商连通性：发一条最小 completion，返回延迟与状态。
/// M1 用系统 curl（macOS 内置）避免引入 HTTP 依赖；Phase 2 代理层再上 reqwest。
pub async fn test_connection(provider: &Provider) -> TestConnectionResult {
    let base = provider.base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() || provider.api_key.trim().is_empty() {
        return TestConnectionResult::failure("Base URL 或 API Key 未配置");
    }

    let is_anthropic = base.contains("anthropic.com");
    let endpoint = if is_anthropic {
        format!("{base}/v1/messages")
    } else {
        format!("{base}/chat/completions")
    };
    let body = serde_json::json!({
        "model": provider.model,
        "max_tokens": 8,
        "messages": [{"role": "user", "content": "ping"}]
    })
    .to_string();

    // 在 async 上下文中调用阻塞子进程，避免阻塞 tokio 运行时主线程
    let endpoint_for_task = endpoint.clone();
    let api_key = provider.api_key.clone();
    let body_for_task = body.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-s", "-o", "/dev/null", "-w", "%{http_code} %{time_total}"])
            .arg("-m").arg("15")
            .arg("-X").arg("POST")
            .arg("-H").arg("Content-Type: application/json");
        if is_anthropic {
            cmd.arg("-H").arg(format!("x-api-key: {api_key}"))
                .arg("-H").arg("anthropic-version: 2023-06-01");
        } else {
            cmd.arg("-H").arg(format!("Authorization: Bearer {api_key}"));
        }
        cmd.arg("-d").arg(&body_for_task).arg(&endpoint_for_task);
        cmd.output()
    })
    .await
    .map_err(|e| TestConnectionResult::failure(&format!("curl 执行失败: {e}")));

    match result {
        Ok(Ok(out)) => {
            if !out.status.success() {
                return TestConnectionResult::failure("curl 退出码非 0（网络不可达？）");
            }
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut parts = stdout.split_whitespace();
            let code = parts.next().unwrap_or("000");
            let time = parts.next().unwrap_or("0");
            let latency_ms = (time.parse::<f64>().unwrap_or(0.0) * 1000.0).round() as u64;
            match code {
                "200" => TestConnectionResult::ok(latency_ms),
                "401" | "403" => TestConnectionResult::failure("鉴权失败（API Key 无效或额度用尽）"),
                "404" => TestConnectionResult::failure("端点或模型不存在（检查 Base URL / 型号名）"),
                "429" => TestConnectionResult::failure("请求过于频繁（限流）"),
                "000" => TestConnectionResult::failure("连接失败（网络不可达或超时）"),
                _ => TestConnectionResult::failure(&format!("HTTP {code}")),
            }
        }
        Ok(Err(e)) => TestConnectionResult::failure(&format!("无法调用 curl: {e}")),
        Err(res) => res,
    }
}

// ── 输入结构（Tauri command 反序列化）────────────────────────────────

/// 新增供应商输入（预设或自定义四要素）。
///
/// PRD 原先裁「上下文窗口等高级字段不做」（O3，触发条件是「有真实用户提诉求」）。
/// **触发条件已达成**：2026-09-23 董事长本人撞了两次 provider 400（上下文溢出），
/// 所以窗口从「不做」变成「可选填」——四要素流程不变，不填就按
/// 型号后缀 / 内置表 / 保守默认自动推导（见 `model_context`）。
#[derive(Debug, Deserialize)]
pub struct ProviderInput {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 附加环境变量（KEY=VALUE）；缺省 None = 空
    #[serde(default)]
    pub extra_env: Option<HashMap<String, String>>,
    /// 模型上下文窗口（token）；缺省 None = 自动推导
    #[serde(default)]
    pub context_window: Option<u32>,
}

/// 编辑供应商输入（仅覆盖提供的字段）
#[derive(Debug, Deserialize)]
pub struct ProviderUpdate {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub extra_env: Option<HashMap<String, String>>,
    /// `Some(None)` = 显式清空（回到自动推导）；`None` = 不改。
    /// 用双层 Option 是必须的——单层分不出「不改」和「清空」，
    /// 同一类坑在素材库那边见过（「我就是要它空」≠「我没改过」）。
    #[serde(default, deserialize_with = "double_option")]
    pub context_window: Option<Option<u32>>,
}

/// 双层 Option 反序列化：`{"context_window": null}` → `Some(None)`（清空），
/// 字段缺席 → `None`（不动）。serde 默认把两者都变成 `None`。
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(de)?))
}

/// 生成 slug id（自定义供应商用）
fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if c.is_ascii_whitespace() || c == '-' || c == '_' {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        format!("provider-{}", chrono::Utc::now().timestamp())
    } else {
        out
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// HOME 是**进程全局** env，而 cargo test 并行跑用例 —— 任何改 HOME 的测试
    /// 都可能踩到别人。作者已在 `sync_active_writes_recommended_env` 的注释里
    /// 记录过这个坑；这里把它变成一份共享纪律：**互斥 + RAII 恢复**。
    ///
    /// 顺带堵住一个更严重的隐患：若测试不换 HOME，`save_to_file` 会去写真实的
    /// `~/.onecode/providers.json`，等于跑一次测试就覆写用户的供应商列表。
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 把 HOME 指向一个唯一的临时目录，返回的 guard 同时持有互斥锁，
    /// 并在 drop 时（包括 panic 展开时）恢复原 HOME。
    struct TempHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        orig: String,
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            std::env::set_var("HOME", &self.orig);
        }
    }

    fn with_temp_home() -> TempHome {
        // 被毒化也要能继续：一个用例 panic 不该让整批测试连锁失败
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig = std::env::var("HOME").unwrap_or_default();
        // ★ 用单调计数器而不是 thread id 做目录键：串行模式（--test-threads=1）下
        // 所有用例跑在同一个线程上，用 thread id 会让它们共用同一个 HOME 目录，
        // 于是 providers.json 之类的状态在用例之间泄漏（实测：并行全绿、串行红）。
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("onecode-test-home-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);
        TempHome { _lock: lock, orig }
    }

    fn catalog_with_two() -> ProviderCatalog {
        ProviderCatalog {
            providers: vec![
                Provider {
                    id: "glm-5.2".into(),
                    name: "GLM-5.2".into(),
                    base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                    api_key: "sk-a".into(),
                    model: "glm-5.2".into(),
                    extra_env: HashMap::new(),
                    context_window: None,
                    created_at: "2026-08-08T00:00:00Z".into(),
                },
                Provider {
                    id: "deepseek-v4-flash".into(),
                    name: "DeepSeek-V4-Flash".into(),
                    base_url: "https://api.deepseek.com".into(),
                    api_key: "".into(),
                    model: "deepseek-v4-flash".into(),
                    extra_env: HashMap::new(),
                    context_window: None,
                    created_at: "2026-08-08T00:00:00Z".into(),
                },
            ],
            failover_queue: vec!["glm-5.2".into(), "deepseek-v4-flash".into()],
            auto_failover_enabled: true,
            failover_params: FailoverParams::default(),
            history: vec![],
            active_provider_id: Some("glm-5.2".into()),
            roster: HashMap::new(),
        }
    }

    #[test]
    fn history_fifo_keeps_last_50() {
        let mut cat = ProviderCatalog::default();
        for i in 0..51 {
            cat.history.push(SwitchEvent {
                time: format!("2026-08-08T00:00:{i:02}Z"),
                from: "a".into(),
                to: "b".into(),
                reason: "manual".into(),
            });
            if cat.history.len() > HISTORY_LIMIT {
                let drop = cat.history.len() - HISTORY_LIMIT;
                cat.history.drain(..drop);
            }
        }
        assert_eq!(cat.history.len(), 50);
        // 最旧的被淘汰：第一条 time 应为 00:01（第 0 条被淘汰）
        assert_eq!(cat.history.first().unwrap().time, "2026-08-08T00:00:01Z");
        assert_eq!(cat.history.last().unwrap().time, "2026-08-08T00:00:50Z");
    }

    #[test]
    fn delete_current_provider_blocked() {
        let store = ProviderStore::new(catalog_with_two());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(delete_provider(&store, "glm-5.2".into()));
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("当前使用中"));
    }

    #[test]
    fn delete_last_provider_blocked() {
        let mut cat = catalog_with_two();
        cat.active_provider_id = Some("deepseek-v4-flash".into());
        cat.providers.retain(|p| p.id == "deepseek-v4-flash");
        let store = ProviderStore::new(cat);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(delete_provider(&store, "deepseek-v4-flash".into()));
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("至少保留"));
    }

    #[test]
    fn delete_non_current_cleans_refs() {
        // ★ 必须在临时 HOME 下跑。否则 `delete_provider` → `save_to_file` 会去写
        // **真实的** `~/.onecode/providers.json` —— 跑一次 cargo test 就可能把用户
        // 真实的供应商列表覆写成测试夹具。这个测试以前"绿"是因为同套件里另一个
        // 用例用 set_var("HOME") 泄漏了临时 HOME，纯属偶然（并行时序一变就红）。
        // `let _ = ...` 会**立即 drop** guard、HOME 马上被恢复 —— 必须绑具名变量。
        let _home = with_temp_home();
        let store = ProviderStore::new(catalog_with_two());
        let rt = tokio::runtime::Runtime::new().unwrap();
        // 先手动塞一条历史引用 deepseek
        {
            let cat_arc = store.arc();
            let mut cat = cat_arc.blocking_write();
            cat.history.push(SwitchEvent {
                time: "2026-08-08T00:00:00Z".into(),
                from: "deepseek-v4-flash".into(),
                to: "glm-5.2".into(),
                reason: "manual".into(),
            });
        }
        rt.block_on(delete_provider(&store, "deepseek-v4-flash".into())).unwrap();
        let cat_arc = store.arc();
        let cat = cat_arc.blocking_read();
        assert!(cat.providers.iter().all(|p| p.id != "deepseek-v4-flash"));
        assert!(cat.failover_queue.iter().all(|q| q != "deepseek-v4-flash"));
        assert!(cat.history.iter().all(|h| h.from != "deepseek-v4-flash" && h.to != "deepseek-v4-flash"));
    }

    /// 用临时 HOME 验证隔离 settings.json 落地。
    ///
    /// **所有场景必须串在这一个测试里**：HOME 是进程全局 env，`cc_config_dir()`
    /// 每次调用都读它，而 cargo test 并行跑用例——分成两个 `#[test]` 会互相踩
    /// （实测踩过：一个用例断言窗口是 262144、另一个刚把 HOME 换掉）。
    /// HOME 的切换与恢复统一走 `with_temp_home()`（互斥 + RAII）。
    #[test]
    fn sync_active_writes_recommended_env() {
        let _home = with_temp_home();
        let tmp = std::path::PathBuf::from(std::env::var("HOME").unwrap());

        // 场景 A：DeepSeek 官方推荐配置（API_KEY 模式 + 差异化槽位 + 1M 窗口）
        let ds = Provider {
            id: "deepseek-v4-flash".into(),
            name: "DeepSeek-V4".into(),
            base_url: "https://api.deepseek.com/anthropic".into(),
            api_key: "sk-test".into(),
            model: "deepseek-v4-pro[1m]".into(),
            context_window: None,
            extra_env: HashMap::from([
                ("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), "deepseek-v4-pro[1m]".into()),
                ("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), "deepseek-v4-pro[1m]".into()),
                ("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), "deepseek-v4-flash".into()),
                ("CLAUDE_CODE_SUBAGENT_MODEL".into(), "deepseek-v4-flash".into()),
                ("CLAUDE_CODE_EFFORT_LEVEL".into(), "max".into()),
                ("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "786432".into()),
            ]),
            created_at: String::new(),
        };
        super::sync_active_to_claude_settings(&ds).unwrap();

        let path = tmp.join(".onecode/cc-config/settings.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let env = v["env"].as_object().unwrap();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://api.deepseek.com/anthropic");
        // extra_env 未声明 AUTH_TOKEN → 仍写 API_KEY
        assert_eq!(env["ANTHROPIC_API_KEY"], "sk-test");
        // extra_env 覆盖默认模型槽位
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-v4-flash");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "deepseek-v4-flash");
        assert_eq!(env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "786432");
        // 声明了模型真实窗口（`[1m]` 后缀 → 1048576）。不声明就是 2026-09-23 那两起 400：
        // Claude Code 按「未识别型号」的默认值办事，会话一路涨过 provider 的上限才撞墙。
        assert_eq!(env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "1048576");
        // 用户 extra_env 里显式给了 AUTO_COMPACT_WINDOW（env 优先于设置项）→
        // 不再写 settings 键，免得 /config 里出现「被环境变量盖住」的困惑提示
        assert!(
            v.get("autoCompactWindow").is_none(),
            "extra_env 已显式指定窗口时不写 settings 键"
        );

        // 场景 B：extra_env 显式声明 AUTH_TOKEN → 不写 API_KEY（避免 auth 冲突）
        let p = Provider {
            id: "p".into(),
            name: "P".into(),
            base_url: "https://x.test/anthropic".into(),
            api_key: "sk-a".into(),
            model: "m1".into(),
            context_window: None,
            extra_env: HashMap::from([("ANTHROPIC_AUTH_TOKEN".into(), "tok-1".into())]),
            created_at: String::new(),
        };
        super::sync_active_to_claude_settings(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let env = v["env"].as_object().unwrap();
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "tok-1");

        // ── 场景 C：**2026-09-23 第一起 400 的回归** ─────────────────────
        //
        // 阿里百炼的 `qwen3.8-27b`：真窗口 262144，但供应商条目 `extra_env` 为空
        // （是「自定义新增」的，不是预设）→ 没声明窗口 → Claude Code 按「未识别型号」
        // 的默认值办事 → 会话涨到 392074 → provider 400。
        let bailian = Provider {
            id: "provider-1787399285".into(),
            name: "百炼".into(),
            base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic".into(),
            api_key: "sk-sp-test".into(),
            model: "qwen3.8-27b".into(),
            extra_env: HashMap::new(), // ← 关键：正是出事的那一条
            context_window: None,
            created_at: String::new(),
        };
        super::sync_active_to_claude_settings(&bailian).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "262144",
            "没把模型真实窗口声明出去 —— 这就是那起 400 的根因"
        );
        // 阈值必须离窗口足够远：实测单轮就能暴涨 23 万 token（阈值检查在上一轮之后）。
        // 这里钉**精确值**——离线脚本（provider 配置的批处理/排查）会照这个数复刻，
        // 差一位就悄悄退回 400。
        let compact = v["autoCompactWindow"].as_u64().unwrap();
        assert_eq!(compact, 151_501, "262144 - 32000(补全) - 65536(暴涨余量) - 13107(兜底)");

        // ── 场景 D：切走之后声明必须跟着换（残留的窗口声明比不声明更糟）────
        let flash = Provider {
            id: "ds-flash".into(),
            name: "DeepSeek-Flash".into(),
            base_url: "https://api.deepseek.com/anthropic".into(),
            api_key: "sk-a".into(),
            model: "deepseek-v4-flash[1m]".into(),
            extra_env: HashMap::new(),
            context_window: None,
            created_at: String::new(),
        };
        super::sync_active_to_claude_settings(&flash).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "1048576");
        assert_eq!(
            v["autoCompactWindow"].as_u64().unwrap(),
            702_004,
            "1048576 - 32000 - 262144 - 52428；这是 deepseek 那起 400 的修复值"
        );

        // ── 场景 E：用户手填的窗口优先（他可能有我们不知道的信息）────────
        let mut manual = bailian.clone();
        manual.context_window = Some(32_768);
        super::sync_active_to_claude_settings(&manual).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "32768");

        // ── 场景 F：一方模型（Claude 系）**什么都不声明** ────────────────
        //
        // CLI 的 `auto` 比自己维护一张表准；我们插手的唯一效果是把窗口调小
        //（内置表里没有 Claude 型号 → 落到 128k 保守默认 → 200k 的模型被按 90k 压缩）。
        let mut native = bailian.clone();
        native.model = "claude-sonnet-5".into();
        native.base_url = "https://api.anthropic.com".into();
        super::sync_active_to_claude_settings(&native).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            !v["env"].as_object().unwrap().contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            "对一方模型不该声明窗口（会把 auto 调好的值压小）"
        );
        assert!(v.get("autoCompactWindow").is_none(), "切回一方模型要清掉我们写的阈值");
        // 走代理的 Claude 型号也一样（base_url 不是 anthropic.com，但型号名骗不了人）
        native.base_url = "https://my-proxy.test/anthropic".into();
        super::sync_active_to_claude_settings(&native).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!v["env"].as_object().unwrap().contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));

        let _ = std::fs::remove_dir_all(&tmp);
        // HOME 的恢复交给 `_home` 的 Drop（panic 时也会恢复）
    }

    #[test]
    fn catalog_view_carries_window_plans() {
        // 界面靠这份数据把「窗口 / 阈值 / 来源」显示出来；
        // 缺了它，用户看到的就是一个不知道哪来的数字
        let view = ProviderCatalogView::from_catalog(catalog_with_two());
        // JSON 形状必须向后兼容：providers 还在顶层（flatten）
        let json = serde_json::to_value(&view).unwrap();
        assert!(json["providers"].is_array());
        assert_eq!(json["providers"].as_array().unwrap().len(), 2);
        // glm-5.2 没有内置表条目 → 必须是 default，且来源写成中文给界面直接用
        assert_eq!(view.windows["glm-5.2"].source, "default");
        assert_eq!(view.window_sources["glm-5.2"], "未识别型号（保守值）");
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Provider"), "my-provider");
        assert_eq!(slugify("GLM-5.2"), "glm-52");
        let empty = slugify("  ");
        assert!(empty.starts_with("provider-"), "got {empty}");
    }

    // ── resolve_route_for_agent：自选模型的核心判断（唯一实现）────────────

    fn provider(id: &str, model: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("https://{id}.example/v1"),
            api_key: format!("key-{id}"),
            model: model.to_string(),
            extra_env: HashMap::from([("MARK".to_string(), id.to_string())]),
            context_window: None,
            created_at: String::new(),
        }
    }

    fn catalog() -> ProviderCatalog {
        let mut c = ProviderCatalog::default();
        c.providers = vec![provider("deepseek", "deepseek-chat"), provider("glm", "glm-4")];
        c.active_provider_id = Some("deepseek".to_string());
        c
    }

    fn global_mirror() -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.api_key = "key-deepseek".to_string();
        cfg.base_url = "https://deepseek.example/v1".to_string();
        cfg.model = "deepseek-chat".to_string();
        cfg.extra_env = HashMap::from([("MARK".to_string(), "deepseek".to_string())]);
        cfg.active_provider_id = Some("deepseek".to_string());
        cfg
    }

    #[test]
    fn route_uses_named_provider_not_global() {
        // 关键：指定了非激活供应商 → 必须用它的凭据，而不是全局快照
        let r = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("glm"), None);
        assert_eq!(r.provider_id.as_deref(), Some("glm"));
        assert_eq!(r.base_url, "https://glm.example/v1");
        assert_eq!(r.api_key, "key-glm");
        assert_eq!(r.model, "glm-4");
        assert_eq!(r.extra_env.get("MARK").map(String::as_str), Some("glm"));
        assert!(!r.fallback);
    }

    #[test]
    fn route_without_request_falls_back_to_global_silently() {
        let r = resolve_route_for_agent(&catalog(), &global_mirror(), None, None, None);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(r.api_key, "key-deepseek");
        assert!(!r.fallback, "没指定就不算回退");
    }

    #[test]
    fn route_unknown_provider_falls_back_and_flags_it() {
        // 静默用错供应商比启动失败更难查 → 必须能被告知
        let r = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("nope"), None);
        assert!(r.fallback);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(r.api_key, "key-deepseek");
    }

    #[test]
    fn route_model_override_wins_over_provider_model() {
        let r = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("deepseek"), Some("deepseek-reasoner"));
        assert_eq!(r.model, "deepseek-reasoner");
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        // 只改模型，凭据仍来自该供应商
        assert_eq!(r.api_key, "key-deepseek");
    }

    #[test]
    fn route_blank_inputs_are_treated_as_absent() {
        let r = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("   "), Some("  "));
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(r.model, "deepseek-chat");
        assert!(!r.fallback, "空白请求不算请求");
    }

    // ── apply_route_env：路由真正落到 env 的地方 ──────────────────────

    /// 与 claude-code 内置档一致的 env 映射（owned，因为 `apply_route_env` 现在收 owned）。
    fn cc_map() -> Vec<(String, String)> {
        [
            ("api_key", "ANTHROPIC_API_KEY"),
            ("base_url", "ANTHROPIC_BASE_URL"),
            ("model", "ANTHROPIC_MODEL"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    #[test]
    fn apply_env_writes_the_named_provider_credentials() {
        let route = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("glm"), None);
        let mut env = HashMap::new();
        apply_route_env(&cc_map(), &route, &mut env);
        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("key-glm"));
        assert_eq!(env.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("https://glm.example/v1"));
        assert_eq!(env.get("ANTHROPIC_MODEL").map(String::as_str), Some("glm-4"));
        assert_eq!(env.get("MARK").map(String::as_str), Some("glm"));
    }

    /// 这条钉住的正是"换供应商其实没换"的失效模式：调用方传来的上一轮 env
    /// 若不先清掉，会盖住本次路由（不报错、不崩溃、静默用错供应商）。
    #[test]
    fn apply_env_clears_stale_credentials_from_the_caller_env() {
        let mut env = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "stale-key".to_string()),
            ("ANTHROPIC_BASE_URL".to_string(), "https://stale.example".to_string()),
            ("ANTHROPIC_MODEL".to_string(), "stale-model".to_string()),
            ("UNRELATED".to_string(), "keep-me".to_string()),
        ]);
        let route = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("glm"), None);
        apply_route_env(&cc_map(), &route, &mut env);

        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("key-glm"));
        assert_eq!(env.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("https://glm.example/v1"));
        assert_eq!(env.get("ANTHROPIC_MODEL").map(String::as_str), Some("glm-4"));
        // 只碰自己声明的键
        assert_eq!(env.get("UNRELATED").map(String::as_str), Some("keep-me"));
    }

    /// 空凭据不留旧值：路由里 api_key 为空时必须把键清掉，
    /// 否则会拿上一轮（或另一个供应商）的 key 去请求。
    #[test]
    fn apply_env_empty_value_removes_the_key_instead_of_keeping_old() {
        let mut route = resolve_route_for_agent(&catalog(), &global_mirror(), None, Some("glm"), None);
        route.api_key = String::new();
        let mut env = HashMap::from([("ANTHROPIC_API_KEY".to_string(), "stale".to_string())]);
        apply_route_env(&cc_map(), &route, &mut env);
        assert_eq!(env.get("ANTHROPIC_API_KEY"), None, "空 api_key 必须清掉旧值");
    }

    /// 两个槽跑不同供应商 → 两份互不相同的 env（目标 ② 的最小证明）。
    #[test]
    fn two_slots_get_different_credentials() {
        let cat = catalog();
        let g = global_mirror();
        let mut a = HashMap::new();
        let mut b = HashMap::new();
        let map = cc_map();
        apply_route_env(
            &map,
            &resolve_route_for_agent(&cat, &g, None, Some("deepseek"), None),
            &mut a,
        );
        apply_route_env(&map, &resolve_route_for_agent(&cat, &g, None, Some("glm"), None), &mut b);
        assert_ne!(a.get("ANTHROPIC_BASE_URL"), b.get("ANTHROPIC_BASE_URL"));
        assert_ne!(a.get("ANTHROPIC_API_KEY"), b.get("ANTHROPIC_API_KEY"));
        assert_ne!(a.get("ANTHROPIC_MODEL"), b.get("ANTHROPIC_MODEL"));
    }

    // ── 员工级路线：花名册（按员工自选模型）+ failover ──────────────────

    /// 一个**没有凭据**的 provider（failover 的触发条件）。
    fn provider_no_key(id: &str, model: &str) -> Provider {
        let mut p = provider(id, model);
        p.api_key = String::new();
        p.extra_env = HashMap::new();
        p
    }

    #[test]
    fn roster_binds_an_employee_to_a_provider_and_says_so() {
        let mut c = catalog();
        c.roster.insert(
            "dev".into(),
            RosterEntry { provider_id: "glm".into(), model: Some("glm-4-plus".into()) },
        );
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("glm"));
        assert_eq!(r.api_key, "key-glm", "必须用花名册那个供应商的凭据");
        assert_eq!(r.model, "glm-4-plus", "花名册指定的模型要生效");
        assert_eq!(r.source, RouteSource::Roster);
        assert!(r.switched_from.is_none());
        assert!(r.switch_reason.is_none());
    }

    #[test]
    fn explicit_request_beats_the_roster() {
        let mut c = catalog();
        c.roster.insert(
            "dev".into(),
            RosterEntry { provider_id: "glm".into(), model: None },
        );
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), Some("deepseek"), None);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"), "调用方显式指定优先级最高");
        assert_eq!(r.source, RouteSource::Requested);
    }

    #[test]
    fn employee_without_a_roster_entry_uses_the_global_route() {
        let c = catalog();
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("nobody"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(r.source, RouteSource::Global);
        assert!(!r.fallback, "没指定过 provider，不算 fallback");
    }

    #[test]
    fn roster_model_is_empty_means_use_the_providers_default() {
        let mut c = catalog();
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: None });
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.model, "glm-4", "用 provider 自己的默认模型");
    }

    #[test]
    fn roster_pointing_at_a_missing_provider_fails_over_with_a_reason() {
        let mut c = catalog();
        c.failover_queue = vec!["glm".into()];
        c.roster.insert(
            "dev".into(),
            RosterEntry { provider_id: "ghost".into(), model: None },
        );
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("glm"), "按撤退队列换到可用的那个");
        assert_eq!(r.switched_from.as_deref(), Some("ghost"));
        let why = r.switch_reason.unwrap_or_default();
        assert!(why.contains("ghost"), "原因里要写清原路线：{why}");
    }

    #[test]
    fn provider_without_credentials_triggers_failover_along_the_queue() {
        let mut c = catalog();
        // glm 没凭据（花名册指向它）→ 应沿 failover_queue 找到 deepseek
        c.providers = vec![provider_no_key("glm", "glm-4"), provider("deepseek", "deepseek-chat")];
        c.failover_queue = vec!["glm".into(), "deepseek".into()];
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: None });

        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_eq!(r.api_key, "key-deepseek");
        assert_eq!(r.switched_from.as_deref(), Some("glm"));
        assert!(r.switch_reason.unwrap_or_default().contains("没有凭据"));
    }

    /// **安全规矩**（第 2 轮定下的）：花名册里写的是 A 供应商的模型名，
    /// failover 换到 B 供应商时**绝不能**把这个模型名带过去。
    #[test]
    fn failover_does_not_carry_the_roster_model_to_another_provider() {
        let mut c = catalog();
        c.providers = vec![provider_no_key("glm", "glm-4"), provider("deepseek", "deepseek-chat")];
        c.failover_queue = vec!["deepseek".into()];
        c.roster.insert(
            "dev".into(),
            RosterEntry { provider_id: "glm".into(), model: Some("glm-4-plus-only-on-glm".into()) },
        );
        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("deepseek"));
        assert_ne!(
            r.model, "glm-4-plus-only-on-glm",
            "换到别家后不能把原供应商的模型名硬塞过去"
        );
        assert_eq!(r.model, "deepseek-chat", "应退回目标 provider 自己的默认模型");
    }

    #[test]
    fn failover_disabled_reports_instead_of_switching_silently() {
        let mut c = catalog();
        c.auto_failover_enabled = false;
        c.providers = vec![provider_no_key("glm", "glm-4"), provider("deepseek", "deepseek-chat")];
        c.failover_queue = vec!["deepseek".into()];
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: None });

        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert!(r.switched_from.is_none(), "关了自动切换就不能换");
        assert!(
            r.switch_reason.unwrap_or_default().contains("自动切换已关闭"),
            "但必须如实说出来"
        );
    }

    #[test]
    fn exhausted_failover_queue_explains_what_was_skipped() {
        let mut c = catalog();
        c.providers = vec![
            provider_no_key("glm", "glm-4"),
            provider_no_key("kimi", "kimi-k2"),
        ];
        c.failover_queue = vec!["glm".into(), "kimi".into(), "ghost".into()];
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: None });

        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        let why = r.switch_reason.unwrap_or_default();
        assert!(why.contains("kimi"), "要说明试过谁：{why}");
        assert!(why.contains("ghost"), "不在目录里的也要提到：{why}");
        assert!(r.switched_from.is_none(), "没换成就不该标 switched_from");
    }

    #[test]
    fn record_switch_is_ring_limited() {
        let mut c = ProviderCatalog::default();
        for i in 0..(HISTORY_LIMIT + 5) {
            record_switch(&mut c, "a", &format!("b{i}"), "failover");
        }
        assert_eq!(c.history.len(), HISTORY_LIMIT);
        assert_eq!(c.history.last().unwrap().to, format!("b{}", HISTORY_LIMIT + 4));
    }

    /// 回归：花名册指向不存在的 provider、全局恰好可用时，
    /// **不能**把 source 标成 Roster 却实际走全局 —— 来源标注必须与事实一致。
    #[test]
    fn source_label_always_matches_where_credentials_came_from() {
        let mut c = catalog();
        c.roster.insert("dev".into(), RosterEntry { provider_id: "ghost".into(), model: None });
        c.failover_queue = vec![]; // 没有撤退路线 → 只能回退全局

        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        // 实际用的是全局凭据（deepseek），所以来源不可能是 Roster
        assert_eq!(r.api_key, "key-deepseek", "实际用了全局凭据");
        assert_ne!(r.source, RouteSource::Roster, "用了全局凭据就不能标成花名册");
        assert!(r.fallback, "回退语义仍要标出来");
        let why = r.switch_reason.unwrap_or_default();
        assert!(why.contains("ghost"), "要说清花名册指向了谁：{why}");
    }

    #[test]
    fn route_source_serialises_to_stable_strings() {
        assert_eq!(RouteSource::Requested.as_str(), "requested");
        assert_eq!(RouteSource::Roster.as_str(), "roster");
        assert_eq!(RouteSource::Global.as_str(), "global");
    }

    // ── 花名册视图 ────────────────────────────────────────────────────

    #[test]
    fn roster_view_shows_binding_and_actual_side_by_side() {
        let mut c = catalog();
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: Some("glm-4".into()) });
        let rows = roster_view(&c, &global_mirror());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_id, "dev");
        assert_eq!(rows[0].provider_id, "glm");
        assert!(rows[0].provider_exists);
        assert_eq!(rows[0].resolved_provider.as_deref(), Some("glm"));
        assert_eq!(rows[0].resolved_source, "roster");
        assert!(rows[0].switched_from.is_none());
    }

    #[test]
    fn roster_view_marks_a_binding_whose_provider_is_gone() {
        let mut c = catalog();
        c.roster.insert("dev".into(), RosterEntry { provider_id: "ghost".into(), model: None });
        let rows = roster_view(&c, &global_mirror());
        assert!(!rows[0].provider_exists, "不在了要标出来");
        let text = roster_text(&rows, &c.providers);
        assert!(text.contains("已不在目录里"), "{text}");
    }

    #[test]
    fn roster_text_lists_usable_and_unusable_providers() {
        let mut c = catalog();
        c.providers.push(provider_no_key("broken", "b-1"));
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: None });
        let text = roster_text(&roster_view(&c, &global_mirror()), &c.providers);
        assert!(text.contains("deepseek"), "{text}");
        assert!(text.contains("broken(不可用)"), "不可用的要标出来：{text}");
    }

    #[test]
    fn empty_roster_text_tells_you_how_to_fill_it() {
        let text = roster_text(&[], &catalog().providers);
        assert!(text.contains("/roster"), "空花名册要给出下一步怎么做：{text}");
    }

    /// 取证：花名册在人眼前长什么样（含"绑了但已切走"这种最需要看见的情况）。
    #[test]
    fn roster_text_demo() {
        let mut c = catalog();
        c.providers = vec![
            provider("glm", "glm-4"),
            provider_no_key("kimi", "kimi-k2"),
            provider("deepseek", "deepseek-chat"),
        ];
        c.failover_queue = vec!["kimi".into(), "deepseek".into()];
        c.roster.insert("dev".into(), RosterEntry { provider_id: "glm".into(), model: Some("glm-4".into()) });
        // 这个绑到了一个"没凭据"的供应商 → 会被切走
        c.roster.insert("researcher".into(), RosterEntry { provider_id: "kimi".into(), model: None });
        let text = roster_text(&roster_view(&c, &global_mirror()), &c.providers);
        eprintln!("===== 员工模型花名册（人看到的）=====\n{text}\n===== end =====");
        assert!(text.contains("@researcher"));
        assert!(text.contains("已从 kimi 切走"), "切走必须可见：{text}");
        assert!(text.contains("kimi(不可用)"));
    }

    // ── 本地模型档（目标 ② 明确要求"含本地模型档"）────────────────────

    #[test]
    fn local_endpoint_is_recognised() {
        assert!(is_local_endpoint("http://localhost:11434/v1"));
        assert!(is_local_endpoint("http://127.0.0.1:1234/v1"));
        assert!(is_local_endpoint("http://0.0.0.0:8000/v1"));
        assert!(is_local_endpoint("HTTP://LocalHost:11434/v1"), "大小写不敏感");
        assert!(!is_local_endpoint("https://api.deepseek.com"));
        assert!(!is_local_endpoint(""));
    }

    /// **本地档不能被"没有凭据"判死**：否则开了 auto_failover 就会被静默换到云上，
    /// 正好与"我要用本地模型"相反。
    #[test]
    fn local_provider_needs_no_credentials_and_is_not_failed_over() {
        let mut local = provider_no_key("ollama", "qwen3:32b");
        local.name = "Ollama".into();
        local.base_url = "http://localhost:11434/v1".into();

        let mut c = catalog();
        c.providers = vec![local, provider("deepseek", "deepseek-chat")];
        c.failover_queue = vec!["deepseek".into()];
        c.roster.insert("dev".into(), RosterEntry { provider_id: "ollama".into(), model: Some("qwen3:32b".into()) });

        let r = resolve_route_for_agent(&c, &global_mirror(), Some("dev"), None, None);
        assert_eq!(r.provider_id.as_deref(), Some("ollama"), "本地档必须被保留");
        assert_eq!(r.model, "qwen3:32b");
        assert!(r.switched_from.is_none(), "不该切走");
        assert!(r.switch_reason.is_none(), "更不该报错");
        assert_eq!(r.source, RouteSource::Roster);
    }

    #[test]
    fn local_and_remote_are_judged_separately() {
        let mut local = provider_no_key("ollama", "qwen3");
        local.base_url = "http://localhost:11434/v1".into();
        assert!(provider_usable(&local).is_ok(), "本地档无凭据也可用");
        let remote = provider_no_key("kimi", "kimi-k2");
        assert!(provider_usable(&remote).is_err(), "云上无凭据仍算不可用");
        let mut no_url = provider_no_key("broken", "x");
        no_url.base_url = String::new();
        assert!(provider_usable(&no_url).is_err(), "没 base_url 谁都不行");
    }

    #[test]
    fn catalog_view_carries_usability_so_the_frontend_need_not_rejudge() {
        let mut local = provider_no_key("ollama", "qwen3");
        local.base_url = "http://localhost:11434/v1".into();
        let mut c = catalog();
        c.providers.push(local);
        c.providers.push(provider_no_key("kimi", "kimi-k2"));

        let view = ProviderCatalogView::from_catalog(c);
        assert_eq!(view.usable.get("ollama"), Some(&true), "本地档可用");
        assert_eq!(view.unusable_reason.get("ollama").map(|s| s.is_empty()), Some(true));
        assert_eq!(view.usable.get("kimi"), Some(&false));
        assert!(view.unusable_reason.get("kimi").unwrap().contains("没有凭据"));
        assert_eq!(view.usable.get("deepseek"), Some(&true));
    }

    // ── 熔断（②）：判定要**保守** ──────────────────────────────────────

    fn usage(i: u64, o: u64) -> crate::engine_run::RunUsage {
        crate::engine_run::RunUsage {
            input_tokens: i,
            output_tokens: o,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        }
    }

    fn report_with(result: Option<crate::engine_run::RunResult>) -> crate::engine_run::RunReport {
        crate::engine_run::RunReport {
            ok: result.as_ref().map(|r| !r.is_error).unwrap_or(false),
            cmd: "engine".to_string(),
            args: Vec::new(),
            cwd: "/tmp".to_string(),
            init: None,
            result,
            assistant_turns: 0,
            wall_ms: 100,
            error: None,
            stderr_tail: String::new(),
            exit_kind: crate::engine_run::ExitKind::Completed,
        }
    }

    fn ok_result() -> crate::engine_run::RunResult {
        crate::engine_run::RunResult {
            subtype: "success".to_string(),
            is_error: false,
            num_turns: 1,
            duration_ms: 100,
            duration_api_ms: 90,
            total_cost_usd: 0.01,
            reply: "好".to_string(),
            session_id: "s".to_string(),
            usage: usage(100, 10),
            model_usage: Vec::new(),
            permission_denials: 0,
        }
    }

    #[test]
    fn a_successful_run_is_a_success() {
        let r = report_with(Some(ok_result()));
        assert_eq!(classify_run(&r), RunOutcome::Success);
    }

    /// 引擎报执行错误、且**一点推理都没做** → 供应商层面的失败。
    #[test]
    fn an_execution_error_before_any_inference_is_a_provider_failure() {
        let mut res = ok_result();
        res.is_error = true;
        res.total_cost_usd = 0.0;
        res.usage = usage(0, 0);
        assert_eq!(classify_run(&report_with(Some(res))), RunOutcome::ProviderFailure);
    }

    /// ★ 超时 / 没有结算 → **说不清**，绝不能算成供应商失败。
    /// 依据：第 20 刀实测过"模型想用工具时会挂住 >90 秒"——那是工具问题。
    /// 拿它换供应商会把好供应商换掉，所以宁可漏判。
    /// ★ 第 25 刀真机验证发现的缺口：**供应商连不上时引擎不产生 result 事件**，
    /// 于是熔断永远抓不到连通性故障。修法：没有结算时按**结束方式**再分。
    #[test]
    fn a_process_that_exits_without_any_result_is_a_provider_failure() {
        let mut r = report_with(None);
        r.exit_kind = crate::engine_run::ExitKind::ExitedWithoutResult;
        assert_eq!(classify_run(&r), RunOutcome::ProviderFailure, "自己退了、一轮都没成");
    }

    /// 超时仍是"说不清"（可能只是模型想用工具挂住了 —— 换供应商没用）。
    #[test]
    fn a_timeout_is_still_inconclusive() {
        let mut r = report_with(None);
        r.exit_kind = crate::engine_run::ExitKind::TimedOut;
        assert_eq!(classify_run(&r), RunOutcome::Inconclusive);
    }

    /// 二进制都不在 → 换供应商同样救不了。
    #[test]
    fn a_spawn_failure_is_inconclusive_because_switching_providers_cannot_help() {
        let mut r = report_with(None);
        r.exit_kind = crate::engine_run::ExitKind::SpawnFailed;
        assert_eq!(classify_run(&r), RunOutcome::Inconclusive);
    }

    #[test]
    fn a_timeout_with_no_result_is_inconclusive_never_a_failure() {
        assert_eq!(classify_run(&report_with(None)), RunOutcome::Inconclusive);
    }

    /// 推理已经做过才报错 → 是模型/应用层的问题，不是"这个供应商连不上"。
    #[test]
    fn an_error_after_real_inference_is_inconclusive() {
        let mut res = ok_result();
        res.is_error = true;
        assert_eq!(classify_run(&report_with(Some(res))), RunOutcome::Inconclusive);
    }

    #[test]
    fn the_breaker_counts_consecutive_failures_and_success_resets() {
        let mut b = Breaker::default();
        assert_eq!(b.record("p1", RunOutcome::ProviderFailure), 1);
        assert_eq!(b.record("p1", RunOutcome::ProviderFailure), 2);
        assert_eq!(b.consecutive("p1"), 2);
        assert_eq!(b.record("p1", RunOutcome::Success), 0);
        assert_eq!(b.consecutive("p1"), 0);
        // 不同供应商各算各的
        b.record("p1", RunOutcome::ProviderFailure);
        b.record("p2", RunOutcome::ProviderFailure);
        assert_eq!(b.consecutive("p1"), 1);
        assert_eq!(b.consecutive("p2"), 1);
    }

    /// "说不清"既不加也不清零 —— 否则偶发一次超时就能把连败洗掉。
    #[test]
    fn inconclusive_neither_increments_nor_resets() {
        let mut b = Breaker::default();
        b.record("p1", RunOutcome::ProviderFailure);
        b.record("p1", RunOutcome::ProviderFailure);
        assert_eq!(b.record("p1", RunOutcome::Inconclusive), 2, "不该被动过");
        assert_eq!(b.consecutive("p1"), 2);
    }

    #[test]
    fn the_breaker_ignores_an_empty_provider_id() {
        let mut b = Breaker::default();
        assert_eq!(b.record("", RunOutcome::ProviderFailure), 0);
        assert_eq!(b.consecutive(""), 0);
    }

    fn cat_with(queue: &[&str], ids: &[(&str, &str, &str)]) -> ProviderCatalog {
        let mut cat = ProviderCatalog::default();
        cat.providers = ids
            .iter()
            .map(|(id, url, key)| Provider {
                id: id.to_string(),
                name: id.to_string(),
                base_url: url.to_string(),
                api_key: key.to_string(),
                model: "m".to_string(),
                extra_env: std::collections::HashMap::new(),
                context_window: None,
                created_at: String::new(),
            })
            .collect();
        cat.failover_queue = queue.iter().map(|s| s.to_string()).collect();
        cat
    }

    #[test]
    fn next_provider_follows_the_queue_and_skips_self_and_unusable() {
        // 队列: [p1(自己), p2(没有 key 且非本地 → 不可用), p3(可用)]
        let cat = cat_with(
            &["p1", "p2", "p3"],
            &[
                ("p1", "https://a.example/v1", "k1"),
                ("p2", "https://b.example/v1", ""),
                ("p3", "https://c.example/v1", "k3"),
            ],
        );
        assert_eq!(next_usable_provider(&cat, "p1").as_deref(), Some("p3"));
    }

    #[test]
    fn next_provider_falls_back_to_catalog_order_when_the_queue_is_exhausted() {
        let cat = cat_with(
            &["p1"],
            &[("p1", "https://a.example/v1", "k1"), ("z9", "https://z.example/v1", "kz")],
        );
        assert_eq!(next_usable_provider(&cat, "p1").as_deref(), Some("z9"));
    }

    #[test]
    fn no_candidate_when_the_current_one_is_the_only_usable_provider() {
        let cat = cat_with(&["p1"], &[("p1", "https://a.example/v1", "k1")]);
        assert_eq!(next_usable_provider(&cat, "p1"), None);
    }
}
