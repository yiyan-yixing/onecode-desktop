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

use crate::config::{ConfigManager, ConfigUpdate};

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
}

impl ProviderCatalogView {
    pub fn from_catalog(catalog: ProviderCatalog) -> Self {
        let mut windows = HashMap::new();
        let mut window_sources = HashMap::new();
        for p in &catalog.providers {
            let plan = p.window_plan();
            windows.insert(p.id.clone(), plan);
            window_sources.insert(p.id.clone(), crate::model_context::source_label(plan.source));
        }
        Self {
            catalog,
            windows,
            window_sources,
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

/// onecode 专属的 Claude Code 隔离配置目录：`~/.onecode/cc-config`。
///
/// pty 启动时通过 `CLAUDE_CONFIG_DIR` 注入，让 onecode 里跑的 claude 只读这份
/// 隔离配置，完全不经手/不依赖用户全局的 `~/.claude/settings.json`
/// （全局隔离，董事长 2026-08-22 指令：不改全局、不读全局）。
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
pub async fn perform_switch(
    app: &AppHandle,
    cfg_mgr: &ConfigManager,
    store: &ProviderStore,
    provider_id: &str,
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

    // ③ history 追加（reason=manual），FIFO 50
    cat.history.push(SwitchEvent {
        time: chrono::Utc::now().to_rfc3339(),
        from: old_active.clone().unwrap_or_else(|| "<none>".to_string()),
        to: provider_id.to_string(),
        reason: "manual".to_string(),
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
        reason: "manual".to_string(),
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
    #[test]
    fn sync_active_writes_recommended_env() {
        let orig_home = std::env::var("HOME").unwrap_or_default();
        let tmp = std::env::temp_dir().join(format!("onecode-cc-test-{}", chrono::Utc::now().timestamp()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);

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
        std::env::set_var("HOME", &orig_home);
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
}
