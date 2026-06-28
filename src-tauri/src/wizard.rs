//! Setup Wizard — 环境检测 + 首次启动判断 + 配置保存。
//!
//! V1 仅检测 + 提示，不自动安装依赖。

use std::process::Command as StdCommand;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::config::{ConfigManager, ConfigUpdate};

// ── 数据结构 ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct DependencyStatus {
    /// 依赖名称 "claude" | "node" | "git"
    pub name: String,
    /// `which` 是否成功
    pub found: bool,
    /// 解析到的版本号
    pub version: Option<String>,
    /// 最低版本要求
    pub min_version: String,
    /// 版本是否满足要求
    pub version_ok: bool,
    /// 安装提示
    pub install_hint: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckEnvironmentResult {
    pub dependencies: Vec<DependencyStatus>,
    pub all_ok: bool,
}

#[derive(Debug, Deserialize)]
pub struct WizardConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

// ── 依赖定义 ────────────────────────────────────────────────────────

struct DepDef {
    name: &'static str,
    cmd: &'static str,
    version_flag: &'static str,
    min_version: &'static str,
    install_hint: &'static str,
}

const DEPS: &[DepDef] = &[
    DepDef {
        name: "claude",
        cmd: "claude",
        version_flag: "--version",
        min_version: "",
        install_hint: "Install: npm install -g @anthropic-ai/claude-code",
    },
    DepDef {
        name: "node",
        cmd: "node",
        version_flag: "--version",
        min_version: "18.0.0",
        install_hint: "Install: brew install node (or visit nodejs.org)",
    },
    DepDef {
        name: "git",
        cmd: "git",
        version_flag: "--version",
        min_version: "2.0.0",
        install_hint: "Install: brew install git (or Xcode Command Line Tools)",
    },
];

/// 检测超时（每条命令）
const CHECK_TIMEOUT_SECS: u64 = 5;

// ── 命令实现 ────────────────────────────────────────────────────────

/// 检测环境依赖（Claude Code CLI / Node.js / Git）
#[tauri::command]
pub fn check_environment() -> Result<CheckEnvironmentResult, String> {
    let deps: Vec<DependencyStatus> = DEPS
        .iter()
        .map(|d| check_one_dep(d))
        .collect();

    let all_ok = deps.iter().all(|d| d.found && d.version_ok);

    Ok(CheckEnvironmentResult {
        dependencies: deps,
        all_ok,
    })
}

/// 是否首次启动（wizard 未完成）
#[tauri::command]
pub fn is_first_run(cfg_mgr: State<'_, ConfigManager>) -> Result<bool, String> {
    let arc = cfg_mgr.arc();
    let cfg = arc.blocking_read();
        Ok(!cfg.wizard_completed)
}

