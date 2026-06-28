//! MultiPtyManager — 多终端 PTY 管理核心。
//!
//! 架构修订（见 desktop-architecture-review.md）已落地：
//! - §2 IPC：PTY 输出走 `Channel<Vec<u8>>` 流式二进制，**无 base64**。
//! - §4 并发：`RwLock<HashMap>`，write/resize/replay/rename/list 取读锁（可并发），
//!   spawn/kill/restart 取写锁（低频）；每个 slot 的 PTY 句柄独立 Mutex。
//! - §5 批量推送：reader 线程 → mpsc → 16ms / 32KB 批量合并 → Channel.send。
//! - 自动重启：移植自 pty.js:135-164 的指数退避（500ms × 2^n，上限 30s，10 次封顶，
//!   稳定运行 5s 重置）。

pub mod health;
mod slot;

pub use slot::{SlotStatus, TerminalSlot};

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::recover_lock;
use crate::events;

/// PTY 初始尺寸（对齐 pty.js DEFAULT_COLS/ROWS）
const DEFAULT_COLS: u16 = 200;
const DEFAULT_ROWS: u16 = 50;
/// 批量合并窗口（~20fps，降低渲染频率减轻 WKWebView 闪烁）
const FLUSH_INTERVAL_MS: u64 = 50;
/// 超过此阈值立即推送（不等定时器，避免大块输出卡住）
const FLUSH_IMMEDIATE_BYTES: usize = 128 * 1024;
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
    pub project_id: Option<String>,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub cmd: String,
    pub cwd: String,
    pub backend: String,
}

/// 终端配置快照（会话持久化用）。不含运行时状态（pid/status/ring buffer），
/// 仅保留重 spawn 所需的 {id,label,project_id,cmd,args,cwd,env,backend,created_at,last_active_at}。
#[derive(serde::Serialize, Clone)]
pub struct SlotConfig {
    pub id: String,
    pub label: String,
    pub project_id: Option<String>,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub backend: String,
    pub created_at: String,
    pub last_active_at: String,
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
    #[allow(dead_code)]
    pub async fn slot_count(&self) -> usize {
        self.inner.read().await.slots.len()
    }

    async fn get_slot(&self, id: Uuid) -> Option<Arc<TerminalSlot>> {
        self.inner.read().await.slots.get(&id).cloned()
    }

    // ── 结构变更：写锁 ──────────────────────────────────────────────

    /// 创建新终端。返回 slot id。
#[allow(clippy::too_many_arguments)]
    pub async fn spawn(
        &self,
        cmd: String,
        args: Vec<String>,
        cwd: String,
        env: HashMap<String, String>,
        data_channel: Channel<Vec<u8>>,
        label: Option<String>,
        project_id: Option<String>,
        backend: String,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> Result<(Uuid, Option<u32>)> {
        {
            let inner = self.inner.read().await;
            if inner.slots.len() >= self.config.max_terminals {
                return Err(anyhow!("已达最大终端数 ({})", self.config.max_terminals));
            }
        }

        let id = Uuid::new_v4();
        let label = label.unwrap_or_else(|| "Terminal".to_string());
        let ring_max = self.config.ring_buffer_max_mb * 1024 * 1024;
        let slot = Arc::new(TerminalSlot::new(id, label, project_id, cmd, args, cwd, env, backend, ring_max));

        let PtyHandles {
            master,
            killer,
            reader,
            child,
            pid,
        } = create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env, cols, rows)?;
        slot.replace_handles(master, killer, pid);

        start_reader_batcher(
            slot.clone(),
            data_channel.clone(),
            reader,
            self.runtime.clone(),
        );
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
    /// 前端传入新的 data_channel（旧 Channel 已失效）+ 终端当前尺寸。
    pub async fn restart(&self, id: Uuid, data_channel: Channel<Vec<u8>>, cols: Option<u16>, rows: Option<u16>) -> Result<()> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;

        // 从前端获取终端尺寸；若前端未传则从旧 PTY master 读取
        let effective_cols = cols.or_else(|| slot.get_size().map(|s| s.cols));
        let effective_rows = rows.or_else(|| slot.get_size().map(|s| s.rows));

        // 1. 标记关闭 → 旧 wait 线程退出（不自动重启）
        slot.mark_closed();
        // 2. 递增 generation → 旧 wait 线程检测到 generation 不匹配，跳过状态写入
        slot.bump_generation();
        // 3. 终止旧 PTY
        let _ = slot.kill();
        // 4. 重新创建 PTY（使用前端传入的终端尺寸，避免换行/光标错位）
        let PtyHandles {
            master,
            killer,
            reader,
            child,
            pid,
        } = create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env, effective_cols, effective_rows)?;
        slot.replace_handles(master, killer, pid);
        slot.clear_buffer();
        slot.set_status(SlotStatus::Running);
        slot.reset_restart();
        slot.closed.store(false, Ordering::SeqCst); // 新 wait 线程恢复正常自动重启

