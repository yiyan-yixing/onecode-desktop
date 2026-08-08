//! Tauri invoke 命令层（前端 → Rust）。
//!
//! 架构修订（review §2）：数据传输用 Channel<Vec<u8>>，不再 base64。
//! - pty_spawn / pty_restart 接收 `data_channel: Channel<Vec<u8>>`（流式二进制）。
//! - pty_write 的 data 是 `Vec<u8>`（零编码）。
//! - pty_replay 返回 `Vec<u8>`（Tab 切换回放）。

use std::collections::HashMap;
use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

use crate::cc_sessions::CcSessionsCache;
use crate::cc_status::CcStatusCache;
use crate::config::{AppConfig, ConfigManager, ConfigUpdate};
use crate::providers::{self, ProviderInput, ProviderStore, ProviderUpdate};
use crate::pty::health::{check_health, HealthReport};
use crate::pty::{MultiPtyManager, SlotConfig, SlotSummary};
use crate::session::{PersistentSlot, SessionStore};

#[derive(serde::Serialize)]
pub struct SpawnResult {
    pub id: String,
    pub pid: Option<u32>,
}

fn parse_id(s: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(s).map_err(|e| format!("invalid id: {e}"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_spawn(
    backend: Option<String>,
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    label: Option<String>,
    project_id: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<SpawnResult, String> {
    // 从 ConfigManager 读当前配置快照（M1 切档后新 spawn 读到新 creds；修复 P0-1：
    // 托管 Arc<AppConfig> 在 Tauri 2 无法覆盖，必须读可变 RwLock 而非冻结快照）
    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;
    let config = &*config; // RwLockReadGuard → &AppConfig
    // 根据后端 profile 解析默认命令/参数/环境变量
    let effective_backend = backend.as_deref().unwrap_or(&config.default_backend);
    let profile = crate::backend::resolve_or_default(effective_backend);

    let cmd = cmd.unwrap_or_else(|| profile.cmd.to_string());
    let args = args.unwrap_or_else(|| profile.default_args.iter().map(|s| s.to_string()).collect());
    let cwd = {
        let c = cwd.unwrap_or_else(|| config.default_cwd.clone());
        // 确保默认工作目录存在
        std::fs::create_dir_all(&c).ok();
        c
    };
    // 根据 profile 的 env_key_map 注入环境变量
    let mut env = env.unwrap_or_default();
    for (config_key, env_var) in profile.env_key_map {
        match *config_key {
            "api_key" if !config.api_key.is_empty() => {
                env.insert(env_var.to_string(), config.api_key.clone());
            }
            "base_url" if !config.base_url.is_empty() => {
                env.insert(env_var.to_string(), config.base_url.clone());
            }
            "model" if !config.model.is_empty() => {
                env.insert(env_var.to_string(), config.model.clone());
            }
            _ => {}
        }
    }
    log::info!("[pty_spawn] backend={effective_backend} cmd={cmd:?} args={args:?} cwd={cwd:?} project_id={project_id:?} cols={cols:?} rows={rows:?}");
    let (id, pid) = state
        .spawn(
            cmd,
            args,
            cwd,
            env,
            data_channel,
            label,
            project_id,
            effective_backend.to_string(),
            cols,
            rows,
        )
        .await
        .map_err(|e| {
            log::error!("[pty_spawn] FAILED: {e}");
            e.to_string()
        })?;
    log::info!("[pty_spawn] OK id={id} pid={pid:?}");
    Ok(SpawnResult {
        id: id.to_string(),
        pid,
    })
}

#[tauri::command]
pub async fn pty_kill(id: String, state: State<'_, MultiPtyManager>) -> Result<(), String> {
    state.kill(parse_id(&id)?).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_restart(
    id: String,
    cols: Option<u16>,
    rows: Option<u16>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .restart(parse_id(&id)?, data_channel, cols, rows)
        .await
        .map_err(|e| e.to_string())
}

/// 前端键盘输入 → PTY。data 为原始字节（UTF-8）。
#[tauri::command]
pub async fn pty_write(
    id: String,
    data: Vec<u8>,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .write(parse_id(&id)?, data)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_resize(
    id: String,
    cols: u16,
    rows: u16,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .resize(parse_id(&id)?, cols, rows)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_list(state: State<'_, MultiPtyManager>) -> Result<Vec<SlotSummary>, String> {
    Ok(state.list().await)
}

#[tauri::command]
pub async fn pty_rename(
    id: String,
    label: String,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .rename(parse_id(&id)?, label)
        .await
        .map_err(|e| e.to_string())
}

/// Tab 切换时前端拉取 ring buffer 回放（返回原始字节）。
#[tauri::command]
pub async fn pty_replay(id: String, state: State<'_, MultiPtyManager>) -> Result<Vec<u8>, String> {
    state
        .replay(parse_id(&id)?)
        .await
        .map_err(|e| e.to_string())
}

/// 标记指定终端为当前活跃（前端 switchTo 时调用）。
/// 更新 last_active_at 时间戳，供 session 恢复时定位上次使用的终端。
#[tauri::command]
pub async fn pty_set_active(id: String, state: State<'_, MultiPtyManager>) -> Result<(), String> {
    state
        .set_active(parse_id(&id)?)
        .await
        .map_err(|e| e.to_string())
}

// ── P1：会话持久化 ──────────────────────────────────────────────────

#[tauri::command]
pub async fn session_save(
    slots: Vec<PersistentSlot>,
    state: State<'_, SessionStore>,
) -> Result<(), String> {
    state.save_all(&slots).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn session_restore(
    state: State<'_, SessionStore>,
) -> Result<Vec<PersistentSlot>, String> {
    state.load_all().await.map_err(|e| e.to_string())
}

/// 快照当前所有终端配置并写入 SQLite（前端 create/close/rename 时去抖调用）。
/// 无需前端回传配置——以 Rust 侧 slot 为单一事实源。
#[tauri::command]
pub async fn session_persist(
    pty: State<'_, MultiPtyManager>,
    store: State<'_, SessionStore>,
) -> Result<usize, String> {
    let cfgs: Vec<SlotConfig> = pty.snapshot().await;
    let slots: Vec<PersistentSlot> = cfgs
        .into_iter()
        .map(|c| PersistentSlot {
            id: c.id,
            label: c.label,
            project_id: c.project_id,
            cmd: c.cmd,
            args: c.args,
            cwd: c.cwd,
            env: c.env,
            backend: c.backend,
            created_at: c.created_at,
            last_active_at: c.last_active_at,
        })
        .collect();
    let n = slots.len();
    store.save_all(&slots).await.map_err(|e| e.to_string())?;
    Ok(n)
}

// ── P1：CC Status（skills/hooks/plugins/tasks/agents） ──────────────

#[tauri::command]
pub async fn cc_status(
    project_dir: Option<String>,
    state: State<'_, CcStatusCache>,
) -> Result<crate::cc_status::CcStatus, String> {
    let p = project_dir.map(PathBuf::from);
    Ok(state.load(p.as_deref()))
}

/// 清空 CC Status 缓存（前端「刷新」用）。
#[tauri::command]
pub async fn cc_status_invalidate(state: State<'_, CcStatusCache>) -> Result<(), String> {
    state.invalidate();
    Ok(())
}

// ── P1：健康检测 ────────────────────────────────────────────────────

#[tauri::command]
pub async fn health_check(state: State<'_, MultiPtyManager>) -> Result<Vec<HealthReport>, String> {
    let summaries = state.list().await;
    Ok(check_health(&summaries))
}

// ── 配置管理 ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn save_config(
    config: ConfigUpdate,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String> {
    let arc = cfg_mgr.arc();
    let mut cfg = arc.write().await;
    config.apply_to(&mut cfg);
    crate::config::save_to_file(&cfg)
}

#[tauri::command]
pub async fn load_config(cfg_mgr: State<'_, ConfigManager>) -> Result<AppConfig, String> {
    let arc = cfg_mgr.arc();
    let cfg = arc.read().await;
    Ok(cfg.clone())
}

#[derive(serde::Serialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub description: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub default: serde_json::Value,
}

#[tauri::command]
pub async fn get_config_path() -> Result<String, String> {
    Ok(crate::config::config_path()
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub async fn get_config_schema() -> Result<Vec<ConfigField>, String> {
    Ok(vec![
        ConfigField {
            key: "default_cmd".into(),
            label: "默认命令".into(),
            description: "终端启动时执行的默认命令".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude"),
        },
        ConfigField {
            key: "default_args".into(),
            label: "默认参数".into(),
            description: "默认命令的启动参数（数组 / 空格分隔）".into(),
            field_type: "array".into(),
            default: serde_json::json!(["--permission-mode", "bypassPermissions"]),
        },
        ConfigField {
            key: "default_cwd".into(),
            label: "工作目录".into(),
            description: "终端启动时的默认工作目录".into(),
            field_type: "string".into(),
            default: serde_json::json!("~/.onecode/workspace"),
        },
        ConfigField {
            key: "max_terminals".into(),
            label: "最大终端数".into(),
            description: "允许同时运行的最大终端数量".into(),
            field_type: "number".into(),
            default: serde_json::json!(30),
        },
        ConfigField {
            key: "ring_buffer_max_mb".into(),
            label: "缓冲区大小".into(),
            description: "每个终端环形缓冲区的最大大小（MB）".into(),
            field_type: "number".into(),
            default: serde_json::json!(10),
        },
        ConfigField {
            key: "api_key".into(),
            label: "API Key".into(),
            description: "AI API 的认证密钥".into(),
            field_type: "string".into(),
            default: serde_json::json!(""),
        },
        ConfigField {
            key: "base_url".into(),
            label: "Base URL".into(),
            description: "AI API 的基础 URL（如自建代理）".into(),
            field_type: "string".into(),
            default: serde_json::json!("https://api.anthropic.com"),
        },
        ConfigField {
            key: "model".into(),
            label: "Model".into(),
            description: "AI 模型标识符".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude-sonnet-4-6"),
        },
        ConfigField {
            key: "default_backend".into(),
            label: "默认后端".into(),
            description: "终端使用的 AI 后端内核（claude-code / opencode / codex 等）".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude-code"),
        },
    ])
}

// ── 项目管理 ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub dir: String,
    pub description: String,
    #[serde(default)]
    pub backend: Option<String>,
}

/// 保存项目元数据到 ~/.onecode/projects/<id>.json。
/// 同名项目已存在则更新（保留原 id 和 created_at），否则新建。
#[tauri::command]
pub async fn save_project(project: ProjectInfo) -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    std::fs::create_dir_all(&projects_dir).map_err(|e| format!("create projects dir: {e}"))?;

    let now = chrono::Utc::now().to_rfc3339();

    // Check if a project with the same name already exists
    let mut existing_id: Option<String> = None;
    let mut existing_path: Option<std::path::PathBuf> = None;
    let mut existing_created: Option<String> = None;
    if projects_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(true, |e| e != "json") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if val.get("name").and_then(|v| v.as_str()) == Some(&project.name) {
                            existing_id = val
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            existing_created = val
                                .get("created_at")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            existing_path = Some(path);
                            break;
                        }
                    }
                }
            }
        }
    }

    let (id, created_at) = if let (Some(eid), Some(ec)) = (existing_id, existing_created) {
        // Update existing project — remove old file if filename doesn't match id
        if let Some(old) = &existing_path {
            let expected = projects_dir.join(format!("{}.json", eid));
            if *old != expected {
                let _ = std::fs::remove_file(old);
            }
        }
        (eid, ec)
    } else {
        // New project
        (uuid::Uuid::new_v4().to_string(), now.clone())
    };

    let data = serde_json::json!({
        "id": id,
        "name": project.name,
        "dir": project.dir,
        "description": project.description,
        "backend": project.backend,
        "created_at": created_at,
        "updated_at": now,
    });
    let path = projects_dir.join(format!("{}.json", id));
    let content = serde_json::to_string_pretty(&data).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;

    // Migrate: remove old <name>.json if it exists (from v0 format)
    let old_path = projects_dir.join(format!("{}.json", project.name));
    if old_path.exists() && old_path != path {
        let _ = std::fs::remove_file(&old_path);
        log::info!("[project] migrated {} -> {}", old_path.display(), id);
    }

    log::info!("[project] saved {} ({})", project.name, id);
    Ok(id)
}

/// 列出所有已保存的项目。惰性迁移：旧格式 <name>.json（无 id 字段）自动分配 UUID 并重命名。
#[tauri::command]
pub async fn list_projects() -> Result<Vec<serde_json::Value>, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    if !projects_dir.exists() {
        return Ok(vec![]);
    }
    let mut projects = vec![];
    let entries = std::fs::read_dir(&projects_dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
                // Lazy migration: assign UUID if missing
                if val.get("id").is_none() {
                    let id = uuid::Uuid::new_v4().to_string();
                    if let Some(o) = val.as_object_mut() {
                        o.insert("id".to_string(), serde_json::Value::String(id.clone()));
                    }
                    // Write back and rename to <id>.json
                    if let Ok(new_content) = serde_json::to_string_pretty(&val) {
                        let new_path = projects_dir.join(format!("{}.json", id));
                        if std::fs::write(&new_path, &new_content).is_ok() {
                            let _ = std::fs::remove_file(&path);
                            log::info!("[project] migrated {} -> {}", path.display(), id);
                        }
                    }
                }
                projects.push(val);
            }
        }
    }
    // Deduplicate by name AND by dir — keep the latest, remove older duplicates.
    // Same-name or same-dir projects are considered duplicates (prevents
    // two project cards pointing to the same directory from showing).
    {
        let mut seen_name: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut seen_dir: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, p) in projects.iter().enumerate() {
            let my_time = p.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
                if let Some(&prev_i) = seen_name.get(name) {
                    let prev_time = projects[prev_i]
                        .get("updated_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if my_time >= prev_time {
                        seen_name.insert(name.to_string(), i);
                    }
                } else {
                    seen_name.insert(name.to_string(), i);
                }
            }
            if let Some(dir) = p.get("dir").and_then(|v| v.as_str()) {
                if !dir.is_empty() {
                    if let Some(&prev_i) = seen_dir.get(dir) {
                        let prev_time = projects[prev_i]
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if my_time >= prev_time {
                            seen_dir.insert(dir.to_string(), i);
                        }
                    } else {
                        seen_dir.insert(dir.to_string(), i);
                    }
                }
            }
        }
        let keep: std::collections::HashSet<usize> = seen_name
            .values()
            .copied()
            .chain(seen_dir.values().copied())
            .collect();
        // Remove orphaned JSON files for duplicate projects
        let remove_indices: Vec<usize> =
            (0..projects.len()).filter(|i| !keep.contains(i)).collect();
        for &i in &remove_indices {
            if let Some(val) = projects[i].get("id").and_then(|v| v.as_str()) {
                let orphan = projects_dir.join(format!("{}.json", val));
                let _ = std::fs::remove_file(&orphan);
                log::info!(
                    "[project] removed duplicate: {} ({})",
                    projects[i]
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    val
                );
            }
        }
        if !remove_indices.is_empty() {
            let mut i = 0usize;
            projects.retain(|_| {
                let keep = remove_indices.binary_search(&i).is_err();
                i += 1;
                keep
            });
        }
    }
    // Sort by created_at descending
    projects.sort_by(|a, b| {
        let ta = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        tb.cmp(ta)
    });
    Ok(projects)
}

