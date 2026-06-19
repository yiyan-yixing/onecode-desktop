//! MultiPtyManager — 多终端 PTY 管理核心。
//!
//! 架构修订（见 desktop-architecture-review.md）已落地：
//! - §2 IPC：PTY 输出走 `Channel<Vec<u8>>` 流式二进制，**无 base64**。
//! - §4 并发：`RwLock<HashMap>`，write/resize/replay/rename/list 取读锁（可并发），
//!   spawn/kill/restart 取写锁（低频）；每个 slot 的 PTY 句柄独立 Mutex。
//! - §5 批量推送：reader 线程 → mpsc → 16ms / 32KB 批量合并 → Channel.send。
//! - 自动重启：移植自 pty.js:135-164 的指数退避（500ms × 2^n，上限 30s，10 次封顶，
//!   稳定运行 5s 重置）。

mod slot;
mod health;

pub use slot::{TerminalSlot, RingBuffer, SlotStatus, RING_BUFFER_MAX_BYTES};

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use portable_pty::{
    native_pty_system, Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::events;

/// PTY 初始尺寸（对齐 pty.js DEFAULT_COLS/ROWS）
const DEFAULT_COLS: u16 = 200;
const DEFAULT_ROWS: u16 = 50;
/// 批量合并窗口（~60fps，与屏幕刷新对齐）
const FLUSH_INTERVAL_MS: u64 = 16;
/// 超过此阈值立即推送（不等定时器，避免大块输出卡 16ms）
const FLUSH_IMMEDIATE_BYTES: usize = 32 * 1024;
/// mpsc channel 容量
const PTY_CHAN_CAPACITY: usize = 256;
/// 自动重启次数上限（对齐 pty.js）
const MAX_RESTART_COUNT: u32 = 10;
const STABLE_RESET_SECS: u64 = 5;

/// 前端拿到的 slot 摘要（pty_list 返回）
#[derive(serde::Serialize, Clone)]
pub struct SlotSummary {
    pub id: String,
    pub label: String,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub cmd: String,
    pub cwd: String,
}

struct ManagerInner {
    slots: HashMap<Uuid, Arc<TerminalSlot>>,
    /// per-slot 的数据 Channel（自动重启时复用）
    channels: HashMap<Uuid, Channel<Vec<u8>>>,
}

pub struct MultiPtyManager {
    inner: RwLock<ManagerInner>,
    app: AppHandle,
    config: Arc<AppConfig>,
    runtime: Handle,
}

impl MultiPtyManager {
    pub fn new(app: AppHandle, config: Arc<AppConfig>, runtime: Handle) -> Self {
        Self {
            inner: RwLock::new(ManagerInner {
                slots: HashMap::new(),
                channels: HashMap::new(),
            }),
            app,
            config,
            runtime,
        }
    }

    /// 当前终端数（容量检查用）
    pub async fn slot_count(&self) -> usize {
        self.inner.read().await.slots.len()
    }

    async fn get_slot(&self, id: Uuid) -> Option<Arc<TerminalSlot>> {
        self.inner.read().await.slots.get(&id).cloned()
    }

    // ── 结构变更：写锁 ──────────────────────────────────────────────

    /// 创建新终端。返回 slot id。
    pub async fn spawn(
        &self,
        cmd: String,
        args: Vec<String>,
        cwd: String,
        env: HashMap<String, String>,
        data_channel: Channel<Vec<u8>>,
        label: Option<String>,
    ) -> Result<(Uuid, Option<u32>)> {
        {
            let inner = self.inner.read().await;
            if inner.slots.len() >= self.config.max_terminals {
                return Err(anyhow!(
                    "已达最大终端数 ({})",
                    self.config.max_terminals
                ));
            }
        }

        let id = Uuid::new_v4();
        let label = label.unwrap_or_else(|| format!("term-{}", short_suffix(id)));
        let ring_max = self.config.ring_buffer_max_mb * 1024 * 1024;
        let slot = Arc::new(TerminalSlot::new(
            id, label, cmd, args, cwd, env, ring_max,
        ));

        let PtyHandles { master, killer, reader, child, pid } =
            create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env)?;
        slot.replace_handles(master, killer, pid);

        start_reader_batcher(slot.clone(), data_channel.clone(), reader, self.runtime.clone());
        start_wait(
            slot.clone(),
            data_channel.clone(),
            child,
            self.app.clone(),
            self.config.clone(),
            self.runtime.clone(),
        );

        {
            let mut inner = self.inner.write().await;
            inner.slots.insert(id, slot);
            inner.channels.insert(id, data_channel);
        }
        Ok((id, pid))
    }

    /// 关闭终端：从管理表移除，终止进程（mark_closed 让 wait 线程退出且不自动重启）。
    pub async fn kill(&self, id: Uuid) -> Result<()> {
        let slot = {
            let mut inner = self.inner.write().await;
            inner.channels.remove(&id);
            inner.slots.remove(&id)
        };
        if let Some(slot) = slot {
            slot.mark_closed();
            let _ = slot.kill();
            slot.clear_buffer();
        }
        Ok(())
    }

    /// 手动重启：复用 cmd/args/cwd/env，清空 buffer，重置重启计数。
    /// 前端传入新的 data_channel（旧 Channel 已失效）。
    pub async fn restart(&self, id: Uuid, data_channel: Channel<Vec<u8>>) -> Result<()> {
        let slot = self.get_slot(id).await.ok_or_else(|| anyhow!("slot not found"))?;

        // 1. 标记关闭 → 旧 wait 线程退出（不自动重启）
        slot.mark_closed();
        // 2. 终止旧 PTY
        let _ = slot.kill();
        // 3. 给旧 wait 线程退出时间（PTY 关闭后 child.wait() 很快返回）
        //    TODO(P2): 用 generation 计数替代 sleep，消除竞态。
        tokio::time::sleep(Duration::from_millis(80)).await;

        // 4. 重新创建 PTY
        let PtyHandles { master, killer, reader, child, pid } =
            create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env)?;
        slot.replace_handles(master, killer, pid);
        slot.clear_buffer();
        slot.set_status(SlotStatus::Running);
        slot.reset_restart();
        slot.closed.store(false, Ordering::SeqCst); // 新 wait 线程恢复正常自动重启

        // 5. 启动新 reader/batcher/wait
        start_reader_batcher(slot.clone(), data_channel.clone(), reader, self.runtime.clone());
        start_wait(
            slot.clone(),
            data_channel.clone(),
            child,
            self.app.clone(),
            self.config.clone(),
            self.runtime.clone(),
        );

        // 6. 更新 channel
        self.inner.write().await.channels.insert(id, data_channel);
        Ok(())
    }

    // ── 读操作：读锁（可并发） ─────────────────────────────────────

    pub async fn write(&self, id: Uuid, data: Vec<u8>) -> Result<()> {
        let slot = self.get_slot(id).await.ok_or_else(|| anyhow!("slot not found"))?;
        slot.write(&data)?;
        Ok(())
    }

    pub async fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<()> {
        let slot = self.get_slot(id).await.ok_or_else(|| anyhow!("slot not found"))?;
        slot.resize(cols, rows)?;
        Ok(())
    }

    pub async fn replay(&self, id: Uuid) -> Result<Vec<u8>> {
        let slot = self.get_slot(id).await.ok_or_else(|| anyhow!("slot not found"))?;
        Ok(slot.replay())
    }

    pub async fn rename(&self, id: Uuid, label: String) -> Result<()> {
        let slot = self.get_slot(id).await.ok_or_else(|| anyhow!("slot not found"))?;
        slot.rename(label);
        Ok(())
    }

    pub async fn list(&self) -> Vec<SlotSummary> {
        let inner = self.inner.read().await;
        inner.slots.values().map(summary_of).collect()
    }
}

