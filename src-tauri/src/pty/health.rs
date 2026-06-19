//! 僵尸进程检测 + 资源监控（P1/M2 骨架）。
//!
//! 设计来源：desktop-client-architecture.md §5.2、desktop-code-structure.md `pty/health.rs`。
//! M2 完整实现：每 5s 轮询 `/proc/{pid}/status`（Linux）或 `ps`（macOS），
//! 检测僵尸（Z 状态）、RSS 超限（>2GB 告警），自动 SIGKILL 僵尸。

use crate::pty::SlotSummary;

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub id: String,
    pub pid: Option<u32>,
    pub is_zombie: bool,
    pub rss_bytes: Option<u64>,
    pub action: HealthAction,
}

#[derive(Debug, Clone, Copy)]
pub enum HealthAction {
    /// 正常
    None,
    /// 自动 SIGKILL 僵尸
    Kill,
    /// RSS 超限，前端告警
    Warn,
}

/// 检测所有 slot 的健康状态。
/// TODO(M2): 实现 /proc 读取 + RSS 解析 + 僵尸判定。
pub fn check_health(_summaries: &[SlotSummary]) -> Vec<HealthReport> {
    Vec::new()
}
