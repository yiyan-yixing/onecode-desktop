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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use portable_pty::{ChildKiller, MasterPty, PtySize};
use uuid::Uuid;

/// 10MB ring buffer per slot（对齐 pty.js MAX_BUFFER_SIZE）
pub const RING_BUFFER_MAX_BYTES: usize = 10 * 1024 * 1024;

/// 终端状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    Running,
    Exited { code: i32 },
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
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub status: Mutex<SlotStatus>,
    pub exit_code: Mutex<Option<i32>>,
    pub pid: Mutex<Option<u32>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub spawned_at: Mutex<Option<Instant>>,
    pub restart_count: Mutex<u32>,

    /// 用户主动关闭/重启标记。
    pub closed: AtomicBool,

    ring_buffer: Mutex<RingBuffer>,
    pty_master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    child_killer: Mutex<Option<Box<dyn ChildKiller + Send>>>,
}

impl TerminalSlot {
    pub fn new(
        id: Uuid,
        label: String,
        cmd: String,
        args: Vec<String>,
        cwd: String,
        env: HashMap<String, String>,
        ring_buffer_max: usize,
    ) -> Self {
        Self {
            id,
            label: Mutex::new(label),
            cmd,
            args,
            cwd,
            env,
            status: Mutex::new(SlotStatus::Running),
            exit_code: Mutex::new(None),
            pid: Mutex::new(None),
            created_at: chrono::Utc::now(),
            spawned_at: Mutex::new(None),
            restart_count: Mutex::new(0),
            closed: AtomicBool::new(false),
            ring_buffer: Mutex::new(RingBuffer::new(ring_buffer_max)),
            pty_master: Mutex::new(None),
            child_killer: Mutex::new(None),
        }
    }

    /// 写入用户输入到 PTY（读锁级别即可，不修改结构）。
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        let master = self.pty_master.lock().expect("pty_master poisoned");
        if let Some(m) = master.as_ref() {
            // portable-pty 0.8：MasterPty 不实现 std::io::Write，须 take_writer() 取 writer。
            let mut w = m
                .take_writer()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            w.write_all(data)?;
            w.flush()?;
        }
        Ok(())
    }

    /// 调整 PTY 大小。
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let master = self.pty_master.lock().expect("pty_master poisoned");
        if let Some(m) = master.as_ref() {
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        }
        Ok(())
    }

    /// 终止子进程（SIGTERM 由 portable-pty 处理）。
    pub fn kill(&self) -> std::io::Result<()> {
        let mut killer = self.child_killer.lock().expect("child_killer poisoned");
        if let Some(k) = killer.as_mut() {
            k.kill()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        }
        Ok(())
    }

    /// 取回放缓冲（Tab 切换时前端调用）。
    pub fn replay(&self) -> Vec<u8> {
        self.ring_buffer.lock().expect("ring_buffer poisoned").replay()
    }

    pub fn clear_buffer(&self) {
        self.ring_buffer.lock().expect("ring_buffer poisoned").clear();
    }

    /// 写入 PTY 输出到 ring buffer（batcher 任务调用）。
    pub fn push_output(&self, data: Vec<u8>) {
        self.ring_buffer.lock().expect("ring_buffer poisoned").push(data);
    }

    pub fn rename(&self, label: String) {
        *self.label.lock().expect("label poisoned") = label;
    }

    pub fn label(&self) -> String {
        self.label.lock().expect("label poisoned").clone()
    }

    pub fn status(&self) -> SlotStatus {
        *self.status.lock().expect("status poisoned")
    }

    pub fn set_status(&self, s: SlotStatus) {
        *self.status.lock().expect("status poisoned") = s;
    }

    /// 标记为用户主动关闭/重启，wait 线程据此退出且不自动重启。
    pub fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn inc_restart(&self) -> u32 {
        let mut c = self.restart_count.lock().expect("restart_count poisoned");
        *c += 1;
        *c
    }

    pub fn reset_restart(&self) {
        *self.restart_count.lock().expect("restart_count poisoned") = 0;
    }

    /// 替换 PTY 句柄（自动重启 / 手动 restart 时复用）。
    pub fn replace_handles(
        &self,
        master: Box<dyn MasterPty + Send>,
        killer: Box<dyn ChildKiller + Send>,
        pid: Option<u32>,
    ) {
        *self.pty_master.lock().expect("pty_master poisoned") = Some(master);
        *self.child_killer.lock().expect("child_killer poisoned") = Some(killer);
        *self.pid.lock().expect("pid poisoned") = pid;
        *self.spawned_at.lock().expect("spawned_at poisoned") = Some(Instant::now());
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
