//! MultiPtyManager — 多终端 PTY 管理核心。
//!
//! 架构修订（见 desktop-architecture-review.md）已落地：
//! - §2 IPC：PTY 输出走 `Channel<Vec<u8>>` 流式二进制，**无 base64**。
//! - §4 并发：`RwLock<HashMap>`，write/resize/replay/rename/list 取读锁（可并发），
//!   spawn/kill/restart 取写锁（低频）；每个 slot 的 PTY 句柄独立 Mutex。
//! - §5 批量推送：reader 线程 → mpsc → 50ms / 128KB 批量合并 + 交互式立即刷新 → Channel.send。
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
use crate::events;
use crate::recover_lock;

/// PTY 初始尺寸（对齐 pty.js DEFAULT_COLS/ROWS）
const DEFAULT_COLS: u16 = 200;
const DEFAULT_ROWS: u16 = 50;
/// 批量合并窗口（~20fps，降低渲染频率减轻 WKWebView 闪烁）
const FLUSH_INTERVAL_MS: u64 = 50;
/// 超过此阈值立即推送（不等定时器，避免大块输出卡住）
const FLUSH_IMMEDIATE_BYTES: usize = 128 * 1024;
/// 用户输入后此窗口内的 PTY 输出立即推送（对齐 Web 网关 FLUSH_INTERACTIVE_MS）。
/// 退格/方向键等交互响应通常只有几十字节，永远达不到 128KB 阈值，
/// 不加此判断则每次都等 50ms 定时器 → 交互拖尾感。
const FLUSH_INTERACTIVE_MS: u64 = 100;
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
        let slot = Arc::new(TerminalSlot::new(
            id, label, project_id, cmd, args, cwd, env, backend, ring_max,
        ));

        let PtyHandles {
            master,
            killer,
            reader,
            child,
            pid,
        } = create_pty(
            &slot.cmd,
            &slot.args,
            &slot.cwd,
            &recover_lock!(slot.env.lock(), "env"),
            cols,
            rows,
        )?;
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
        // 第一个 PTY 创建 → 屏幕不熄灭（和视频播放器效果一致，幂等调用）
        crate::keep_awake::prevent_display_sleep();
        Ok((id, pid))
    }

    /// 关闭终端：从管理表移除，终止进程（mark_closed 让 wait 线程退出且不自动重启）。
    pub async fn kill(&self, id: Uuid) -> Result<()> {
        let (slot, is_empty) = {
            let mut inner = self.inner.write().await;
            inner.channels.remove(&id);
            let slot = inner.slots.remove(&id);
            (slot, inner.slots.is_empty())
        };
        if let Some(slot) = slot {
            slot.mark_closed();
            let _ = slot.kill();
            slot.clear_buffer();
        }
        // 最后一个 PTY 关闭 → 恢复屏幕正常熄灭行为
        if is_empty {
            crate::keep_awake::allow_display_sleep();
        }
        Ok(())
    }

    /// 刷新 slot 的 env（切档后用新 config 覆盖 profile 管理的键，保留自定义键）。
    /// ADR §5.3 坑①：`pty.restart` 复用 slot 旧 env，切档后必须显式刷新 env 再 restart，
    /// 否则切了模型、进程还是旧 env。
    pub async fn refresh_env(&self, id: Uuid, config: &AppConfig) -> Result<()> {
        let slot = self
            .get_slot(id)
            .await
            .ok_or_else(|| anyhow!("slot not found"))?;
        let backend = slot.backend.clone();
        let profile = crate::backend::resolve_or_default(&backend);
        let mut env = recover_lock!(slot.env.lock(), "env").clone();
        for (config_key, env_var) in profile.env_key_map {
            match *config_key {
                "api_key" => {
                    if !config.api_key.is_empty() {
                        env.insert(env_var.to_string(), config.api_key.clone());
                    } else {
                        env.remove(*env_var);
                    }
                }
                "base_url" => {
                    if !config.base_url.is_empty() {
                        env.insert(env_var.to_string(), config.base_url.clone());
                    } else {
                        env.remove(*env_var);
                    }
                }
                "model" => {
                    if !config.model.is_empty() {
                        env.insert(env_var.to_string(), config.model.clone());
                    } else {
                        env.remove(*env_var);
                    }
                }
                _ => {}
            }
        }
        // 供应商附加环境变量（DeepSeek 官方推荐配置等）→ 覆盖默认注入值；
        // AUTH_TOKEN 优先：存在时丢弃 API_KEY 避免 auth 冲突。
        for (k, v) in &config.extra_env {
            env.insert(k.clone(), v.clone());
        }
        if env.contains_key("ANTHROPIC_AUTH_TOKEN") {
            env.remove("ANTHROPIC_API_KEY");
        }
        *recover_lock!(slot.env.lock(), "env") = env;
        Ok(())
    }

    /// 手动重启：复用 cmd/args/cwd/env，清空 buffer，重置重启计数。
    /// 前端传入新的 data_channel（旧 Channel 已失效）+ 终端当前尺寸。
    pub async fn restart(
        &self,
        id: Uuid,
        data_channel: Channel<Vec<u8>>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> Result<()> {
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
        } = create_pty(
            &slot.cmd,
            &slot.args,
            &slot.cwd,
            &recover_lock!(slot.env.lock(), "env"),
            effective_cols,
            effective_rows,
        )?;
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
                env: recover_lock!(s.env.lock(), "env").clone(),
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
        // 所有 PTY 已关闭 → 恢复屏幕正常熄灭行为
        crate::keep_awake::allow_display_sleep();
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
                log::warn!(
                    "[pty] kill_all: lock still busy after {}ms",
                    (attempt + 1) * 10
                );
            }
        }
        log::error!(
            "[pty] kill_all: could not acquire lock after 200ms, some terminals may not be killed"
        );
        Vec::new()
    }
}