fn summary_of(s: &TerminalSlot) -> SlotSummary {
    SlotSummary {
        id: s.id.to_string(),
        label: s.label(),
        status: s.status().as_str().to_string(),
        pid: *s.pid.lock().expect("pid poisoned"),
        exit_code: *s.exit_code.lock().expect("exit_code poisoned"),
        cmd: s.cmd.clone(),
        cwd: s.cwd.clone(),
    }
}

/// 取 UUID 后 4 位 hex 作为默认标签后缀。
fn short_suffix(id: Uuid) -> String {
    let h = id.simple().to_string();
    let len = h.len();
    h[len.saturating_sub(4)..].to_string()
}

// ── PTY 创建 / 后台任务 ────────────────────────────────────────────

struct PtyHandles {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send>,
    reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
}

/// 创建 PTY pair + spawn 子进程。portable-pty 0.8 API。
fn create_pty(
    cmd: &str,
    args: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
) -> Result<PtyHandles> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(
            PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            },
            None,
        )
        .map_err(|e| anyhow!("openpty failed: {e}"))?;

    let mut cb = CommandBuilder::new(cmd);
    for a in args {
        cb.arg(a);
    }
    if !cwd.is_empty() {
        cb.cwd(cwd);
    }
    cb.env("TERM", "xterm-256color");
    for (k, v) in env {
        cb.env(k, v);
    }

    let mut master = pair.master;
    // spawn 消费 slave（关闭 slave 端，PTY 才能正确发送 EOF）。
    let child = master.spawn(pair.slave, cb).map_err(|e| anyhow!("spawn failed: {e}"))?;
    let killer = child.clone_killer();
    // portable-pty 0.8: process_id() -> Option<u32>
    let pid = child.process_id();
    let reader = master
        .try_clone_reader()
        .map_err(|e| anyhow!("clone_reader failed: {e}"))?;

    Ok(PtyHandles { master, killer, reader, child, pid })
}

