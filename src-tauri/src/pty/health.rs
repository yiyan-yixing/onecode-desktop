//! 僵尸进程检测 + RSS 资源监控（P1 完整实现）。
//!
//! 设计来源：desktop-client-architecture.md §5.2、desktop-code-structure.md `pty/health.rs`。
//!
//! 实现：
//! - 后台 tokio 任务每 5s 轮询所有 slot 的 pid：
//!   * Linux：读 `/proc/{pid}/status` 取 VmRSS + State（Z=僵尸）。
//!   * macOS：`ps -o rss=,state= -p {pid}`。
//! - 检测结果 emit `health:report`（前端在状态栏告警）。
//! - RSS > 2GB → `Warn`；僵尸状态 → `Kill`（仅报告，真正回收由 wait 线程 child.wait 完成，
//!   对已 defunct 的僵尸发 SIGKILL 无意义，故不自动 kill）。
//!
//! 注：本模块刻意只用 std（不引入 sysinfo），降低依赖与编译体积。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::pty::{MultiPtyManager, SlotSummary};

/// RSS 告警阈值：2 GiB（对齐 PRD 资源占用告警）。
const RSS_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 轮询间隔。
const POLL_INTERVAL_SECS: u64 = 5;

#[derive(serde::Serialize, Debug, Clone)]
pub struct HealthReport {
    pub id: String,
    pub pid: Option<u32>,
    pub status: String,
    pub alive: bool,
    pub is_zombie: bool,
    pub rss_bytes: Option<u64>,
    pub action: HealthAction,
}

#[derive(serde::Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum HealthAction {
    /// 正常
    None,
    /// 僵尸（报告，回收依赖 wait 线程）
    Kill,
    /// RSS 超限，前端告警
    Warn,
    /// 进程已不在，但 slot 仍认为运行中（陈旧）
    Stale,
}

/// 检测所有 slot 的健康状态。
pub fn check_health(summaries: &[SlotSummary]) -> Vec<HealthReport> {
    summaries.iter().map(report_of).collect()
}

fn report_of(s: &SlotSummary) -> HealthReport {
    let Some(pid) = s.pid else {
        return HealthReport {
            id: s.id.clone(),
            pid: None,
            status: s.status.clone(),
            alive: false,
            is_zombie: false,
            rss_bytes: None,
            action: HealthAction::None,
        };
    };
    let (alive, rss_bytes, is_zombie) = probe(pid);

    let action = if is_zombie {
        HealthAction::Kill
    } else if !alive && s.status == "running" {
        HealthAction::Stale
    } else if let Some(rss) = rss_bytes {
        if rss >= RSS_WARN_BYTES {
            HealthAction::Warn
        } else {
            HealthAction::None
        }
    } else {
        HealthAction::None
    };

    HealthReport {
        id: s.id,
        pid: Some(pid),
        status: s.status,
        alive,
        is_zombie,
        rss_bytes,
        action,
    }
}

/// 启动健康检测后台循环：每 POLL_INTERVAL_SECS 秒轮询并 emit `health:report`。
pub fn start_loop(app: AppHandle) {
    // 用 tauri::async_runtime::spawn（不依赖「当前线程 tokio task 上下文」，
    // Tauri 自管理运行时，setup 阶段直接调用安全）。
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
        tick.tick().await; // 跳过立即触发（应用刚启动，终端未必就绪）
        loop {
            tick.tick().await;
            let Some(mgr) = app.try_state::<MultiPtyManager>() else {
                continue;
            };
            let summaries = mgr.list().await;
            let reports = check_health(&summaries);
            // 只在有需要关注的项时才推送，避免每 5s 刷屏
            let notable: Vec<&HealthReport> = reports
                .iter()
                .filter(|r| !matches!(r.action, HealthAction::None))
                .collect();
            if !notable.is_empty() {
                let payload: Vec<HealthReport> = notable.into_iter().cloned().collect();
                let _ = app.emit("health:report", payload);
            }
        }
    });
}

// ── 平台探测 ────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn probe(pid: u32) -> (bool, Option<u64>, bool) {
    let dir = format!("/proc/{pid}");
    if !std::path::Path::new(&dir).exists() {
        return (false, None, false);
    }
    let status = std::fs::read_to_string(format!("{dir}/status")).unwrap_or_default();
    let mut rss_bytes: Option<u64> = None;
    let mut is_zombie = false;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest
                .split_whitespace()
                .next()
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(0);
            rss_bytes = Some(kb * 1024);
        } else if let Some(rest) = line.strip_prefix("State:") {
            is_zombie = rest.trim_start().starts_with('Z');
        }
    }
    (true, rss_bytes, is_zombie)
}

#[cfg(target_os = "macos")]
fn probe(pid: u32) -> (bool, Option<u64>, bool) {
    let out = match std::process::Command::new("ps")
        .args(["-o", "rss=,state=", "-p", &pid.to_string()])
        .output()
    {
        Ok(o) => o,
        Err(_) => return (false, None, false),
    };
    if !out.status.success() || out.stdout.is_empty() {
        return (false, None, false);
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let mut it = line.split_whitespace();
    let rss_kb: Option<u64> = it.next().and_then(|t| t.parse::<u64>().ok());
    let state = it.next().unwrap_or("");
    (true, rss_kb.map(|kb| kb * 1024), state.starts_with('Z'))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn probe(_pid: u32) -> (bool, Option<u64>, bool) {
    (false, None, false)
}