/// 删除项目元数据（不删除目录和关联终端）。按 name 字段查找并删除对应 JSON。
#[tauri::command]
pub async fn delete_project(name: String) -> Result<(), String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    if !projects_dir.exists() {
        return Err(format!("项目 '{}' 不存在", name));
    }
    let entries = std::fs::read_dir(&projects_dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if val.get("name").and_then(|v| v.as_str()) == Some(&name) {
                    std::fs::remove_file(&path).map_err(|e| format!("删除失败: {e}"))?;
                    log::info!("[project] deleted {} ({})", name, path.display());
                    return Ok(());
                }
            }
        }
    }
    Err(format!("项目 '{}' 不存在", name))
}

// ── CC Sessions ──────────────────────────────────────────────────

#[tauri::command]
pub async fn cc_sessions_list(
    project_dir: Option<String>,
    cache: State<'_, CcSessionsCache>,
) -> Result<Vec<crate::cc_sessions::CcSession>, String> {
    // No filter by default — show conversations from all projects.
    // Frontend can pass project_dir to filter by a specific project.
    Ok(cache.load(project_dir.as_deref()))
}

// ── 系统信息 ──────────────────────────────────────────────────────────

/// 返回当前用户的 home 目录（跨平台）。
/// HOME (Unix/macOS) / USERPROFILE (Windows) / 回退 "."。
#[tauri::command]
pub async fn get_home_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    Ok(home)
}