/// 保存 Wizard 配置（API Key / Base URL / Model + 标记 wizard_completed）
#[tauri::command]
pub async fn save_wizard_config(
    config: WizardConfig,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String> {
    let arc = cfg_mgr.arc();
    let mut cfg = arc.write().await;

    // 更新配置
    let update = ConfigUpdate {
        default_cmd: None,
        default_args: None,
        default_cwd: None,
        max_terminals: None,
        ring_buffer_max_mb: None,
        api_key: Some(config.api_key),
        base_url: Some(config.base_url),
        model: Some(config.model),
        wizard_completed: Some(true),
    };
    update.apply_to(&mut cfg);

    // 持久化到文件
    crate::config::save_to_file(&cfg)
}

// ── 内部辅助 ────────────────────────────────────────────────────────

/// 检测单个依赖
fn check_one_dep(dep: &DepDef) -> DependencyStatus {
    // 1. which 检查
    let found = run_with_timeout("which", &[dep.cmd], CHECK_TIMEOUT_SECS)
        .map(|output| output.status.success())
        .unwrap_or(false);

    if !found {
        return DependencyStatus {
            name: dep.name.to_string(),
            found: false,
            version: None,
            min_version: dep.min_version.to_string(),
            version_ok: false,
            install_hint: dep.install_hint.to_string(),
        };
    }

    // 2. --version 检查
    let version = run_with_timeout(dep.cmd, &[dep.version_flag], CHECK_TIMEOUT_SECS)
        .ok()
        .and_then(|output| {
            if output.status.success() {
                parse_version(&String::from_utf8_lossy(&output.stdout))
            } else {
                None
            }
        });

    let version_ok = if dep.min_version.is_empty() {
        // 无最低版本要求，找到即可
        true
    } else {
        version
            .as_ref()
            .map(|v| version_gte(v, dep.min_version))
            .unwrap_or(false)
    };

    DependencyStatus {
        name: dep.name.to_string(),
        found: true,
        version,
        min_version: dep.min_version.to_string(),
        version_ok,
        install_hint: if version_ok {
            String::new()
        } else {
            dep.install_hint.to_string()
        },
    }
}

/// 运行命令并等待（带超时）
fn run_with_timeout(
    cmd: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, String> {
    let child = StdCommand::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {cmd} failed: {e}"))?;

    // 用 thread + timeout 模拟 wait_timeout（std 没有 wait_timeout）
    let handle = child;
    let result = std::thread::scope(|s| {
        let h = s.spawn(move || handle.wait_with_output());
        // 等待超时或完成
        match h.join() {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(format!("wait {cmd} failed: {e}")),
            Err(_) => Err(format!("{cmd} timed out after {timeout_secs}s")),
        }
    });

    result
}

/// 从命令输出解析版本号（提取首个 semver-like 字符串）
fn parse_version(output: &str) -> Option<String> {
    // 匹配 x.y.z 或 x.y 格式
    let _re = regex_simple();
    for part in output.split_whitespace() {
        if is_semver_like(part) {
            return Some(part.to_string());
        }
    }
    // 回退：尝试从整行提取
    let trimmed = output.trim();
    if let Some(pos) = trimmed.rfind(|c: char| c.is_ascii_digit()) {
        let start = trimmed[..=pos]
            .rfind(|c: char| !c.is_ascii_digit() && c != '.')
            .map(|p| p + 1)
            .unwrap_or(0);
        let candidate = &trimmed[start..=pos];
        if is_semver_like(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// 简单 semver 匹配（不引入 regex crate，手写检查）
fn is_semver_like(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u32>().is_ok())
}

/// 正则占位（V1 用手写解析，不引入 regex crate）
fn regex_simple() {}

/// 版本比较：v >= min_version（简单 major.minor.patch 数值比较）
fn version_gte(v: &str, min: &str) -> bool {
    let v_parts: Vec<u32> = v
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    let m_parts: Vec<u32> = min
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();

    for i in 0..m_parts.len().max(v_parts.len()) {
        let v_n = v_parts.get(i).copied().unwrap_or(0);
        let m_n = m_parts.get(i).copied().unwrap_or(0);
        if v_n > m_n {
            return true;
        }
        if v_n < m_n {
            return false;
        }
    }
    true // equal
}

// ── 单元测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_gte() {
        assert!(version_gte("22.5.1", "18.0.0"));
        assert!(version_gte("18.0.0", "18.0.0"));
        assert!(version_gte("2.40.0", "2.0.0"));
        assert!(!version_gte("16.20.0", "18.0.0"));
        assert!(version_gte("18.1.0", "18.0.0"));
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("v22.5.1"), Some("22.5.1".to_string()));
        assert_eq!(parse_version("node v22.5.1"), Some("22.5.1".to_string()));
        assert_eq!(parse_version("git version 2.40.0"), Some("2.40.0".to_string()));
        assert_eq!(parse_version("claude 1.0.5"), Some("1.0.5".to_string()));
    }

    #[test]
    fn test_is_semver_like() {
        assert!(is_semver_like("22.5.1"));
        assert!(is_semver_like("18.0.0"));
        assert!(is_semver_like("2.0"));
        assert!(!is_semver_like(""));
        assert!(!is_semver_like("abc"));
        assert!(!is_semver_like("v22"));
    }
}
