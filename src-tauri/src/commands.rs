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

use crate::cc_status::CcStatusCache;
use crate::config::AppConfig;
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
pub async fn pty_spawn(
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    label: Option<String>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
    config: State<'_, Arc<AppConfig>>,
) -> Result<SpawnResult, String> {
    let cmd = cmd.unwrap_or_else(|| config.default_cmd.clone());
    let args = args.unwrap_or_else(|| config.default_args.clone());
    let cwd = cwd.unwrap_or_else(|| config.default_cwd.clone());
    let env = env.unwrap_or_default();
    let (id, pid) = state
        .spawn(cmd, args, cwd, env, data_channel, label)
        .await
        .map_err(|e| e.to_string())?;
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
pub async fn pty_replay(
    id: String,
    state: State<'_, MultiPtyManager>,
) -> Result<Vec<u8>, String> {
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
    state
        .save_all(&slots)
        .await
        .map_err(|e| e.to_string())
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
pub async fn health_check(
    state: State<'_, MultiPtyManager>,
) -> Result<Vec<HealthReport>, String> {
    let summaries = state.list().await;
    Ok(check_health(&summaries))
}