/// 前端调试日志：JS → Rust → 终端 stdout
#[tauri::command]
pub fn debug_log(tag: String, msg: String) {
    log::info!("[js:{tag}] {msg}");
}

// ── 外链 / 编辑器打开（optimization-005 / 007）───────────────────────

/// Ctrl/Cmd+Click 链接：用系统默认浏览器打开外链。
/// 走 std::process 直接调系统 opener（mac `open` / win `start` / linux `xdg-open`），
/// 不依赖 GUI PATH、无需额外插件权限（tauri-plugin-shell 的 open 已被官方标记 deprecated）。
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    open_with_default(&url)
}

/// "用 VS Code 打开"：用本地 VS Code 打开目录/文件。
/// 优先 `code` CLI，失败时按平台回退（mac `open -a`、win `cmd /C code`、linux `xdg-open`）。
#[tauri::command]
pub fn open_in_vscode(path: String) -> Result<(), String> {
    spawn_editor(&path)
}

/// 用系统默认处理器打开 URL/路径（mac `open` / win `start` / linux `xdg-open`）。
fn open_with_default(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(target_os = "linux")]
    {
        return std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = target;
        Err("unsupported platform".into())
    }
}

fn spawn_editor(path: &str) -> Result<(), String> {
    // 优先 code CLI（若已装 "Install 'code' in PATH"）。
    // GUI app 的 PATH 受限（mac 上通常不含 /usr/local/bin），此 spawn 多数会失败 → 走平台回退。
    if std::process::Command::new("code")
        .arg(path)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        // open -a 按应用名定位，不依赖 PATH；VS Code 未安装时 spawn 失败返回错误
        return std::process::Command::new("open")
            .args(["-a", "Visual Studio Code", path])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open VS Code failed: {e}"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "code", path])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open VS Code failed: {e}"));
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = path;
        Err("unsupported platform".into())
    }
}

