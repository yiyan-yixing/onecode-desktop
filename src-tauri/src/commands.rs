//! Tauri invoke 命令层（前端 → Rust）。
//!
//! 架构修订（review §2）：数据传输用 Channel<Vec<u8>>，不再 base64。
//! - pty_spawn / pty_restart 接收 `data_channel: Channel<Vec<u8>>`（流式二进制）。
//! - pty_write 的 data 是 `Vec<u8>`（零编码）。
//! - pty_replay 返回 `Vec<u8>`（Tab 切换回放）。

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use crate::config::AppConfig;
use crate::pty::{MultiPtyManager, SlotSummary};
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
