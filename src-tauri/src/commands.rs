//! Tauri invoke 命令层（前端 → Rust）。
//!
//! 架构修订（review §2）：数据传输用 Channel<Vec<u8>>，不再 base64。
//! - pty_spawn / pty_restart 接收 `data_channel: Channel<Vec<u8>>`（流式二进制）。
//! - pty_write 的 data 是 `Vec<u8>`（零编码）。
//! - pty_replay 返回 `Vec<u8>`（Tab 切换回放）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use crate::cc_sessions::CcSessionsCache;
use crate::cc_status::CcStatusCache;
use crate::config::{AppConfig, ConfigManager, ConfigUpdate};
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
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    label: Option<String>,
    project_id: Option<String>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
    config: State<'_, Arc<AppConfig>>,
) -> Result<SpawnResult, String> {
    let cmd = cmd.unwrap_or_else(|| config.default_cmd.clone());
    let args = args.unwrap_or_else(|| config.default_args.clone());
    let cwd = {
        let c = cwd.unwrap_or_else(|| config.default_cwd.clone());
        // 确保默认工作目录存在
        std::fs::create_dir_all(&c).ok();
        c
    };
    // 合并配置中的 API 环境变量（Wizard 设置的 api_key/base_url/model）
    let mut env = env.unwrap_or_default();
    if !config.api_key.is_empty() {
        env.insert("ANTHROPIC_API_KEY".to_string(), config.api_key.clone());
    }
    if !config.base_url.is_empty() {
        env.insert("ANTHROPIC_BASE_URL".to_string(), config.base_url.clone());
    }
    if !config.model.is_empty() {
        env.insert("ANTHROPIC_MODEL".to_string(), config.model.clone());
    }
    log::info!("[pty_spawn] cmd={cmd:?} args={args:?} cwd={cwd:?} project_id={project_id:?}");
    let (id, pid) = state
        .spawn(cmd, args, cwd, env, data_channel, label, project_id)
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
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .restart(parse_id(&id)?, data_channel)
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
            created_at: c.created_at,
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

// ── 项目管理 ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub dir: String,
    pub description: String,
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
                if path.extension().map_or(true, |e| e != "json") { continue; }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if val.get("name").and_then(|v| v.as_str()) == Some(&project.name) {
                            existing_id = val.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                            existing_created = val.get("created_at").and_then(|v| v.as_str()).map(|s| s.to_string());
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
        "created_at": created_at,
        "updated_at": now,
    });
    let path = projects_dir.join(format!("{}.json", id));
    let content =
        serde_json::to_string_pretty(&data).map_err(|e| format!("serialize: {e}"))?;
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
                    val.as_object_mut().map(|o| {
                        o.insert("id".to_string(), serde_json::Value::String(id.clone()));
                    });
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
    // Deduplicate by name — keep the latest (most recent updated_at), remove older duplicates
    {
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (i, p) in projects.iter().enumerate() {
            if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
                let my_time = p.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(&prev_i) = seen.get(name) {
                    let prev_time = projects[prev_i].get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                    if my_time >= prev_time {
                        seen.insert(name.to_string(), i);
                    }
                    // else keep prev_i
                } else {
                    seen.insert(name.to_string(), i);
                }
            }
        }
        let keep: std::collections::HashSet<usize> = seen.values().copied().collect();
        // Remove orphaned JSON files for duplicate projects
        let remove_indices: Vec<usize> = (0..projects.len()).filter(|i| !keep.contains(i)).collect();
        for &i in &remove_indices {
            if let Some(val) = projects[i].get("id").and_then(|v| v.as_str()) {
                let orphan = projects_dir.join(format!("{}.json", val));
                let _ = std::fs::remove_file(&orphan);
                log::info!("[project] removed duplicate: {} ({})",
                    projects[i].get("name").and_then(|v| v.as_str()).unwrap_or("?"), val);
            }
        }
        if !remove_indices.is_empty() {
            let mut i = 0usize;
            projects.retain(|_| { let keep = remove_indices.binary_search(&i).is_err(); i += 1; keep });
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