/// 快照以 root 为根的整棵进程树（含 root）。
/// 用 /bin/ps 一次拉全表构建 ppid→children 映射再 BFS——macOS 无 /proc，逐目录遍历不可行。
/// ps 失败时返回空（调用方降级为只杀直接子进程）。
fn snapshot_tree(root: u32) -> Vec<u32> {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (
            it.next().and_then(|s| s.parse::<u32>().ok()),
            it.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }
    let mut victims = Vec::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        victims.push(pid);
        if let Some(kids) = children.get(&pid) {
            stack.extend_from_slice(kids);
        }
    }
    victims
}

/// 对整棵进程树执行 SIGTERM → 100ms → SIGKILL 双段清扫（退出/关闭终端专用）。
/// 只 SIGTERM 直接子进程的教训：shell 先死，孙代（claude/node/…）被 launchd
/// 收养成孤儿继续跑——「退出 OneCode 后 claude 还在」的根因（2026-08-22 实证）。
fn sweep_tree(victims: &[u32]) {
    if victims.is_empty() {
        return;
    }
    let self_pid = std::process::id();
    let targets: Vec<u32> = victims
        .iter()
        .copied()
        .filter(|p| *p > 1 && *p != self_pid)
        .collect();
    if targets.is_empty() {
        return;
    }
    // 礼貌 SIGTERM，给 shell/claude 100ms 自行退出，再 SIGKILL 兜底防僵尸/赖活
    let _ = std::process::Command::new("/bin/kill")
        .args(targets.iter().map(u32::to_string))
        .status();
    std::thread::sleep(Duration::from_millis(100));
    let _ = std::process::Command::new("/bin/kill")
        .arg("-9")
        .args(targets.iter().map(u32::to_string))
        .status();
    log::info!("[pty] sweep_tree: SIGTERM→SIGKILL {} processes", targets.len());
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

/// 从隔离配置目录 `{CLAUDE_CONFIG_DIR}/settings.json` 的 `env` 节中移除
/// `ANTHROPIC_AUTH_TOKEN`。
///
/// Claude Code 启动时会读取此文件的 `env` 节并注入到进程环境中。
/// 如果同时存在 `ANTHROPIC_API_KEY`（Wizard/Settings 配置）和 `ANTHROPIC_AUTH_TOKEN`，
/// Anthropic SDK 会报 auth 冲突警告。
/// 幂等操作：仅在 AUTH_TOKEN 存在时才写入文件，已移除则跳过。
/// 全局隔离：只碰 onecode 专属的隔离配置目录（与 create_pty 的 CLAUDE_CONFIG_DIR
/// 一致），绝不读写用户全局 `~/.claude/settings.json`。
fn remove_auth_token_from_settings() {
    let path = crate::providers::cc_config_dir().join("settings.json");
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
    log::info!(
        "[auth-fix] removing ANTHROPIC_AUTH_TOKEN from {}",
        path.display()
    );
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

    // macOS GUI 进程不继承用户 shell 的 locale 设置，
    // 子进程默认 C/POSIX locale → 无法正确处理 UTF-8 CJK 文本（乱码）。
    // 策略：解析用户实际 locale，确保子进程 LANG/LC_CTYPE 为 UTF-8。
    // 不设 LC_ALL（会覆盖用户 LC_MESSAGES/LC_TIME 等自定义偏好）。
    let resolved_locale = resolve_locale();
    cb.env("LANG", &resolved_locale);
    cb.env("LC_CTYPE", &resolved_locale);
    cb.env("COLORTERM", "truecolor");

    for (k, v) in env {
        cb.env(k, v);
    }

    // 全局隔离（董事长 2026-08-22 指令：onecode 不改全局、不读全局 ~/.claude）。
    // 两层防线：
    // 1. CLAUDE_CONFIG_DIR → 状态隔离：onecode 的 claude 把 settings/transcripts/
    //    projects 全部读写到专属目录 ~/.onecode/cc-config，不经手用户全局
    //    ~/.claude（后者仍可能被当作 project 源发现，见下方注释 2 的兜底）。
    // 2. CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1 → 路由权威：声明「宿主托管
    //    provider」，Claude Code 忽略任何 settings 文件（managed/project/user）
    //    里的 ANTHROPIC_BASE_URL / API_KEY / MODEL 类变量，以进程 env 为准。
    //    这保证切档后实际走激活供应商，全局 ~/.claude/settings.json 的 env
    //    （哪怕被 project 发现机制带进来）也无法覆盖。
    // 其它后端（codex 等）忽略这两个变量，无副作用。
    cb.env(
        "CLAUDE_CONFIG_DIR",
        &crate::providers::cc_config_dir().to_string_lossy().to_string(),
    );
    cb.env("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1");
    // 关掉 Claude Code 向 Anthropic 官方发的非必要流量（feature flags/用量上报）。
    // 否则国内访问 api.anthropic.com 报 "Unable to connect to Anthropic services:
    // Status 403"——那不是模型请求失败（模型走第三方供应商是通的），是这些非必要
    // 上报被墙。所有供应商一律关掉（onecode 内嵌 claude 永不连官方端点）。
    cb.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    // 自定义模型名（DeepSeek/Bailian/GLM 等）claude 不认识时会强行套 200k 上下文窗口
    // 并打印警告 "is not a model this version of Claude Code recognizes ... auto-compact
    // will keep this session within 200k tokens"。第三方模型名进不了 claude 的模型表，
    // 统一关掉这个未知模型窗口强执行，让上下文窗口以 API 实际返回为准（2026-08-22
    // 实测：交互终端内警告完全消失，请求不受影响）。
    cb.env("CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT", "1");
    // 登录/首次引导防线（官方文档，见 claude-code-guide 核查）：
    // - DISABLE_LOGIN_COMMAND=1：隐藏 /login 命令（官方为"外部 API key 认证"场景设计）；
    // - IS_DEMO=1：跳过首次 onboarding 引导、隐藏 email/org 显示；
    // - CLAUDE_CODE_SKIP_FAST_MODE_NETWORK_ERRORS=1：跳过 fast-mode 对 api.anthropic.com
    //   的网络探测（国内 403 的另一来源）。
    // 组合效果：onecode 内嵌 claude 永不触发浏览器 OAuth 登录、永不连官方端点。
    cb.env("DISABLE_LOGIN_COMMAND", "1");
    cb.env("IS_DEMO", "1");
    cb.env("CLAUDE_CODE_SKIP_FAST_MODE_NETWORK_ERRORS", "1");

    // 若子进程环境中同时存在 ANTHROPIC_API_KEY 和 ANTHROPIC_AUTH_TOKEN，
    // Claude Code 会报 auth 冲突警告。策略：保留 API_KEY，移除 AUTH_TOKEN。
    //
    // AUTH_TOKEN 来源有三层（按优先级从高到低）：
    // 1. 隔离配置目录 {CLAUDE_CONFIG_DIR}/settings.json "env" 节 — onecode 专属，
    //    Claude Code 启动时读取并注入（最常见；全局 ~/.claude 已被 CLAUDE_CONFIG_DIR
    //    隔离，不经手）
    // 2. macOS Keychain（claude /login 存入的 OAuth token）
    // 3. 进程环境变量（继承自父进程 shell）
    //
    // 必须在全部三层都清除，否则任何一个来源都会重新注入 AUTH_TOKEN。
    if cb.get_env("ANTHROPIC_API_KEY").is_some() {
        // 第3层：移除进程环境变量中的 AUTH_TOKEN
        cb.env_remove("ANTHROPIC_AUTH_TOKEN");
        // 第1层：从隔离配置目录的 settings.json env 节移除 AUTH_TOKEN
        remove_auth_token_from_settings();
        // 第2层（keychain 的陈旧 OAuth token）不再设空串占位——统一由下方 guard
        // 之后的非空 CLAUDE_CODE_OAUTH_TOKEN 注入覆盖（见「状态栏登录态」注释）。
        // 空串会让 claude 判定未登录，状态栏误显 "Not logged in · Run /login"。
    }

    // OneCode Desktop 进程常被从一个已运行的 Claude Code 会话中拉起
    // （make dev / npm run dev），父进程环境会带上 Claude Code 的内部会话
    // 标记（CLAUDE_CODE_CHILD_SESSION 等）。CommandBuilder 默认继承父环境，
    // 这些标记透传进 PTY 里的 claude 子进程后，claude 会判定自己是"子会话"
    // → 关闭 transcript 落盘（"Transcript saving is off — inherited
    // CLAUDE_CODE_CHILD_SESSION marker"）。
    // 策略：清除这些纯运行时 Plumbing 标记（不含 auth/config 类，如
    // CLAUDE_CODE_OAUTH_TOKEN 绝不能动），让 PTY 里的 claude 作为独立顶层
    // 会话运行、正常保存 transcript。
    for marker in [
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_PARENT_SESSION_ID",
        "CLAUDE_CODE_MAIN_PROCESS_ID",
        "CLAUDE_CODE_SSE_PORT",
    ] {
        cb.env_remove(marker);
    }

    // 防 Claude Code 登录陷阱（董事长 2026-08-22）：
    // claude 启动时若既无 API Key 也无 Auth Token，会尝试 first-party OAuth 登录
    // → 连接 api.anthropic.com → 国内直接 403 "Claude Code 在您所在地区不可用"。
    // onecode 内嵌 claude **永不登录 Claude Code、永不连官方端点**：缺 creds 或
    // base_url 指向 Anthropic 官方（默认值）就报错不拉起，终端显示清晰提示，
    // 引导去设置/供应商面板配置第三方 Base URL + API Key。
    // 只对 claude 后端生效（cmd 以 claude 结尾；codex/opencode 用各自变量不受影响）。
    let is_claude = cmd.ends_with("claude");
    if is_claude {
        let key_ok = cb
            .get_env("ANTHROPIC_API_KEY")
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let token_ok = cb
            .get_env("ANTHROPIC_AUTH_TOKEN")
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let oauth_ok = cb
            .get_env("CLAUDE_CODE_OAUTH_TOKEN")
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let base_url = cb
            .get_env("ANTHROPIC_BASE_URL")
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 指向第三方供应商端点（非 Anthropic 官方）才有意义；空/官方端点=登录陷阱
        let third_party = !base_url.is_empty() && !base_url.starts_with("https://api.anthropic.com");
        if !key_ok && !token_ok && !oauth_ok {
            return Err(anyhow!(
                "未配置 API Key：请先在「设置 / 供应商」面板配置可用的 Base URL + API Key。\
                 onecode 内嵌 Claude Code 不支持登录（国内无法访问 api.anthropic.com）"
            ));
        }
        if !third_party && !token_ok && !oauth_ok {
            return Err(anyhow!(
                "Base URL 未指向第三方供应商（当前为 Anthropic 官方端点，国内不可用）。\
                 请在「设置 / 供应商」面板配置第三方 Base URL + API Key"
            ));
        }
    }

    // 状态栏登录态（2026-08-22 实测修复）：claude 的 "Not logged in · Run /login"
    // 反映的是 OAuth 账号态而非 API key 可用性——请求走第三方 API key 完全正常，
    // 但状态栏仍显示 Not logged in，误导用户以为未配置认证。填一个**非空**
    // CLAUDE_CODE_OAUTH_TOKEN（用当前生效的 key/token 值）即可让 claude 判定
    // 已登录：纯显示层，实测与 ANTHROPIC_API_KEY 共存不冲突、不影响路由
    // （请求仍按 SDK 优先级走 x-api-key / Bearer）。
    // 注意：
    // - 必须放在上面的登录陷阱 guard **之后**——guard 用 oauth_ok 判断是否放行
    //   官方端点，提前注入非空值会绕开该防线；
    // - 不能写进隔离 settings.json env——CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1
    //   会忽略 settings 文件里的 provider/auth 变量，只能走进程 env（此处）。
    // 非空值同时覆盖 macOS Keychain 里的陈旧 OAuth token（原「第2层空串」的活）。
    let display_token = cb
        .get_env("ANTHROPIC_API_KEY")
        .or_else(|| cb.get_env("ANTHROPIC_AUTH_TOKEN"))
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !display_token.is_empty() {
        cb.env("CLAUDE_CODE_OAUTH_TOKEN", &display_token);
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
/// reader 线程阻塞读 PTY → mpsc；batcher 50ms/128KB 合并 + 交互式立即刷新 → Channel.send + ring_buffer.push。
fn start_reader_batcher(
    slot: Arc<TerminalSlot>,
    channel: Channel<Vec<u8>>,
    reader: Box<dyn Read + Send>,
    runtime: Handle,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PTY_CHAN_CAPACITY);

    // reader 线程：阻塞读，把原始字节发到 mpsc
    // 【诊断】捕获前 10KB PTY 数据到 ~/.onecode/pty-dump.txt（字符串提取），用于排查终端异常输出
    // 每次新 PTY 创建时覆盖写入，包含前 10KB 的可读文本（去除 ANSI 控制序列）
    let diag_path = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".onecode/pty-dump.txt"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/pty-dump.txt"));
    // 重置诊断文件
    let _ = std::fs::write(&diag_path, b"");
    let diag_limit: usize = 10 * 1024;
    let diag_total = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    // 诊断：累计写入文件（上限 10KB，ANSI 控制序列会被保留便于 hex 分析）
                    let prev = diag_total.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    if prev < diag_limit {
                        let write_len = (n.min(diag_limit - prev)).min(n);
                        let _ = std::fs::OpenOptions::new()
                            .create(true).append(true).open(&diag_path)
                            .and_then(|f| {
                                use std::io::Write;
                                let mut w = f;
                                w.write_all(&buf[..write_len])
                            });
                    }

                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    // batcher：批量合并 + 推送（三重触发：大块立即 / 交互式微批 / 定时器兜底）
    //
    // 交互式微批设计：
    //   PTY 对单次退格的响应经常拆成 2+ chunk（如 4B + 32B）在同毫秒内到达。
    //   如果每个 chunk 单独 flush → 2 次 Channel.send → 2 次 term.write → 2 次 canvas 重绘。
    //   微批合并：收到交互式数据后等 2ms，将可能紧随的后续 chunk 合并为单次 flush。
    //   总延迟：2ms（微批等待）vs 0ms（立即），但渲染次数减半 → 体感更快。
    runtime.spawn(async move {
        let mut batch = Vec::with_capacity(8192);
        let mut interval = tokio::time::interval(Duration::from_millis(FLUSH_INTERVAL_MS));
        interval.tick().await; // 首次立即触发

        // 交互式微批定时器：收到交互式数据后启动，2ms 后 flush
        // 初始设为极大值（永不到期），进入微批模式时重设为 2ms
        let interactive_delay = tokio::time::sleep(Duration::from_secs(86400));
        let mut interactive_pending = false;
        // pin 交互式延迟 future（tokio::select! 需要 pinned future）
        tokio::pin!(interactive_delay);

        loop {
            tokio::select! {
                Some(chunk) = rx.recv() => {
                    let recv_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    batch.extend_from_slice(&chunk);
                    if batch.len() >= FLUSH_IMMEDIATE_BYTES {
                        // 大块输出 — 立即推送（避免积压），取消微批定时器
                        interactive_pending = false;
                        flush(&channel, &slot, &mut batch);
                    } else if !interactive_pending {
                        // 非交互式数据或首包 — 检查是否在交互式窗口内
                        let last_input = slot.last_input_time.load(Ordering::Relaxed);
                        if last_input > 0 && recv_ts.saturating_sub(last_input) < FLUSH_INTERACTIVE_MS {
                            // 交互式数据 — 启动 2ms 微批定时器，等后续 chunk 合并
                            interactive_pending = true;
                            interactive_delay.set(tokio::time::sleep(Duration::from_millis(2)));
                        }
                        // 非交互式 → 不做任何操作，等 50ms 定时器
                    }
                    // interactive_pending = true → 正在微批等待中，新数据已 extend，等定时器到期
                }
                _ = &mut interactive_delay, if interactive_pending => {
                    // 交互式微批到期 — flush 合并后的数据
                    if !batch.is_empty() {
                        let last_input = slot.last_input_time.load(Ordering::Relaxed);
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let ago_ms = now_ms.saturating_sub(last_input);
                        log::debug!("[pty] INTERACTIVE flush: {}B input_ago={}ms", batch.len(), ago_ms);
                        flush(&channel, &slot, &mut batch);
                    }
                    interactive_pending = false;
                }
                _ = interval.tick() => {
                    // 定时器兜底 — 非交互式或微批漏网的场景
                    interactive_pending = false;
                    if !batch.is_empty() {
                        log::debug!("[pty] TIMER flush: {}B", batch.len());
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
            log::debug!(
                "[pty] stale wait thread gen={} current={} id={}, exiting",
                my_gen,
                slot.current_generation(),
                slot.id
            );
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
        match create_pty(
            &slot.cmd,
            &slot.args,
            &slot.cwd,
            &recover_lock!(slot.env.lock(), "env"),
            pty_size.map(|s| s.cols),
            pty_size.map(|s| s.rows),
        ) {
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

/// 解析用户 locale，确保子进程 LANG/LC_CTYPE 为 UTF-8。
///
/// macOS GUI 应用（从 Dock/Finder 启动）不继承用户 shell 的 locale 设置，
/// 子进程默认 C/POSIX locale → 无法正确处理 UTF-8 多字节字符（CJK 乱码）。
///
/// 策略：
/// 1. 检查当前进程 LANG（如已是 UTF-8 则直接用）
/// 2. 查询 login shell 的 $LANG（匹配用户实际 locale 配置）
/// 3. 尝试 macOS `defaults read -g AppleLocale` + 拼接 `.UTF-8`
/// 4. 兜底 `en_US.UTF-8`
///
/// 不设 LC_ALL（会覆盖用户 LC_MESSAGES/LC_TIME 等自定义偏好），
/// 只设 LANG + LC_CTYPE（前者为默认 locale，后者控制字符编码）。
fn resolve_locale() -> String {
    // 1. 检查当前进程环境
    if let Ok(lang) = std::env::var("LANG") {
        if lang.contains("UTF-8") || lang.contains("utf-8") || lang.contains("utf8") {
            return lang;
        }
        // LANG 存在但非 UTF-8 — 强制 UTF-8 变体（如 en_US.ISO8859-1 → en_US.UTF-8）
        if let Some(dot) = lang.find('.') {
            return format!("{}.UTF-8", &lang[..dot]);
        }
        return format!("{}.UTF-8", lang);
    }

    // 2. 查询 login shell（与 resolve_full_path 同模式）
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    for args in &[
        &["-l", "-i", "-c", "echo $LANG"][..],
        &["-l", "-c", "echo $LANG"],
    ] {
        if let Ok(output) = std::process::Command::new(&shell).args(*args).output() {
            if output.status.success() {
                let lang = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !lang.is_empty()
                    && (lang.contains("UTF-8") || lang.contains("utf-8") || lang.contains("utf8"))
                {
                    return lang;
                }
                // Shell 有 LANG 但非 UTF-8 — 升级
                if !lang.is_empty() {
                    if let Some(dot) = lang.find('.') {
                        return format!("{}.UTF-8", &lang[..dot]);
                    }
                    return format!("{}.UTF-8", lang);
                }
            }
        }
    }

    // 3. 尝试 macOS defaults（AppleLocale）
    if let Ok(output) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
    {
        if output.status.success() {
            let locale = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !locale.is_empty() {
                return format!("{}.UTF-8", locale);
            }
        }
    }

    // 4. 兜底
    "en_US.UTF-8".to_string()
}