        // 5. 启动新 reader/batcher/wait
        start_reader_batcher(
            slot.clone(),
            data_channel.clone(),
            reader,
            self.runtime.clone(),
        );
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
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        slot.write(&data)?;
        Ok(())
    }

    pub async fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<()> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        slot.resize(cols, rows)?;
        Ok(())
    }

    pub async fn replay(&self, id: Uuid) -> Result<Vec<u8>> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        Ok(slot.replay())
    }

    pub async fn rename(&self, id: Uuid, label: String) -> Result<()> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        slot.rename(label);
        Ok(())
    }

    pub async fn list(&self) -> Vec<SlotSummary> {
        let inner = self.inner.read().await;
        inner.slots.values().map(summary_of).collect()
    }

    /// 当前所有终端的配置快照（会话持久化用，不含运行时状态）。
    pub async fn snapshot(&self) -> Vec<SlotConfig> {
        let inner = self.inner.read().await;
        inner
            .slots
            .values()
            .map(|s| SlotConfig {
                id: s.id.to_string(),
                label: s.label(),
                project_id: recover_lock!(s.project_id.lock(), "project_id").clone(),
                cmd: s.cmd.clone(),
                args: s.args.clone(),
                cwd: s.cwd.clone(),
                env: s.env.clone(),
                backend: s.backend.clone(),
                created_at: s.created_at.to_rfc3339(),
                last_active_at: recover_lock!(s.last_active_at.lock(), "last_active_at").clone(),
            })
            .collect()
    }

    /// 标记指定终端为当前活跃（前端 switchTo 时调用）。
    pub async fn set_active(&self, id: Uuid) -> Result<()> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        slot.touch_active();
        Ok(())
    }

    /// 同步终止所有终端（托盘「退出」用，避免异步 + State 借用生命周期问题）。
    /// 对 tokio RwLock 用 try_write；若拿不到锁（罕见，spawn/restart 进行中）则
    /// 退化为 try_read 逐个 kill（slot.kill 是同步的，无需改结构）。
    /// P1-15 fix: 若两者都失败，使用受限重试循环（避免 blocking_write 导致
    /// tokio 运行时死锁），确保退出时不遗漏终端。
    pub fn kill_all_blocking(&self) {
        // Phase 1: acquire slot list (with P1-15 retry fallback)
        let slots = self.kill_all_acquire_slots();
        // Phase 2: kill all acquired slots
        for s in &slots {
            s.mark_closed(); // 让 wait 线程不自动重启
            let _ = s.kill();
        }
    }

    /// 尝试获取所有终端 slot 的列表用于 kill_all。
    /// P1-15 fix: 使用受限重试循环（避免 blocking_write 导致 tokio 运行时死锁）。
    fn kill_all_acquire_slots(&self) -> Vec<Arc<TerminalSlot>> {
        // Fast path: try_write
        if let Ok(mut inner) = self.inner.try_write() {
            inner.channels.clear();
            return inner.slots.drain().map(|(_, v)| v).collect();
        }
        // Fallback 1: try_read (slot.kill is sync, no struct mutation needed)
        if let Ok(inner) = self.inner.try_read() {
            return inner.slots.values().cloned().collect();
        }
        // P1-15: Both try_write and try_read failed. Use a bounded retry loop
        // instead of blocking_write (which can deadlock the tokio runtime).
        log::warn!("[pty] kill_all: lock busy, retrying");
        for attempt in 0..20 {
            std::thread::sleep(Duration::from_millis(10));
            if let Ok(mut inner) = self.inner.try_write() {
                inner.channels.clear();
                return inner.slots.drain().map(|(_, v)| v).collect();
            }
            if let Ok(inner) = self.inner.try_read() {
                return inner.slots.values().cloned().collect();
            }
            if attempt % 5 == 4 {
                log::warn!("[pty] kill_all: lock still busy after {}ms", (attempt + 1) * 10);
            }
        }
        log::error!("[pty] kill_all: could not acquire lock after 200ms, some terminals may not be killed");
        Vec::new()
    }
}

fn summary_of(s: &Arc<TerminalSlot>) -> SlotSummary {
    SlotSummary {
        id: s.id.to_string(),
        label: s.label(),
        project_id: recover_lock!(s.project_id.lock(), "project_id").clone(),
        status: s.status().as_str().to_string(),
        pid: *recover_lock!(s.pid.lock(), "pid"),
        exit_code: *recover_lock!(s.exit_code.lock(), "exit_code"),
        cmd: s.cmd.clone(),
        cwd: s.cwd.clone(),
        backend: s.backend.clone(),
    }
}

