//! Backend profiles — predefined per-kernel defaults for AI coding CLIs.
//!
//! Each backend (claude-code, opencode, codex, crush, aider, goose, hermes)
//! has its own command, default arguments, environment variable mapping, and
//! config directory. This module provides static profile definitions and
//! runtime detection of installed backends.

use serde::Serialize;

// ── Static profile definitions ─────────────────────────────────────

/// Per-backend static defaults.
pub struct BackendProfile {
    pub id: &'static str,
    pub display_name: &'static str,
    pub cmd: &'static str,
    pub default_args: &'static [&'static str],
    /// Maps config field name ("api_key", "base_url", "model") → env var name.
    /// Empty slice means the backend manages these via its own config file.
    pub env_key_map: &'static [(&'static str, &'static str)],
    /// Config directory name relative to $HOME (e.g., ".claude", ".config/goose").
    /// Empty string means no standard directory.
    pub config_dir_name: &'static str,
    /// Whether this backend needs the OAuth token conflict workaround.
    pub needs_auth_fix: bool,
    /// Install hint shown on spawn failure.
    pub install_hint: &'static str,
}

pub const PROFILES: &[BackendProfile] = &[
    BackendProfile {
        id: "claude-code",
        display_name: "Claude Code",
        cmd: "claude",
        default_args: &[
            "--permission-mode",
            "bypassPermissions",
            "--dangerously-skip-permissions",
        ],
        env_key_map: &[
            ("api_key", "ANTHROPIC_API_KEY"),
            ("base_url", "ANTHROPIC_BASE_URL"),
            ("model", "ANTHROPIC_MODEL"),
        ],
        config_dir_name: ".claude",
        needs_auth_fix: true,
        install_hint: "npm install -g @anthropic-ai/claude-code",
    },
    BackendProfile {
        id: "opencode",
        display_name: "OpenCode",
        cmd: "opencode",
        default_args: &[],
        env_key_map: &[],
        config_dir_name: "",
        needs_auth_fix: false,
        install_hint: "Download from https://github.com/opencode-ai/opencode",
    },
    BackendProfile {
        id: "crush",
        display_name: "Crush",
        cmd: "crush",
        default_args: &["--yolo"],
        env_key_map: &[],
        config_dir_name: ".config/crush",
        needs_auth_fix: false,
        install_hint: "Download from https://github.com/charmbracelet/crush",
    },
    BackendProfile {
        id: "codex",
        display_name: "Codex CLI",
        cmd: "codex",
        default_args: &["--approval-mode", "full-auto"],
        env_key_map: &[
            ("api_key", "OPENAI_API_KEY"),
            ("base_url", "OPENAI_BASE_URL"),
            ("model", "CODEX_MODEL"),
        ],
        config_dir_name: ".codex",
        needs_auth_fix: false,
        install_hint: "npm install -g @openai/codex",
    },
    BackendProfile {
        id: "aider",
        display_name: "Aider",
        cmd: "aider",
        default_args: &["--auto-commits"],
        env_key_map: &[],
        config_dir_name: "",
        needs_auth_fix: false,
        install_hint: "pip install aider-chat",
    },
    BackendProfile {
        id: "goose",
        display_name: "Goose",
        cmd: "goose",
        default_args: &[],
        env_key_map: &[],
        config_dir_name: ".config/goose",
        needs_auth_fix: false,
        install_hint: "Download from https://github.com/aaif-goose/goose",
    },
    BackendProfile {
        id: "hermes",
        display_name: "Hermes",
        cmd: "hermes",
        default_args: &[],
        env_key_map: &[],
        config_dir_name: ".hermes",
        needs_auth_fix: false,
        install_hint: "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
    },
];

// ── Lookup ────────────────────────────────────────────────────────

/// Find a profile by id. Returns None if not found.
pub fn resolve(id: &str) -> Option<&'static BackendProfile> {
    PROFILES.iter().find(|p| p.id == id)
}

/// Find a profile by id, falling back to claude-code (PROFILES[0]).
pub fn resolve_or_default(id: &str) -> &'static BackendProfile {
    resolve(id).unwrap_or(&PROFILES[0])
}

// ── Detection ─────────────────────────────────────────────────────

/// Detect which backends are installed by running `which <cmd>` for each profile.
/// Returns (profile, is_installed) pairs.
pub fn detect_installed() -> Vec<(&'static BackendProfile, bool)> {
    PROFILES
        .iter()
        .map(|p| {
            let found = std::process::Command::new("which")
                .arg(p.cmd)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            (p, found)
        })
        .collect()
}

// ── IPC serializable type ─────────────────────────────────────────

/// Serializable backend info sent to the frontend via IPC.
#[derive(Clone, Debug, Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub cmd: String,
    pub default_args: Vec<String>,
    pub env_key_map: Vec<(String, String)>,
    pub config_dir_name: String,
    pub needs_auth_fix: bool,
    pub install_hint: String,
    pub installed: bool,
}

impl From<&'static BackendProfile> for BackendInfo {
    fn from(p: &BackendProfile) -> Self {
        Self {
            id: p.id.to_string(),
            display_name: p.display_name.to_string(),
            cmd: p.cmd.to_string(),
            default_args: p.default_args.iter().map(|s| s.to_string()).collect(),
            env_key_map: p
                .env_key_map
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            config_dir_name: p.config_dir_name.to_string(),
            needs_auth_fix: p.needs_auth_fix,
            install_hint: p.install_hint.to_string(),
            installed: false, // set by detect_installed
        }
    }
}

/// IPC command: list all backends with their installation status.
#[tauri::command]
pub fn list_backends() -> Vec<BackendInfo> {
    let detected = detect_installed();
    detected
        .iter()
        .map(|(p, found)| {
            let mut info = BackendInfo::from(*p);
            info.installed = *found;
            info
        })
        .collect()
}