// ── Provider 管理（M1：多供应商 + 手动切换）────────────────────────

/// 列出全部供应商 + 目录元数据（前端 F3 / 芯片渲染用）
#[tauri::command]
pub async fn providers_list(store: State<'_, ProviderStore>) -> Result<crate::providers::ProviderCatalog, String> {
    let cat = store.arc().read().await.clone();
    Ok(cat)
}

/// 列出两档预置供应商（新增时下拉带出默认 base_url + 型号）
#[tauri::command]
pub async fn providers_presets() -> Result<Vec<crate::providers::ProviderPreset>, String> {
    Ok(crate::providers::PRESETS.to_vec())
}

/// 新增供应商（预设或自定义四要素）
#[tauri::command]
pub async fn providers_add(
    provider: ProviderInput,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<crate::providers::Provider, String> {
    let saved = providers::add_provider(&store, provider).await?;
    // P2-3: 首个供应商自动激活后，把 creds 同步到 desktop.json（复用切换链路）
    let was_auto_active = {
        let cat_arc = store.arc();
        let cat = cat_arc.read().await;
        cat.active_provider_id.as_deref() == Some(saved.id.as_str())
    };
    if was_auto_active {
        providers::perform_switch(&app, &cfg_mgr, &store, &saved.id).await?;
    }
    let _ = app.emit("providers-changed", ());
    Ok(saved)
}

/// 编辑供应商
#[tauri::command]
pub async fn providers_update(
    id: String,
    updates: ProviderUpdate,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::update_provider(&store, &cfg_mgr, id, updates).await?;
    let _ = app.emit("providers-changed", ());
    Ok(())
}

/// 删除供应商（删除约束由后端强制：仅剩 1 家 / 当前使用中不可删）
#[tauri::command]
pub async fn providers_delete(
    id: String,
    app: AppHandle,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::delete_provider(&store, id).await?;
    let _ = app.emit("providers-changed", ());
    Ok(())
}

/// 测试供应商连通性（AC-F3.3：最小 completion → 延迟 + 状态）
#[tauri::command]
pub async fn providers_test(
    id: String,
    store: State<'_, ProviderStore>,
) -> Result<crate::providers::TestConnectionResult, String> {
    let cat_arc = store.arc();
    let provider = {
        let cat = cat_arc.read().await;
        cat.providers
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| format!("供应商不存在: {id}"))?
    };
    // 先 drop 读锁再 await（curl -m 15 阻塞期间不让写操作等锁）
    Ok(providers::test_connection(&provider).await)
}

/// 手动切换（F2：写 desktop.json + providers.json → 刷新托管 AppConfig → emit provider-switched）
#[tauri::command]
pub async fn providers_switch(
    provider_id: String,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::perform_switch(&app, &cfg_mgr, &store, &provider_id).await
}

/// 切档后刷新指定 slot 的 env（ADR 坑①：pty.restart 复用旧 env，必须先刷新）
/// 前端在 provider-switched 后对每个运行中 tab 先调本命令再 pty_restart。
#[tauri::command]
pub async fn pty_refresh_env(
    id: String,
    state: State<'_, MultiPtyManager>,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String> {
    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;
    state
        .refresh_env(parse_id(&id)?, &config)
        .await
        .map_err(|e| e.to_string())
}
