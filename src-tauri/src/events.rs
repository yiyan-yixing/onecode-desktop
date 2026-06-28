//! IPC 事件（Rust → 前端）。
//!
//! 架构修订（见 desktop-architecture-review.md §2/§8）：
//! PTY 输出走 `Channel<Vec<u8>>` 流式二进制推送，**不走** `pty:data:{id}` 事件。
//! 此处仅保留低频的进程退出事件，用 `app.emit` 推送。

/// PTY 进程退出事件名：`pty:exit:{id}`，payload 为 exit code (i32)。
pub fn pty_exit_event(id: &str) -> String {
    format!("pty:exit:{id}")
}

/// PTY 进程自动重启成功事件名：`pty:restart:{id}`，payload 为新 pid (Option<u32>)。
/// 当后端自动重启（非用户手动 restart）成功时 emit，通知前端状态从 exited → running。
pub fn pty_restart_event(id: &str) -> String {
    format!("pty:restart:{id}")
}
