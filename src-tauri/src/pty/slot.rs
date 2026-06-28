//! TerminalSlot + RingBuffer + SlotStatus
//!
//! 设计来源：
//! - RingBuffer 重写自 `onecode/agent-runtime/gateway/pty.js` 的 `_chunks[]` /
//!   `_bufferLength` / `_flattenAndTrim()`（line 49-180）。
//! - 自动重启策略见 `pty.js:135-164`，实现位于 `mod.rs` 的退出监听线程。
//!
//! 并发模型（架构修订 §4）：
//! 每个 TerminalSlot 内部的 PTY 句柄有**独立 Mutex**，不同 slot 的 write 不互斥；
//! MultiPtyManager 仅在结构变更（spawn/kill）时取写锁。

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use portable_pty::{ChildKiller, MasterPty, PtySize};
use uuid::Uuid;

use crate::recover_lock;

/// 10MB ring buffer per slot（对齐 pty.js MAX_BUFFER_SIZE）
#[allow(dead_code)]
pub const RING_BUFFER_MAX_BYTES: usize = 10 * 1024 * 1024;

/// 终端状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    Running,
    Exited {
        code: i32,
    },
    Restarting,
    /// 连续重启超过上限，放弃
    Crashed,
}

impl SlotStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SlotStatus::Running => "running",
            SlotStatus::Exited { .. } => "exited",
            SlotStatus::Restarting => "restarting",
            SlotStatus::Crashed => "crashed",
        }
    }
}

/// 环形缓冲区 — 缓存 PTY 输出，供 Tab 切换时 replay。
///
/// 对齐 pty.js 语义：`chunks` 存原始字节段，超 `1.1 × max` 时 flatten 裁剪头部。
/// `dirty/cached_replay` 缓存 replay 结果，避免每次 Tab 切换都拼接。
pub struct RingBuffer {
    chunks: Vec<Vec<u8>>,
    total_len: usize,
    max_size: usize,
    dirty: bool,
    cached_replay: Option<Vec<u8>>,
}

impl RingBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            chunks: Vec::new(),
            total_len: 0,
            max_size,
            dirty: true,
            cached_replay: None,
        }
    }

    pub fn push(&mut self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        self.total_len += data.len();
        self.chunks.push(data);
        self.dirty = true;
        // 超 1.1x 时裁剪（与 pty.js 一致，避免每个小 chunk 都裁）
        if self.total_len > self.max_size + self.max_size / 10 {
            self.trim();
        }
    }

    /// 拼接全部 chunk 并裁掉超出部分（保留尾部 max_size 字节）。
    /// 等价于 pty.js 的 `_flattenAndTrim()`。
    fn trim(&mut self) {
        if self.total_len <= self.max_size {
            return;
        }
        let mut flat = Vec::with_capacity(self.total_len);
        for c in &self.chunks {
            flat.extend_from_slice(c);
        }
        let drop_len = flat.len().saturating_sub(self.max_size);
        flat.drain(0..drop_len);
        self.chunks = vec![flat];
        self.total_len = self.chunks[0].len();
        self.dirty = true;
    }

    /// 全量回放（带缓存）。Tab 切换时由 `pty_replay` 返回给前端。
    pub fn replay(&mut self) -> Vec<u8> {
        if !self.dirty {
            if let Some(cached) = &self.cached_replay {
                return cached.clone();
            }
        }
        let mut out = Vec::with_capacity(self.total_len);
        for c in &self.chunks {
            out.extend_from_slice(c);
        }
        self.cached_replay = Some(out.clone());
        self.dirty = false;
        out
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.total_len = 0;
        self.cached_replay = None;
        self.dirty = true;
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.total_len
    }
}

/// 单个终端槽位：PTY 句柄 + RingBuffer + 元数据。
///
/// 所有可变字段用独立 Mutex，避免不同操作互相阻塞。
/// `closed` 标志协调 wait 线程：用户主动关闭/重启时置位，wait 线程据此退出且不触发自动重启。
pub struct TerminalSlot {
    pub id: Uuid,
    pub label: Mutex<String>,
    pub project_id: Mutex<Option<String>>,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
    /// 后端内核标识（"claude-code" / "opencode" / "codex" 等）
    pub backend: String,
    pub status: Mutex<SlotStatus>,
    pub exit_code: Mutex<Option<i32>>,
    pub pid: Mutex<Option<u32>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 上次切换到该终端的时间（session 恢复时用于定位上次活跃项目）
    pub last_active_at: Mutex<String>,
    pub spawned_at: Mutex<Option<Instant>>,
    pub restart_count: Mutex<u32>,