// ── Claude Code auth 冲突修复 ────────────────────────────────────────

/// 从 `~/.claude/settings.json` 的 `env` 节中移除 `ANTHROPIC_AUTH_TOKEN`。
///
/// Claude Code 启动时会读取此文件的 `env` 节并注入到进程环境中。
/// 如果同时存在 `ANTHROPIC_API_KEY`（Wizard/Settings 配置）和 `ANTHROPIC_AUTH_TOKEN`，
/// Anthropic SDK 会报 auth 冲突警告。
/// 幂等操作：仅在 AUTH_TOKEN 存在时才写入文件，已移除则跳过。
fn remove_auth_token_from_settings() {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => h,
        Err(_) => return,
    };
    let path = std::path::PathBuf::from(&home).join(".claude/settings.json");
    if !path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    let env_obj = match settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        Some(obj) => obj,
        None => return,
    };
    if env_obj.remove("ANTHROPIC_AUTH_TOKEN").is_none() {
        // AUTH_TOKEN 不存在，无需写入
        return;
    }
    log::info!("[auth-fix] removing ANTHROPIC_AUTH_TOKEN from {}", path.display());
    if let Ok(pretty) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(&path, pretty);
    }
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
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<PtyHandles> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.unwrap_or(DEFAULT_ROWS),
            cols: cols.unwrap_or(DEFAULT_COLS),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| anyhow!("openpty failed: {e}"))?;

    let mut cb = CommandBuilder::new(cmd);
    for a in args {
        cb.arg(a);
    }
    if !cwd.is_empty() {
        cb.cwd(cwd);
    }
    cb.env("TERM", "xterm-256color");

    // macOS GUI 进程的 PATH 不含用户 shell 路径（~/.local/bin 等），
    // 导致 "claude" 等安装在用户目录下的命令找不到。
    // 策略：先尝试从登录+交互式 shell 获取完整 PATH，
    // 若失败则拼合常见用户路径。始终确保子进程 PATH 包含 ~/.local/bin。
    let resolved_path = resolve_full_path();
    cb.env("PATH", resolved_path);

    for (k, v) in env {
        cb.env(k, v);
    }

    // 若子进程环境中同时存在 ANTHROPIC_API_KEY 和 ANTHROPIC_AUTH_TOKEN，
    // Claude Code 会报 auth 冲突警告。策略：保留 API_KEY，移除 AUTH_TOKEN。
    //
    // AUTH_TOKEN 来源有三层（按优先级从高到低）：
    // 1. ~/.claude/settings.json "env" 节 — Claude Code 启动时读取并注入（最常见）
    // 2. macOS Keychain（claude /login 存入的 OAuth token）
    // 3. 进程环境变量（继承自父进程 shell）
    //
    // 必须在全部三层都清除，否则任何一个来源都会重新注入 AUTH_TOKEN。
    if cb.get_env("ANTHROPIC_API_KEY").is_some() {
        // 第3层：移除进程环境变量中的 AUTH_TOKEN
        cb.env_remove("ANTHROPIC_AUTH_TOKEN");
        // 第2层：空字符串覆盖 keychain 的 OAuth token
        cb.env("CLAUDE_CODE_OAUTH_TOKEN", "");
        // 第1层：从 settings.json 的 env 节中移除 AUTH_TOKEN
        remove_auth_token_from_settings();
    }

    // portable-pty 0.8：spawn_command 在 **slave** 上（不是 master.spawn(slave, cb)）。
    // spawn 后 slave 随 pair 释放关闭，master reader 才能在子进程退出时收到 EOF。
    let child = pair
        .slave
        .spawn_command(cb)
        .map_err(|e| anyhow!("spawn failed: {e}"))?;
    let master = pair.master;
    let killer = child.clone_killer();
    // portable-pty 0.8: Child::process_id() -> Option<u32>
    let pid = child.process_id();
    let reader = master
        .try_clone_reader()
        .map_err(|e| anyhow!("clone_reader failed: {e}"))?;

    Ok(PtyHandles {
        master,
        killer,
        reader,
        child,
        pid,
    })
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
    let _ = channel.send(data); // 推送到前端 xterm
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
    // 记录此 wait 线程启动时的 generation，后续操作仅在 generation 匹配时执行
    let my_gen = slot.current_generation();

    std::thread::spawn(move || loop {
        // portable-pty 0.8: Child::wait(&mut self) -> IoResult<ExitStatus>，ExitStatus::success()
        let code = match child.wait() {
            Ok(status) => {
                if status.success() {
                    0
                } else {
                    1
                }
            }
            Err(_) => 1,
        };

        // generation 不匹配 → 说明 restart() 已递增 generation，此线程已过期
        if slot.current_generation() != my_gen {
            log::debug!("[pty] stale wait thread gen={} current={} id={}, exiting", my_gen, slot.current_generation(), slot.id);
            return;
        }

        // 用户主动关闭/重启 → 退出，不自动重启
        if slot.is_closed() {
            return;
        }

        slot.set_status(SlotStatus::Exited { code });
        *recover_lock!(slot.exit_code.lock(), "exit_code") = Some(code);
        let id_str = slot.id.to_string();
        let _ = app.emit(&events::pty_exit_event(&id_str), code);

        // 稳定运行 >=5s → 重置重启计数（对齐 pty.js:128-133）
        let stable = recover_lock!(slot.spawned_at.lock(), "spawned_at")
            .map(|t| t.elapsed() >= Duration::from_secs(STABLE_RESET_SECS))
            .unwrap_or(false);
        if stable {
            slot.reset_restart();
        }

        let count = *recover_lock!(slot.restart_count.lock(), "restart_count");
        if count >= MAX_RESTART_COUNT {
            log::error!("[pty] max restart reached id={}", slot.id);
            slot.set_status(SlotStatus::Crashed);
            return;
        }

        let new_count = slot.inc_restart();
        // 指数退避：500ms × 2^(new_count-1)，上限 30s（对齐 pty.js:156）
        let exp: u32 = (new_count - 1).min(20);
        let delay_ms = (500u64)
            .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX / 2))
            .min(30000);
        slot.set_status(SlotStatus::Restarting);
        log::warn!(
            "[pty] auto-restart id={} attempt={} in {}ms",
            slot.id,
            new_count,
            delay_ms
        );
        std::thread::sleep(Duration::from_millis(delay_ms));

        // sleep 期间用户可能主动关闭或重启 → 再次检查
        if slot.is_closed() || slot.current_generation() != my_gen {
            return;
        }

        // 自动重启：从旧 PTY master 读取当前尺寸，避免重启后换行/光标错位
        let pty_size = slot.get_size();
        match create_pty(&slot.cmd, &slot.args, &slot.cwd, &slot.env, pty_size.map(|s| s.cols), pty_size.map(|s| s.rows)) {
            Ok(h) => {
                slot.replace_handles(h.master, h.killer, h.pid);
                slot.clear_buffer();
                slot.set_status(SlotStatus::Running);
                // ★ 通知前端：进程已自动重启，状态从 exited → running
                let _ = app.emit(&events::pty_restart_event(&id_str), h.pid);
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

/// 解析完整 PATH（确保 GUI 启动时子进程也能找到 claude 等用户安装的命令）。
///
/// macOS GUI 应用（从 Dock/Finder 启动）的 PATH 仅含系统目录，
/// 不含 ~/.local/bin 等。单纯用 `$SHELL -l -c` 也不够，因为 login shell
/// 只 source .zprofile，不 source .zshrc——而 ~/.local/bin 通常在 .zshrc 中设置。
///
/// 策略：
/// 1. 尝试 `$SHELL -l -i -c 'echo $PATH'`（login + interactive，source .zshrc）
/// 2. 若失败，尝试 `-l -c`（login only）
/// 3. 若仍失败，拼合 HOME 下常见用户路径 + 系统 PATH
/// 4. 最终兜底：确保 ~/.local/bin 一定在 PATH 中
fn resolve_full_path() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".to_string());
    let parent_path = std::env::var("PATH").unwrap_or_default();

    // 尝试 login + interactive shell（会 source .zshrc，包含 nvm/fnm/bun 等 PATH）
    for args in &[
        &["-l", "-i", "-c", "echo $PATH"][..],
        &["-l", "-c", "echo $PATH"],
    ] {
        if let Ok(output) = std::process::Command::new(&shell).args(*args).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && path.contains('/') {
                    // 确保 ~/.local/bin 在 PATH 中（某些 .zshrc 可能不设置它）
                    return ensure_local_bin(&path, &home);
                }
            }
        }
    }

    // 回退：父进程 PATH + 常见用户路径
    let fallback = format!(
        "{home}/.local/bin:{home}/.bun/bin:{home}/.icode/bin:{home}/.cargo/bin:\
         {home}/bin:/opt/homebrew/bin:/usr/local/bin:{parent_path}",
        home = home,
        parent_path = parent_path
    );
    ensure_local_bin(&fallback, &home)
}

/// 确保 PATH 中包含 $HOME/.local/bin（claude 的安装位置）。
fn ensure_local_bin(path: &str, home: &str) -> String {
    let local_bin = format!("{home}/.local/bin", home = home);
    if path.split(':').any(|p| p == local_bin) {
        path.to_string()
    } else {
        format!("{local_bin}:{path}")
    }
}