/// 启动 reader 线程 + batcher tokio 任务。
/// reader 线程阻塞读 PTY → mpsc；batcher 16ms/32KB 合并 → Channel.send + ring_buffer.push。
fn start_reader_batcher(
    slot: Arc<TerminalSlot>,
    channel: Channel<Vec<u8>>,
    reader: Box<dyn Read + Send>,
    runtime: Handle,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PTY_CHAN_CAPACITY);

    // reader 线程：阻塞读，把原始字节发到 mpsc
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break; // channel 关闭（batcher 退出）
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    // batcher：批量合并 + 推送
    runtime.spawn(async move {
        let mut batch = Vec::with_capacity(8192);
        let mut interval = tokio::time::interval(Duration::from_millis(FLUSH_INTERVAL_MS));
        interval.tick().await; // 首次立即触发
        loop {
            tokio::select! {
                Some(chunk) = rx.recv() => {
                    batch.extend_from_slice(&chunk);
                    if batch.len() >= FLUSH_IMMEDIATE_BYTES {
                        flush(&channel, &slot, &mut batch);
                    }
                }
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        flush(&channel, &slot, &mut batch);
                    }
                }
                else => break, // reader 线程结束 → rx 关闭
            }
        }
    });
}

fn flush(channel: &Channel<Vec<u8>>, slot: &TerminalSlot, batch: &mut Vec<u8>) {
    if batch.is_empty() {
        return;
    }
    let data = std::mem::take(batch);
    slot.push_output(data.clone()); // 写 ring buffer（供 Tab 切换 replay）
    let _ = channel.send(data);     // 推送到前端 xterm
}

/// 启动退出监听线程：阻塞 child.wait()，崩溃则指数退避自动重启。
fn start_wait(
    slot: Arc<TerminalSlot>,
    channel: Channel<Vec<u8>>,
    mut child: Box<dyn Child + Send + Sync>,
    app: AppHandle,
    _config: Arc<AppConfig>,
    runtime: Handle,
) {
    std::thread::spawn(move || loop {
        // portable-pty 0.8: Child::wait(&mut self) -> IoResult<ExitStatus>，ExitStatus::success()
        let code = match child.wait() {
            Ok(status) => {
                if status.success() { 0 } else { 1 }
            }
            Err(_) => 1,
        };

        // 用户主动关闭/重启 → 退出，不自动重启
        if slot.is_closed() {
            return;
        }

        slot.set_status(SlotStatus::Exited { code });
        *slot.exit_code.lock().expect("exit_code poisoned") = Some(code);
        let id_str = slot.id.to_string();
        let _ = app.emit(&events::pty_exit_event(&id_str), code);

        // 稳定运行 >=5s → 重置重启计数（对齐 pty.js:128-133）
        let stable = slot
            .spawned_at
            .lock()
            .expect("spawned_at poisoned")
            .map(|t| t.elapsed() >= Duration::from_secs(STABLE_RESET_SECS))
            .unwrap_or(false);
        if stable {
            slot.reset_restart();
        }

        let count = *slot.restart_count.lock().expect("restart_count poisoned");
        if count >= MAX_RESTART_COUNT {
            log::error!("[pty] max restart reached id={}", slot.id);
            slot.set_status(SlotStatus::Crashed);
            return;
        }

        let new_count = slot.inc_restart();
        // 指数退避：500ms × 2^(new_count-1)，上限 30s（对齐 pty.js:156）
        let exp = (new_count - 1).min(20) as u32;
        let delay_ms = (500u64)
            .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX / 2))
            .min(30000);
        slot.set_status(SlotStatus::Restarting);
        log::warn!(
            "[pty] auto-restart id={} attempt={} in {}ms",
            slot.id, new_count, delay_ms
        );
        std::thread::sleep(Duration::from_millis(delay_ms));

        // sleep 期间用户可能主动关闭 → 再次检查
        if slot.is_closed() {
            return;
        }

        match create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env) {
            Ok(h) => {
                slot.replace_handles(h.master, h.killer, h.pid);
                slot.clear_buffer();
                slot.set_status(SlotStatus::Running);
                start_reader_batcher(slot.clone(), channel.clone(), h.reader, runtime.clone());
                child = h.child; // 继续等待新进程
            }
            Err(e) => {
                log::error!("[pty] auto-restart spawn failed id={}: {e}", slot.id);
                slot.set_status(SlotStatus::Crashed);
                return;
            }
        }
    });
}
