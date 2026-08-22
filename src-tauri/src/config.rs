//! 应用配置（支持从 ~/.onecode/desktop.json 读取 + 持久化）。
//!
//! M1 用默认值；M2+ 从配置文件读取，前端可通过 Settings 面板修改。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    /// 默认启动命令
    pub default_cmd: String,
    /// 默认参数
    pub default_args: Vec<String>,
    /// 默认工作目录（未指定 cwd 时）
    pub default_cwd: String,
    /// 最大并发终端数
    pub max_terminals: usize,
    /// 每个 slot 的 ring buffer 上限（MB）
    pub ring_buffer_max_mb: usize,
    /// API Key（Anthropic / 兼容 API）
    #[serde(default)]
    pub api_key: String,
    /// API Base URL
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// AI 模型标识
    #[serde(default = "default_model")]
    pub model: String,
    /// 供应商附加环境变量（DeepSeek 官方推荐配置等；KEY=VALUE，随 spawn 注入子进程）
    #[serde(default)]
    pub extra_env: HashMap<String, String>,
    /// Setup Wizard 是否已完成
    #[serde(default)]
    pub wizard_completed: bool,
    /// 当前激活的 provider id（providers.json 目录中的引用；None = 回退现状单供应商）
    #[serde(default)]
    pub active_provider_id: Option<String>,
    /// 默认后端内核（"claude-code" / "opencode" / "codex" 等）
    #[serde(default = "default_backend")]
    pub default_backend: String,
}

fn default_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_backend() -> String {
    "claude-code".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let default_cwd = format!("{home}/.onecode/workspace", home = home);
        Self {
            default_cmd: "claude".to_string(),
            default_args: vec![
                "--permission-mode".into(),
                "bypassPermissions".into(),
                "--dangerously-skip-permissions".into(),
            ],
            default_cwd,
            max_terminals: 30,
            ring_buffer_max_mb: 10,
            extra_env: HashMap::new(),
            api_key: String::new(),
            base_url: default_base_url(),
            model: default_model(),
            wizard_completed: false,
            active_provider_id: None,
            default_backend: default_backend(),
        }
    }
}

/// 配置文件路径：~/.onecode/desktop.json
pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".onecode/desktop.json")
}

/// 从文件加载配置，文件不存在则返回默认值
pub fn load_from_file() -> AppConfig {
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    log::warn!(
                        "[config] parse {} failed: {e}, using defaults",
                        path.display()
                    );
                }
            },
            Err(e) => {
                log::warn!(
                    "[config] read {} failed: {e}, using defaults",
                    path.display()
                );
            }
        }
    }
    AppConfig::default()
}

/// 保存配置到文件
pub fn save_to_file(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    // Ensure ~/.onecode/ exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    let content =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize failed: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("write {} failed: {e}", path.display()))?;
    log::info!("[config] saved to {}", path.display());
    Ok(())
}

/// 可变配置管理器（Arc<RwLock> 允许前端通过 IPC 修改配置）
#[derive(Clone)]
pub struct ConfigManager {
    inner: Arc<RwLock<AppConfig>>,
}

impl ConfigManager {
    pub fn new(initial: AppConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }

    pub fn arc(&self) -> Arc<RwLock<AppConfig>> {
        self.inner.clone()
    }
}

/// 前端 Settings 面板提交的配置片段（所有字段可选，仅更新提供的字段）
#[derive(Debug, Deserialize, Default)]
pub struct ConfigUpdate {
    pub default_cmd: Option<String>,
    pub default_args: Option<Vec<String>>,
    pub default_cwd: Option<String>,
    pub max_terminals: Option<usize>,
    pub ring_buffer_max_mb: Option<usize>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub extra_env: Option<HashMap<String, String>>,
    pub wizard_completed: Option<bool>,
    pub default_backend: Option<String>,
    /// 外 Option = 是否提供该字段；内 Option = 值（None 表示清除 provider 指针）
    pub active_provider_id: Option<Option<String>>,
}

impl ConfigUpdate {
    /// 合并到 AppConfig（仅覆盖有值的字段）
    pub fn apply_to(&self, cfg: &mut AppConfig) {
        if let Some(v) = &self.default_cmd {
            cfg.default_cmd = v.clone();
        }
        if let Some(v) = &self.default_args {
            cfg.default_args = v.clone();
        }
        if let Some(v) = &self.default_cwd {
            cfg.default_cwd = v.clone();
        }
        if let Some(v) = &self.max_terminals {
            cfg.max_terminals = *v;
        }
        if let Some(v) = &self.ring_buffer_max_mb {
            cfg.ring_buffer_max_mb = *v;
        }
        if let Some(v) = &self.api_key {
            cfg.api_key = v.clone();
        }
        if let Some(v) = &self.base_url {
            cfg.base_url = v.clone();
        }
        if let Some(v) = &self.model {
            cfg.model = v.clone();
        }
        if let Some(v) = &self.extra_env {
            cfg.extra_env = v.clone();
        }
        if let Some(v) = &self.wizard_completed {
            cfg.wizard_completed = *v;
        }
        if let Some(v) = &self.default_backend {
            cfg.default_backend = v.clone();
        }
        if let Some(v) = &self.active_provider_id {
            cfg.active_provider_id = v.clone();
        }
    }
}

/// 启动配置文件热加载：每 2 秒轮询 ~/.onecode/desktop.json，
/// 检测外部变更（agent 直接修改）后自动重载配置并通知前端。
pub fn start_config_watcher(mut_cfg: Arc<RwLock<AppConfig>>, app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let path = config_path();
        let mut last_content: Option<String> = None;

        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let current = match fs::read_to_string(&path) {
                Ok(c) => Some(c),
                Err(_) => {
                    // 文件不存在或无法读取 → 记录空哨兵（仅首次）
                    if last_content.is_none() {
                        last_content = Some(String::new());
                    }
                    continue;
                }
            };

            if current == last_content {
                continue;
            }

            // 内容发生变化 → 解析并重载
            if let Some(ref content) = current {
                match serde_json::from_str::<AppConfig>(content) {
                    Ok(cfg) => {
                        let mut w = mut_cfg.write().await;
                        *w = cfg;
                        log::info!("[config] reloaded from external change ({})", path.display());
                        let _ = app_handle.emit("config-changed", ());
                    }
                    Err(e) => {
                        log::warn!("[config] external file parse failed: {e}");
                    }
                }
            }
            last_content = current;
        }
    });
}