    /// 用户主动关闭/重启标记。
    pub closed: AtomicBool,

    /// generation 计数器 — 每次 spawn/restart 递增。
    /// wait 线程记录自己启动时的 generation，仅在 generation 匹配时写入状态/自动重启。
    /// 消除 restart() 中的 sleep 竞态。
    pub generation: AtomicU32,

    ring_buffer: Mutex<RingBuffer>,
    pty_master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// 缓存 PTY writer——take_writer() 只能调用一次，必须在 replace_handles 时预先取出。
    pty_writer: Mutex<Option<Box<dyn Write + Send>>>,
    child_killer: Mutex<Option<Box<dyn ChildKiller + Send>>>,
}

impl TerminalSlot {
#[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        label: String,
        project_id: Option<String>,
        cmd: String,
        args: Vec<String>,
        cwd: String,
        env: HashMap<String, String>,
        backend: String,
        ring_buffer_max: usize,
    ) -> Self {
        Self {
            id,
            label: Mutex::new(label),
            project_id: Mutex::new(project_id),
            cmd,
            args,
            cwd,
            env,
            backend,
            status: Mutex::new(SlotStatus::Running),
            exit_code: Mutex::new(None),
            pid: Mutex::new(None),
            created_at: chrono::Utc::now(),
            last_active_at: Mutex::new(String::new()),
            spawned_at: Mutex::new(None),
            restart_count: Mutex::new(0),
            closed: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            ring_buffer: Mutex::new(RingBuffer::new(ring_buffer_max)),
            pty_master: Mutex::new(None),
            pty_writer: Mutex::new(None),
            child_killer: Mutex::new(None),
        }
    }

    /// 写入用户输入到 PTY（读锁级别即可，不修改结构）。
    /// 若 pty_writer 为 None（中毒恢复后或 slot 未就绪），返回 NotConnected 错误。
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        let mut writer = recover_lock!(self.pty_writer.lock(), "pty_writer");
        match writer.as_mut() {
            Some(w) => {
                w.write_all(data)?;
                w.flush()?;
                Ok(())
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "pty writer not available (slot may have crashed)",
            )),
        }
    }

    /// 调整 PTY 大小。
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let master = recover_lock!(self.pty_master.lock(), "pty_master");
        if let Some(m) = master.as_ref() {
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(())
    }

    /// 获取 PTY 当前尺寸（用于重启时保留终端大小）。
    pub fn get_size(&self) -> Option<PtySize> {
        let master = recover_lock!(self.pty_master.lock(), "pty_master");
        master.as_ref().and_then(|m| m.get_size().ok())
    }

    /// 终止子进程（SIGTERM 由 portable-pty 处理）。
    pub fn kill(&self) -> std::io::Result<()> {
        let mut killer = recover_lock!(self.child_killer.lock(), "child_killer");
        if let Some(k) = killer.as_mut() {
            k.kill().map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(())
    }

    /// 取回放缓冲（Tab 切换时前端调用）。
    pub fn replay(&self) -> Vec<u8> {
        recover_lock!(self.ring_buffer.lock(), "ring_buffer").replay()
    }

    pub fn clear_buffer(&self) {
        recover_lock!(self.ring_buffer.lock(), "ring_buffer").clear();
    }

    /// 写入 PTY 输出到 ring buffer（batcher 任务调用）。
    pub fn push_output(&self, data: Vec<u8>) {
        recover_lock!(self.ring_buffer.lock(), "ring_buffer").push(data);
    }

    pub fn rename(&self, label: String) {
        *recover_lock!(self.label.lock(), "label") = label;
    }

    /// 标记此终端为当前活跃（前端 switchTo 时调用）。
    /// 时间戳写入 last_active_at，供 session 恢复时定位上次使用的终端。
    pub fn touch_active(&self) {
        *recover_lock!(self.last_active_at.lock(), "last_active_at") = chrono::Utc::now().to_rfc3339();
    }

    pub fn label(&self) -> String {
        recover_lock!(self.label.lock(), "label").clone()
    }

    pub fn status(&self) -> SlotStatus {
        *recover_lock!(self.status.lock(), "status")
    }

    pub fn set_status(&self, s: SlotStatus) {
        *recover_lock!(self.status.lock(), "status") = s;
    }

    /// 标记为用户主动关闭/重启，wait 线程据此退出且不自动重启。
    pub fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 递增 generation，返回新值。用于 spawn/restart 时标记"这是新一代"。
    pub fn bump_generation(&self) -> u32 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 读取当前 generation。
    pub fn current_generation(&self) -> u32 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn inc_restart(&self) -> u32 {
        let mut c = recover_lock!(self.restart_count.lock(), "restart_count");
        *c += 1;
        *c
    }

    pub fn reset_restart(&self) {
        *recover_lock!(self.restart_count.lock(), "restart_count") = 0;
    }

    /// 替换 PTY 句柄（自动重启 / 手动 restart 时复用）。
    pub fn replace_handles(
        &self,
        master: Box<dyn MasterPty + Send>,
        killer: Box<dyn ChildKiller + Send>,
        pid: Option<u32>,
    ) {
        // take_writer() 只能调用一次，在此预先取出并缓存
        let writer = master
            .take_writer()
            .map_err(|e| {
                log::error!("[pty] take_writer failed: {e}");
                e
            })
            .ok();
        *recover_lock!(self.pty_master.lock(), "pty_master") = Some(master);
        *recover_lock!(self.pty_writer.lock(), "pty_writer") = writer;
        *recover_lock!(self.child_killer.lock(), "child_killer") = Some(killer);
        *recover_lock!(self.pid.lock(), "pid") = pid;
        *recover_lock!(self.spawned_at.lock(), "spawned_at") = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[test]
    fn ring_push_len() {
        let mut rb = RingBuffer::new(1000);
        rb.push(bytes("hello"));
        rb.push(bytes(" world"));
        assert_eq!(rb.len(), 11);
    }

    #[test]
    fn ring_empty_push_ignored() {
        let mut rb = RingBuffer::new(1000);
        rb.push(Vec::new());
        assert_eq!(rb.len(), 0);
    }

    #[test]
    fn ring_trim_keeps_tail() {
        let mut rb = RingBuffer::new(100);
        rb.push(bytes(&"0123456789".repeat(20))); // 200 > 1.1*100 → 裁到 100
        assert_eq!(rb.len(), 100);
        let full = "0123456789".repeat(20);
        assert_eq!(rb.replay(), &full.as_bytes()[full.len() - 100..]);
    }

    #[test]
    fn ring_trim_multichunk_correct() {
        let mut rb = RingBuffer::new(50);
        rb.push(bytes("AAAA"));
        rb.push(bytes(&"BBBB".repeat(10)));
        rb.push(bytes(&"CCCC".repeat(10))); // 84 > 55 → 裁
        assert_eq!(rb.len(), 50);
        let full = format!("{}{}{}", "AAAA", "BBBB".repeat(10), "CCCC".repeat(10));
        assert_eq!(rb.replay(), &full.as_bytes()[full.len() - 50..]);
    }

    #[test]
    fn ring_replay_cache_idempotent() {
        let mut rb = RingBuffer::new(1000);
        rb.push(bytes("cache"));
        let r1 = rb.replay();
        let r2 = rb.replay();
        assert_eq!(r1, r2);
    }

    #[test]
    fn ring_push_invalidates_cache() {
        let mut rb = RingBuffer::new(1000);
        rb.push(bytes("a"));
        let _ = rb.replay();
        rb.push(bytes("b"));
        assert_eq!(rb.replay(), b"ab".to_vec());
    }

    #[test]
    fn ring_clear_empties() {
        let mut rb = RingBuffer::new(1000);
        rb.push(bytes("xyz"));
        rb.clear();
        assert_eq!(rb.len(), 0);
        assert!(rb.replay().is_empty());
    }

    #[test]
    fn ring_no_trim_under_threshold() {
        let mut rb = RingBuffer::new(100);
        rb.push(bytes(&"x".repeat(50))); // 50 <= 110 不裁
        assert_eq!(rb.len(), 50);
    }
}
