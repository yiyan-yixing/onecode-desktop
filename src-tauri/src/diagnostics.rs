//! 启动期降级的**可见化**。
//!
//! 起因很直接：把 app 真的跑起来（第 17 轮）之后，日志里躺着三条降级
//! —— 隔离配置写不进去、会话库打不开、WebKit 缓存目录建不了 ——
//! **而界面上一点提示都没有**。这会话库那条尤其要命：终端列表/会话恢复直接是空的，
//! 用户看到的是"我的终端都不见了"，日志里才写着原因。
//!
//! 这与整个项目的一条规矩是同源的（第 3、4、9、12 刀反复出现）：
//! **降级必须如实显示，不能只躺在日志里**。
//!
//! 这里只收集**启动期**的、影响功能的降级。进程健康（RSS/僵尸）走 `pty::health`，
//! 两条通道职责不同、不混。

use std::sync::Mutex;

/// 一条启动期降级。`impact` 是**用户视角的后果** —— 只说技术原因没用，
/// 得让人知道"我因此少了什么"。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StartupIssue {
    /// 机器可读的分类（`session-store` / `cc-config` / `app-data-dir` …）
    pub kind: String,
    /// 技术原因（原样带出，便于排查）
    pub message: String,
    /// 用户视角的后果（中文，界面直接用）
    pub impact: String,
}

static ISSUES: Mutex<Vec<StartupIssue>> = Mutex::new(Vec::new());

/// 记一条启动期降级。**同时写日志** —— 界面与日志各有用处，不能只留一处。
pub fn record(kind: &str, message: impl Into<String>, impact: impl Into<String>) {
    let issue = StartupIssue {
        kind: kind.to_string(),
        message: message.into(),
        impact: impact.into(),
    };
    log::warn!(
        "[startup] {}：{}（影响：{}）",
        issue.kind,
        issue.message,
        issue.impact
    );
    if let Ok(mut v) = ISSUES.lock() {
        // 同一分类只留第一条：重复的不增加信息量，反而把界面撑长
        if !v.iter().any(|i| i.kind == issue.kind) {
            v.push(issue);
        }
    }
}

/// 当前收集到的降级（前端启动时取一次）。
pub fn snapshot() -> Vec<StartupIssue> {
    ISSUES.lock().map(|v| v.clone()).unwrap_or_default()
}

/// 清空。**只给测试用** —— 进程级的启动降级不该在运行中被清掉。
#[cfg(test)]
pub fn clear() {
    if let Ok(mut v) = ISSUES.lock() {
        v.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试之间共享进程级状态，所以串行跑。
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn records_and_snapshots_a_degradation_with_its_user_impact() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        record("session-store", "attempt to write a readonly database", "终端列表可能不完整");
        let s = snapshot();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, "session-store");
        assert!(s[0].message.contains("readonly"));
        assert!(s[0].impact.contains("终端列表"), "必须带用户视角的后果");
        clear();
    }

    #[test]
    fn the_same_kind_is_recorded_only_once() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        record("cc-config", "第一次失败", "切档可能不生效");
        record("cc-config", "第二次失败", "切档可能不生效");
        record("app-data-dir", "另一个分类", "另一个后果");
        let s = snapshot();
        assert_eq!(s.len(), 2, "同分类只留第一条：{s:?}");
        assert_eq!(s[0].message, "第一次失败");
        clear();
    }

    #[test]
    fn empty_when_nothing_went_wrong() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        assert!(snapshot().is_empty());
    }
}
