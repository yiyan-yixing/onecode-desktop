//! Backend profiles — predefined per-kernel defaults for AI coding CLIs.
//!
//! Each backend (claude-code, opencode, codex, crush, aider, goose, hermes)
//! has its own command, default arguments, environment variable mapping, and
//! config directory. This module provides static profile definitions and
//! runtime detection of installed backends.

use std::path::{Path, PathBuf};

use serde::Serialize;

// ── Static profile definitions ─────────────────────────────────────

/// 一个引擎（后端）的默认定义。
///
/// 字段是 **owned**（不是 `&'static str`）—— 因为引擎定义现在可以来自配置
/// （`~/.onecode/engines.json`），加一个新引擎不该需要改 Rust 重编译。
/// 内置的 7 个仍然是默认值：配置文件只是"覆盖/追加"，不存在就完全等同从前。
#[derive(Clone, Debug)]
pub struct BackendProfile {
    pub id: String,
    pub display_name: String,
    pub cmd: String,
    pub default_args: Vec<String>,
    /// 引擎**说话的方式**：
    /// - `print`（默认）：`-p <prompt> --output-format stream-json`，一问一答；
    /// - `acp`：Agent Client Protocol（JSON-RPC over stdio，双向，可中途要权限）。
    ///
    /// 为什么要有这个字段：ACP 引擎**不吃** `-p`，也不该被塞进"打印模式"的参数。
    /// 调度台需要按协议分流，而不是假设所有引擎都长得像 `claude -p`。

    pub protocol: String,
    /// Maps config field name ("api_key", "base_url", "model") → env var name.
    /// Empty means the backend manages these via its own config file.
    pub env_key_map: Vec<(String, String)>,
    /// Config directory name relative to $HOME (e.g., ".claude", ".config/goose").
    /// Empty string means no standard directory.
    pub config_dir_name: String,
    /// Whether this backend needs the OAuth token conflict workaround.
    pub needs_auth_fix: bool,
    /// Flag that passes an extra system prompt **from a file**, if the backend
    /// supports one. `None` means agent identity cannot be injected here — the
    /// caller must record/show the degradation instead of pretending it worked.
    ///
    /// claude-code: 已在本机随发布的官方 bundle 帮助文本中确认
    /// `--append-system-prompt-file <file>`（与 `--append-system-prompt` 互斥，
    /// "Please use only one."）。用文件而非 argv：长正文不受 argv 长度限制，
    /// 且正文不会出现在 `ps` 里。
    pub system_prompt_file_flag: Option<String>,
    /// 明确表达"权限姿态"的参数（claude-code: `--permission-mode bypassPermissions
    /// --dangerously-skip-permissions`）。应用员工姿态时**先整段移除再按姿态追加**。
    ///
    /// 为什么要把这份参数写在 profile 里而不是散在 spawn：权限参数是"同一判断的
    /// 第二处实现"的高发区（CLAUDE.md 规则 7）—— 只改 append 不清除，就会出现
    /// 两个 `--permission-mode`，claude 取哪个取决于解析顺序，静默失效。
    pub permission_args: Vec<String>,
    /// 设置权限档的 flag。`None` = 该后端不支持按档控制（请求了要如实报告未生效）。
    /// claude-code: `--permission-mode`，取值 `default|acceptEdits|planMode|bypassPermissions`
    /// （已在本机官方 bundle 中确认）。
    pub permission_mode_flag: Option<String>,
    /// 这个后端**接受**的权限档取值白名单（空 = 不校验）。
    ///
    /// 为什么要白名单：`--permission-mode` 的合法取值是**引擎版本相关**的，而且
    /// 传错**不是警告而是直接退出**（实测 2.1.63：`planMode` 非法 → CLI 报
    /// "argument 'planMode' is invalid" 然后终端根本起不来）。
    /// 有了白名单，我们就能在**参数生成阶段**发现并如实报告，而不是让用户面对
    /// 一个秒退的终端。
    pub permission_mode_values: Vec<String>,
    /// 工具白名单 flag。`None` = 不支持。
    /// claude-code: `--allowedTools` / `--allowed-tools`，逗号或空格分隔的工具名列表
    /// （官方帮助原文：'Comma or space-separated list of tool names to allow
    /// (e.g. "Bash(git:*) Edit")'）—— 已在本机 bundle 中确认。
    pub allowed_tools_flag: Option<String>,
    /// Install hint shown on spawn failure.
    pub install_hint: String,
    /// 这个定义从哪来：`builtin` | `user`
    pub source: BackendSource,
}

/// 引擎定义的来源。界面上要能区分"内置的"与"你自己加的"。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendSource {
    Builtin,
    /// `~/.onecode/engines.json`（全局，你的）
    User,
    /// `<项目>/.onecode/engines.json`（跟着仓库走）
    Project,
}

/// 小工具：把 `&[&str]` 变成 owned。内置档的写法因此保持读得出来。
fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// 员工级权限姿态。
///
/// 只表达"引擎允许我们控制的那一维"：Claude Code 的 `--permission-mode`。
/// 不发明第二套权限语言 —— 强制最终仍要落在引擎的沙箱/审批上（见产品规格 §6 铁律 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// 不干预，沿用后端默认（**默认值：保持现状，不改变既有用户行为**）
    Inherit,
    /// 全程不问（现状默认）
    BypassPermissions,
    /// 自动接受文件编辑，其它动作仍要批准 —— 最接近"窄而实在的门"
    AcceptEdits,
    /// 每条都问
    Default,
    /// 只规划不动手。**CLI 取值是 `plan`，不是 `planMode`**
    /// （2026-10-14 实测：`--permission-mode planMode` 会被 CLI 直接拒绝并退出，
    /// 合法值只有 acceptEdits / bypassPermissions / default / dontAsk / plan）。
    Plan,
    /// 不主动问（CLI 取值 `dontAsk`）。这是实测才知道存在的档位。
    DontAsk,
}

impl Posture {
    /// 解析 `permission:` frontmatter。未识别返回 `None`（调用方要如实告警，
    /// 不能静默当成 Inherit —— 那会让用户以为门开了其实没开）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "").as_str() {
            "" => Some(Posture::Inherit),
            "bypasspermissions" | "bypass" | "full" | "yolo" => Some(Posture::BypassPermissions),
            "acceptedits" | "accept-edits" | "edits" => Some(Posture::AcceptEdits),
            "default" | "ask" => Some(Posture::Default),
            // `planmode` 是**错误但有历史**的写法（本产品第 3 刀就是这么写的）：
            // 仍然解析成 Plan，但真值统一用 CLI 接受的 `plan`。见 `mode()`。
            "planmode" | "plan" | "readonly" | "read-only" => Some(Posture::Plan),
            "dontask" | "don'task" | "noask" => Some(Posture::DontAsk),
            _ => None,
        }
    }

    /// 对应的 CLI 取值。`Inherit` 返回 `None`（不追加任何参数）。
    pub fn mode(self) -> Option<&'static str> {
        match self {
            Posture::Inherit => None,
            Posture::BypassPermissions => Some("bypassPermissions"),
            Posture::AcceptEdits => Some("acceptEdits"),
            Posture::Default => Some("default"),
            // ★ 实测取值是 `plan`。写 `planMode` 会让 CLI
            // "option '--permission-mode <mode>' argument 'planMode' is invalid" 直接退出。
            Posture::Plan => Some("plan"),
            Posture::DontAsk => Some("dontAsk"),
        }
    }

    pub fn is_inherit(self) -> bool {
        matches!(self, Posture::Inherit)
    }
}

/// 应用姿态的结果 —— **必须能被如实报告**，"请求了但没生效"不能吞掉。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostureOutcome {
    /// 权限档是否真的写进了参数
    pub mode_applied: Option<&'static str>,
    /// 工具白名单是否真的写进了参数
    pub tools_applied: Vec<String>,
    /// 权限档是**推断**出来的（员工只声明了工具白名单、没声明权限档）。
    /// 界面要显示这一点 —— 不能让人以为是自己设的。
    pub mode_inferred: bool,
    /// 没生效的原因（后端不支持 / 未识别），供日志与界面显示
    pub unsupported_reason: Option<String>,
}

/// 移除 `args` 中**连续出现**的 `slice`（整段匹配，不做逐 token 删除）。
///
/// 必须整段匹配：`["--permission-mode","bypassPermissions"]` 是"flag + 取值"，
/// 逐 token 删会把 flag 删掉、留下孤立的取值，重复应用就会累积垃圾
/// （幂等性测试抓到过这个）。
fn remove_contiguous(args: &mut Vec<String>, slice: &[String]) {
    if slice.is_empty() || args.len() < slice.len() {
        return;
    }
    let mut i = 0;
    while i + slice.len() <= args.len() {
        if (0..slice.len()).all(|k| args[i + k] == slice[k]) {
            args.drain(i..i + slice.len());
            continue; // 同一位置可能还有下一段
        }
        i += 1;
    }
}

/// 移除 `flag` 及其取值（兼容 `flag value` 与 `flag=value`）。
fn strip_flag_with_value(args: &mut Vec<String>, flag: &str) {
    let eq = format!("{flag}=");
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == flag {
            if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if a.starts_with(&eq) {
            i += 1;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    *args = out;
}

/// 把员工声明的权限姿态落到参数上。
///
/// 语义（三条都有测试钉住）：
/// 1. **什么都没声明 → 一个参数都不动**（保持后端默认，绝不改变既有用户行为）；
/// 2. 只声明了工具白名单 → 白名单在 `bypassPermissions` 下**形同虚设**，
///    因此把档位推断为 `acceptEdits` 并标记 `mode_inferred`（如实显示，不静默）；
/// 3. **幂等**：恢复会话时持久化的 `args` 已经带过姿态、会被再应用一次，
///    所以"先清后写"必须把 flag 连取值一起清掉。
pub fn apply_posture_args(
    profile: &BackendProfile,
    posture: Posture,
    allowed_tools: &[String],
    args: &mut Vec<String>,
) -> PostureOutcome {
    let mut out = PostureOutcome::default();

    // 1) 什么都没声明 → 不干预
    if posture.is_inherit() && allowed_tools.is_empty() {
        return out;
    }

    // 2) 只有白名单、没有档位 → 收窄到 acceptEdits，并标明这是推断
    let posture = if posture.is_inherit() {
        out.mode_inferred = true;
        Posture::AcceptEdits
    } else {
        posture
    };

    // 3) 先清：后端默认权限参数（整段） + 上次应用过的档位/白名单（flag + 取值）
    remove_contiguous(args, &profile.permission_args);
    if let Some(flag) = &profile.permission_mode_flag {
        strip_flag_with_value(args, flag);
    }
    if let Some(flag) = &profile.allowed_tools_flag {
        strip_flag_with_value(args, flag);
    }

    // 4) 再写
    if let Some(mode) = posture.mode() {
        // ★ 先校验取值：传错**不是警告而是直接退出**（实测 2.1.63：`--permission-mode
        // planMode` → CLI 报 "argument 'planMode' is invalid" 然后终端起不来）。
        // 在参数生成阶段拦住并如实报告，用户才不会面对一个秒退的终端。
        let allowed = &profile.permission_mode_values;
        if !allowed.is_empty() && !allowed.iter().any(|v| v == mode) {
            out.unsupported_reason = Some(format!(
                "后端 '{}' 不接受权限档 {mode:?}（可用：{}）",
                profile.id,
                allowed.join(" / ")
            ));
            return out;
        }
        match &profile.permission_mode_flag {
            Some(flag) => {
                args.push(flag.clone());
                args.push(mode.to_string());
                out.mode_applied = Some(mode);
            }
            None => {
                out.unsupported_reason = Some(format!(
                    "backend '{}' declares no permission-mode flag",
                    profile.id
                ));
            }
        }
    }

    if !allowed_tools.is_empty() {
        match &profile.allowed_tools_flag {
            Some(flag) => {
                args.push(flag.clone());
                args.push(allowed_tools.join(","));
                out.tools_applied = allowed_tools.to_vec();
            }
            None => {
                let r = format!("backend '{}' declares no allowed-tools flag", profile.id);
                out.unsupported_reason = Some(match out.unsupported_reason.take() {
                    Some(prev) => format!("{prev}; {r}"),
                    None => r,
                });
            }
        }
    }

    out
}

/// 默认协议：`print`（一问一答的结构化模式）。
/// `acp` 走 Agent Client Protocol —— 见 `crate::acp`。
fn default_protocol() -> String {
    "print".to_string()
}

pub fn builtin_profiles() -> Vec<BackendProfile> {
    vec![
    BackendProfile {
        id: "claude-code".into(),
        display_name: "Claude Code".into(),
        cmd: "claude".into(),
        default_args: strs(&[
            "--permission-mode",
            "bypassPermissions",
            "--dangerously-skip-permissions",
        ]),
        protocol: default_protocol(),
        env_key_map: pairs(&[
            ("api_key", "ANTHROPIC_API_KEY"),
            ("base_url", "ANTHROPIC_BASE_URL"),
            ("model", "ANTHROPIC_MODEL"),
        ]),
        config_dir_name: ".claude".into(),
        needs_auth_fix: true,
        system_prompt_file_flag: Some("--append-system-prompt-file".into()),
        permission_args: strs(&[
            "--permission-mode",
            "bypassPermissions",
            "--dangerously-skip-permissions",
        ]),
        permission_mode_flag: Some("--permission-mode".into()),
        // 2026-10-14 实测（cli.js 自己的报错文本）：
        // "Allowed choices are acceptEdits, bypassPermissions, default, dontAsk, plan"
        permission_mode_values: strs(&[
            "acceptEdits",
            "bypassPermissions",
            "default",
            "dontAsk",
            "plan",
        ]),
        allowed_tools_flag: Some("--allowedTools".into()),
        install_hint: "npm install -g @anthropic-ai/claude-code".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "opencode".into(),
        display_name: "OpenCode".into(),
        cmd: "opencode".into(),
        default_args: strs(&[]),
        protocol: default_protocol(),
        env_key_map: pairs(&[]),
        config_dir_name: "".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&[]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "Download from https://github.com/opencode-ai/opencode".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "crush".into(),
        display_name: "Crush".into(),
        cmd: "crush".into(),
        default_args: strs(&["--yolo"]),
        protocol: default_protocol(),
        env_key_map: pairs(&[]),
        config_dir_name: ".config/crush".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&["--yolo"]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "Download from https://github.com/charmbracelet/crush".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "codex".into(),
        display_name: "Codex CLI".into(),
        cmd: "codex".into(),
        default_args: strs(&["--approval-mode", "full-auto"]),
        protocol: default_protocol(),
        env_key_map: pairs(&[
            ("api_key", "OPENAI_API_KEY"),
            ("base_url", "OPENAI_BASE_URL"),
            ("model", "CODEX_MODEL"),
        ]),
        config_dir_name: ".codex".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&["--approval-mode", "full-auto"]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "npm install -g @openai/codex".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "aider".into(),
        display_name: "Aider".into(),
        cmd: "aider".into(),
        default_args: strs(&["--auto-commits"]),
        protocol: default_protocol(),
        env_key_map: pairs(&[]),
        config_dir_name: "".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&[]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "pip install aider-chat".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "goose".into(),
        display_name: "Goose".into(),
        cmd: "goose".into(),
        default_args: strs(&[]),
        protocol: default_protocol(),
        env_key_map: pairs(&[]),
        config_dir_name: ".config/goose".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&[]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "Download from https://github.com/aaif-goose/goose".into(),
        source: BackendSource::Builtin,
    },
    BackendProfile {
        id: "hermes".into(),
        display_name: "Hermes".into(),
        cmd: "hermes".into(),
        default_args: strs(&[]),
        protocol: default_protocol(),
        env_key_map: pairs(&[]),
        config_dir_name: ".hermes".into(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: strs(&[]),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash".into(),
        source: BackendSource::Builtin,
    },
]
}

// ── 用户自定义引擎（加引擎不必改 Rust 重编译）──────────────────────

/// 用户引擎定义文件：`~/.onecode/engines.json`。
///
/// 目标 ① 是"可换引擎的调度台"。内置的 7 个引擎写在代码里，加第 8 个要改 Rust
/// 重编译 —— 那就不叫"可换"。这个文件让用户自己接一个新 CLI（或 ACP 桥）。
///
/// 形状：
/// ```json
/// { "engines": [
///     { "id": "my-acp", "base": "claude-code", "cmd": "acp-claude", "display_name": "Claude via ACP" },
///     { "id": "local-qwen", "cmd": "qwen", "display_name": "Qwen CLI" }
/// ]}
/// ```
/// `base` = 从一个内置档**继承**，只覆盖写出来的字段。绝大多数自定义引擎只差一两项，
/// 让用户把 claude-code 的 `--append-system-prompt-file` / `--allowedTools` 全抄一遍
/// 既啰嗦又必然抄错。
/// 项目层引擎定义的位置：`<项目>/.onecode/engines.json`。
pub fn project_engines_path(cwd: &Path) -> PathBuf {
    cwd.join(".onecode").join("engines.json")
}

pub fn engines_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".onecode/engines.json")
}

/// 用户引擎定义（**只写要覆盖/新增的字段**；`None` = 继承）。
#[derive(Debug, Clone, serde::Deserialize, Default)]
struct EngineDef {
    id: String,
    /// 从哪个内置档继承（不写 = 从零开始，各项取保守默认）
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    cmd: Option<String>,
    #[serde(default)]
    default_args: Option<Vec<String>>,
    /// 引擎说话的方式：`print`（默认）/ `acp`。不写 = 继承 base。
    /// 用户在 `engines.json` 里就是这样声明一个 ACP 引擎的。
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    env_key_map: Option<Vec<(String, String)>>,
    #[serde(default)]
    config_dir_name: Option<String>,
    #[serde(default)]
    needs_auth_fix: Option<bool>,
    #[serde(default)]
    system_prompt_file_flag: Option<String>,
    #[serde(default)]
    permission_args: Option<Vec<String>>,
    #[serde(default)]
    permission_mode_flag: Option<String>,
    /// 覆盖"该引擎接受的权限档取值"（引擎升级后取值变了就在这里改）
    #[serde(default)]
    permission_mode_values: Option<Vec<String>>,
    #[serde(default)]
    allowed_tools_flag: Option<String>,
    #[serde(default)]
    install_hint: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
struct EnginesFile {
    #[serde(default)]
    engines: Vec<EngineDef>,
}

/// 载入用户引擎时的**问题清单**（坏条目不能被静默丢掉 —— 用户会以为它生效了）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineProblem {
    pub id: String,
    pub problem: String,
}

/// id 白名单：id 会进日志、进槽位记录、进界面；限制字符集免得出现
/// 前后空白/控制字符这类"看起来一样其实不同"的 id。
pub fn is_safe_engine_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 把一条用户定义叠到 base 上。`None` 字段 = 继承。
fn apply_def(mut profile: BackendProfile, def: &EngineDef) -> BackendProfile {
    if let Some(v) = &def.display_name {
        profile.display_name = v.clone();
    }
    if let Some(v) = &def.cmd {
        profile.cmd = v.clone();
    }
    if let Some(v) = &def.default_args {
        profile.default_args = v.clone();
    }
    if let Some(v) = &def.protocol {
        profile.protocol = v.clone();
    }
    if let Some(v) = &def.env_key_map {
        profile.env_key_map = v.clone();
    }
    if let Some(v) = &def.config_dir_name {
        profile.config_dir_name = v.clone();
    }
    if let Some(v) = def.needs_auth_fix {
        profile.needs_auth_fix = v;
    }
    if let Some(v) = &def.system_prompt_file_flag {
        // 空串 = 明确表达"这个引擎没有这个能力"（便于覆盖掉继承来的 flag）
        profile.system_prompt_file_flag = if v.is_empty() { None } else { Some(v.clone()) };
    }
    if let Some(v) = &def.permission_args {
        profile.permission_args = v.clone();
    }
    if let Some(v) = &def.permission_mode_flag {
        profile.permission_mode_flag = if v.is_empty() { None } else { Some(v.clone()) };
    }
    if let Some(v) = &def.permission_mode_values {
        profile.permission_mode_values = v.clone();
    }
    if let Some(v) = &def.allowed_tools_flag {
        profile.allowed_tools_flag = if v.is_empty() { None } else { Some(v.clone()) };
    }
    if let Some(v) = &def.install_hint {
        profile.install_hint = v.clone();
    }
    profile
}

/// 从零开始的一条引擎（没写 base 时用）。
///
/// `cmd` **留空**，不默认成 id：默认等于凭空编一个命令名，用户若没把二进制命名成
/// 同名，spawn 只会给一个难懂的 OS 报错。留空则由 [`load_user_engines`] 明确报
/// "没有 cmd"。`display_name` 默认成 id 是无害的展示信息，可以兜。
fn blank_profile(id: &str) -> BackendProfile {
    BackendProfile {
        id: id.to_string(),
        display_name: id.to_string(),
        cmd: String::new(),
        default_args: Vec::new(),
        protocol: default_protocol(),
        env_key_map: Vec::new(),
        config_dir_name: String::new(),
        needs_auth_fix: false,
        system_prompt_file_flag: None,
        permission_args: Vec::new(),
        permission_mode_flag: None,
        permission_mode_values: Vec::new(),
        allowed_tools_flag: None,
        install_hint: String::new(),
        source: BackendSource::User,
    }
}

/// 读用户引擎文件。返回 `(定义列表, 问题列表)`；文件不存在 = 空（不是错误）。
pub fn load_user_engines(path: &Path) -> (Vec<BackendProfile>, Vec<EngineProblem>) {
    let mut problems: Vec<EngineProblem> = Vec::new();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (Vec::new(), problems);
    };
    let file: EnginesFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            problems.push(EngineProblem {
                id: "(整份文件)".to_string(),
                problem: format!("JSON 解析失败：{e}"),
            });
            return (Vec::new(), problems);
        }
    };

    let builtins = builtin_profiles();
    let mut out: Vec<BackendProfile> = Vec::new();
    for def in file.engines {
        if !is_safe_engine_id(&def.id) {
            problems.push(EngineProblem {
                id: def.id.clone(),
                problem: "id 不合法（只允许字母数字与 - _ .，最长 64）".to_string(),
            });
            continue;
        }
        // 不允许用户定义**覆盖内置 id** 之外的同名重复
        if out.iter().any(|p: &BackendProfile| p.id == def.id) {
            problems.push(EngineProblem {
                id: def.id.clone(),
                problem: "重复定义，后面的被忽略".to_string(),
            });
            continue;
        }
        let mut profile = match def.base.as_deref() {
            Some(base) => match builtins.iter().find(|p| p.id == base) {
                Some(b) => b.clone(),
                None => {
                    problems.push(EngineProblem {
                        id: def.id.clone(),
                        problem: format!("base {base:?} 不是内置引擎，已从零开始"),
                    });
                    blank_profile(&def.id)
                }
            },
            None => blank_profile(&def.id),
        };
        profile = apply_def(profile, &def);
        profile.id = def.id.clone();
        profile.source = BackendSource::User;
        // 没 cmd = 一定启动失败。**跳过并上报**，与"不安全 id / 重复定义"一致：
        // 一个必定失败的条目不该出现在引擎选择器里，但用户必须知道它被忽略了。
        if profile.cmd.trim().is_empty() {
            problems.push(EngineProblem {
                id: def.id.clone(),
                problem: "没有 cmd，启动必然失败，已忽略".to_string(),
            });
            continue;
        }
        out.push(profile);
    }
    (out, problems)
}

/// 本次注册表的快照：内置 + 用户。用户定义**同名则覆盖内置**（改 cmd/参数时最方便）。
#[derive(Debug, Clone)]
pub struct Registry {
    pub profiles: Vec<BackendProfile>,
    pub problems: Vec<EngineProblem>,
}

impl Registry {
    /// 三层合并：**内置 → 全局（你的）→ 项目（跟着仓库走）**。
    ///
    /// ## ★ 项目层只能"新增"，不能"覆盖"
    ///
    /// 这是个安全问题，不是洁癖：你 `git clone` 一个仓库、在调度台里打开它，
    /// 如果项目层能覆盖 `claude-code` 的 `cmd`，那么**开一个槽位就等于执行了
    /// 仓库里写的任意命令** —— 而且你完全看不出来。
    ///
    /// 所以规则是：项目层里 **id 与已有引擎重复的定义一律拒绝**，并如实报出来。
    /// 想在项目里改 claude 的行为？用**新 id + `base` 继承**：
    ///
    /// ```json
    /// { "id": "claude-code-p", "base": "claude-code", "default_args": ["--append-system-prompt-file", ".claude/x.md"] }
    /// ```
    ///
    /// 这也是为什么"项目默认引擎"该由**全局配置**去指 —— 项目不能自己把自己变成默认。
    pub fn load_layers(global_path: &Path, project_cwd: Option<&Path>) -> Self {
        let builtins = builtin_profiles();
        let (user, mut problems) = load_user_engines(global_path);
        let mut profiles: Vec<BackendProfile> = Vec::with_capacity(builtins.len() + user.len());
        for b in builtins {
            match user.iter().find(|u| u.id == b.id) {
                // 同名 → 用用户的（覆盖），但保留 source 标记为 User
                Some(u) => profiles.push(u.clone()),
                None => profiles.push(b),
            }
        }
        for u in user {
            if !profiles.iter().any(|p| p.id == u.id) {
                profiles.push(u);
            }
        }

        // ── 项目层 ──
        if let Some(cwd) = project_cwd {
            let ppath = project_engines_path(cwd);
            let (project, pproblems) = load_user_engines(&ppath);
            problems.extend(pproblems);
            for mut pend in project {
                if profiles.iter().any(|p| p.id == pend.id) {
                    let msg = format!(
                        "{}: 项目层不能覆盖已有引擎 {:?}（克隆一个仓库就能改掉要跑的命令）—— 请用新 id + base 继承",
                        ppath.display(),
                        pend.id
                    );
                    log::warn!("[engines] {msg}");
                    problems.push(EngineProblem {
                        id: pend.id.clone(),
                        problem: msg,
                    });
                    continue;
                }
                pend.source = BackendSource::Project;
                profiles.push(pend);
            }
        }

        Self { profiles, problems }
    }

    pub fn get(&self, id: &str) -> Option<&BackendProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }
}

// ── Lookup ────────────────────────────────────────────────────────

/// 按 id 找，找不到退回 claude-code（内置第一项）。
///
/// 退不回去（理论上不可能）时返回一个最小可用档 —— 绝不 panic：
/// 这是启动终端的路径。
pub fn resolve_or_default(id: &str) -> BackendProfile {
    resolve_in_project(id, None)
}

/// 同 [`resolve_or_default`]，但**带上项目层**（`<cwd>/.onecode/engines.json`）。
///
/// `cwd` 是 `None` 时只看内置 + 全局 —— 与老行为一致。
pub fn resolve_in_project(id: &str, cwd: Option<&Path>) -> BackendProfile {
    let reg = Registry::load_layers(&engines_path(), cwd);
    if let Some(p) = reg.get(id) {
        return p.clone();
    }
    if let Some(p) = reg.get("claude-code") {
        return p.clone();
    }
    let mut fallback = blank_profile("claude-code");
    fallback.cmd = "claude".to_string();
    fallback.display_name = "Claude Code".to_string();
    fallback.install_hint = "npm install -g @anthropic-ai/claude-code".to_string();
    fallback
}

// ── Detection ─────────────────────────────────────────────────────

/// Detect which backends are installed by running `which <cmd>` for each profile.
///
/// Uses the full PATH (resolved from user shell) instead of the GUI process PATH,
/// because macOS GUI apps don't inherit the user's shell PATH — they only get
/// `/usr/bin:/bin:/usr/sbin:/sbin`. This causes `which` to miss backends installed
/// in `~/.local/bin`, `/opt/homebrew/bin`, nvm/fnm paths, etc.
///
/// The PATH resolution logic mirrors `pty::resolve_full_path()`.
/// 同 [`detect_installed`]，但可带**项目层**（`<cwd>/.onecode/engines.json`）。
pub fn detect_installed_in(cwd: Option<&Path>) -> Vec<(BackendProfile, bool)> {
    // Resolve full PATH from user shell (same strategy as pty spawn)
    let resolved_path = resolve_detection_path();

    Registry::load_layers(&engines_path(), cwd)
        .profiles
        .into_iter()
        .map(|p| {
            let found = std::process::Command::new("which")
                .arg(&p.cmd)
                .env("PATH", &resolved_path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            (p, found)
        })
        .collect()
}

/// Resolve the full PATH for backend detection.
///
/// On macOS, GUI apps launched from Dock/Finder get a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`). Backends like `opencode`, `codex`, `claude`
/// are typically installed in user directories (`~/.local/bin`, `/opt/homebrew/bin`,
/// nvm/fnm bun paths, etc.) that are only in the user's shell PATH.
///
/// Strategy (mirrors `pty::resolve_full_path()`):
/// 1. Try `$SHELL -l -i -c 'echo $PATH'` (login + interactive → sources .zshrc)
/// 2. Try `$SHELL -l -c 'echo $PATH'` (login only → sources .zprofile)
/// 3. Fallback: parent PATH + common user directories
/// 4. Always ensure `~/.local/bin` is in PATH
fn resolve_detection_path() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".to_string());
    let parent_path = std::env::var("PATH").unwrap_or_default();

    // Try login + interactive shell (sources .zshrc → includes nvm/fnm/bun etc.)
    for args in &[
        &["-l", "-i", "-c", "echo $PATH"][..],
        &["-l", "-c", "echo $PATH"],
    ] {
        if let Ok(output) = std::process::Command::new(&shell).args(*args).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && path.contains('/') {
                    return ensure_local_bin(&path, &home);
                }
            }
        }
    }

    // Fallback: parent PATH + common user directories
    let fallback = format!(
        "{home}/.local/bin:{home}/.bun/bin:{home}/.icode/bin:{home}/.cargo/bin:\
         {home}/bin:/opt/homebrew/bin:/usr/local/bin:{parent_path}",
        home = home,
        parent_path = parent_path
    );
    ensure_local_bin(&fallback, &home)
}

/// Ensure PATH includes $HOME/.local/bin (claude's default install location).
fn ensure_local_bin(path: &str, home: &str) -> String {
    let local_bin = format!("{home}/.local/bin", home = home);
    if path.split(':').any(|p| p == local_bin) {
        path.to_string()
    } else {
        format!("{local_bin}:{path}")
    }
}

// ── IPC serializable type ─────────────────────────────────────────

/// Serializable backend info sent to the frontend via IPC.
#[derive(Clone, Debug, Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub cmd: String,
    pub default_args: Vec<String>,
    /// 引擎**说话的方式**：
    /// - `print`（默认）：`-p <prompt> --output-format stream-json`，一问一答；
    /// - `acp`：Agent Client Protocol（JSON-RPC over stdio，双向，可中途要权限）。
    ///
    /// 为什么要有这个字段：ACP 引擎**不吃** `-p`，也不该被塞进"打印模式"的参数。
    /// 调度台需要按协议分流，而不是假设所有引擎都长得像 `claude -p`。
    pub protocol: String,
    pub env_key_map: Vec<(String, String)>,
    pub config_dir_name: String,
    pub needs_auth_fix: bool,
    pub install_hint: String,
    pub installed: bool,
    /// `builtin` | `user`（界面要能区分"内置的"与"你自己加的"）
    pub source: BackendSource,
}

impl From<&BackendProfile> for BackendInfo {
    fn from(p: &BackendProfile) -> Self {
        Self {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            cmd: p.cmd.clone(),
            default_args: p.default_args.clone(),
            protocol: p.protocol.clone(),
            env_key_map: p.env_key_map.clone(),
            config_dir_name: p.config_dir_name.clone(),
            needs_auth_fix: p.needs_auth_fix,
            install_hint: p.install_hint.clone(),
            installed: false, // set by detect_installed
            source: p.source,
        }
    }
}

/// IPC command: list all backends with their installation status.
#[tauri::command]
pub fn list_backends(cwd: Option<String>) -> Vec<BackendInfo> {
    let detected = detect_installed_in(cwd.as_deref().map(Path::new));
    detected
        .iter()
        .map(|(p, found)| {
            let mut info = BackendInfo::from(p);
            info.installed = *found;
            info
        })
        .collect()
}

/// 引擎列表的**人看到的样子**（纯函数，便于单测）。
pub fn engines_text(path: &Path, detected: &[(BackendProfile, bool)], problems: &[EngineProblem]) -> String {
    let mut s = format!("引擎（{} 个）\n\n", detected.len());
    for (p, installed) in detected {
        let mark = if *installed { "✅" } else { "✗ 未装" };
        let src = match p.source {
            BackendSource::Builtin => "内置",
            BackendSource::User => "**你自己加的**",
            // 项目层必须一眼看得出：它跟着仓库走，不是你装的
            BackendSource::Project => "**来自项目**（跟着仓库走）",
        };
        s.push_str(&format!("{mark}  {}  ({})\n", p.id, src));
        s.push_str(&format!("      {} → {}\n", p.display_name, p.cmd));
        let caps: Vec<&str> = [
            p.system_prompt_file_flag.as_deref().map(|_| "身份注入"),
            p.permission_mode_flag.as_deref().map(|_| "权限档"),
            p.allowed_tools_flag.as_deref().map(|_| "工具白名单"),
        ]
        .iter()
        .flatten()
        .copied()
        .collect();
        if !caps.is_empty() {
            s.push_str(&format!("      能力：{}\n", caps.join(" / ")));
        }
    }
    if !problems.is_empty() {
        s.push_str(&format!("\n⚠ 定义有问题（{} 条，未生效）：\n", problems.len()));
        for pr in problems {
            s.push_str(&format!("  {}：{}\n", pr.id, pr.problem));
        }
    }
    s.push_str(&format!("\n加引擎：写 {}\n", path.display()));
    s.push_str("（形状见「引擎配置示例」）");
    s
}

/// 一份可直接抄的示例内容（JSON 文本）。
///
/// 它曾同时被注册成 Tauri 命令，但前端从不调用（`engines_example_show` 直接用它）。
/// 第 18 轮的自检抓到这类"注册了却没人调"的死表面，于是去掉了命令注册 —— 函数留下，
/// 因为它确实被用着。
pub fn engines_example() -> String {
    r#"{
  "engines": [
    {
      "id": "claude-via-acp",
      "base": "claude-code",
      "cmd": "claude-agent-acp",
      "protocol": "acp",
      "display_name": "Claude Code (ACP)",
      "install_hint": "npm install -g @zed-industries/claude-agent-acp"
    },
    {
      "id": "local-qwen",
      "base": "codex",
      "cmd": "qwen",
      "display_name": "Qwen CLI (本地)"
    }
  ]
}"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只在内置档里找 —— 测试**不能**走 `resolve_or_default`（那会读真实的
    /// `~/.onecode/engines.json`，用户一旦覆盖了内置档，测试结果就跟着变）。
    fn builtin(id: &str) -> BackendProfile {
        builtin_profiles()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("内置档必须有 {id}"))
    }

    fn cc() -> BackendProfile {
        builtin("claude-code")
    }

    #[test]
    fn posture_parse_accepts_documented_values() {
        assert_eq!(Posture::parse(""), Some(Posture::Inherit));
        assert_eq!(Posture::parse("  "), Some(Posture::Inherit));
        assert_eq!(Posture::parse("bypassPermissions"), Some(Posture::BypassPermissions));
        assert_eq!(Posture::parse("acceptEdits"), Some(Posture::AcceptEdits));
        assert_eq!(Posture::parse("default"), Some(Posture::Default));
        // `planMode` 是**本产品第 3 刀写错过的值**：仍然接受这个输入（兼容旧配置），
        // 但 `mode()` 输出必须是 CLI 真正接受的 `plan`。
        assert_eq!(Posture::parse("planMode"), Some(Posture::Plan));
        assert_eq!(Posture::parse("plan").map(|p| p.mode()), Some(Some("plan")));
        assert_eq!(Posture::parse("planMode").map(|p| p.mode()), Some(Some("plan")));
        assert_eq!(Posture::parse("dontAsk"), Some(Posture::DontAsk));
        assert_eq!(Posture::DontAsk.mode(), Some("dontAsk"));
        assert_eq!(Posture::parse("accept-edits"), Some(Posture::AcceptEdits));
        // 大小写/下划线不敏感
        assert_eq!(Posture::parse("AcceptEdits"), Some(Posture::AcceptEdits));
        assert_eq!(Posture::parse("read_only"), Some(Posture::Plan));
    }

    #[test]
    fn posture_parse_rejects_unknown_so_caller_can_warn() {
        // 未识别必须返回 None：静默当 Inherit 会让人以为门开了、其实没开
        assert_eq!(Posture::parse("whatever"), None);
        assert_eq!(Posture::parse("sudo"), None);
    }

    #[test]
    fn inherit_leaves_args_untouched() {
        let mut args: Vec<String> = cc().default_args.iter().map(|s| s.to_string()).collect();
        let before = args.clone();
        let out = apply_posture_args(&cc(), Posture::Inherit, &[], &mut args);
        assert_eq!(args, before, "不干预就必须一个参数都不动");
        assert!(out.mode_applied.is_none());
        assert!(out.unsupported_reason.is_none());
    }

    #[test]
    fn accept_edits_replaces_the_bypass_defaults() {
        let mut args: Vec<String> = cc().default_args.iter().map(|s| s.to_string()).collect();
        let out = apply_posture_args(&cc(), Posture::AcceptEdits, &[], &mut args);
        assert_eq!(out.mode_applied, Some("acceptEdits"));
        // 关键：不能同时留下两个 --permission-mode（claude 取哪个取决于解析顺序）
        assert_eq!(args.iter().filter(|a| *a == "--permission-mode").count(), 1);
        assert!(args.contains(&"acceptEdits".to_string()));
        assert!(!args.contains(&"bypassPermissions".to_string()));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn applied_posture_is_idempotent() {
        // 恢复会话时持久化的 args 已带过姿态，会被再应用一次 —— 必须收敛
        let mut args: Vec<String> = cc().default_args.iter().map(|s| s.to_string()).collect();
        let tools = vec!["Read".to_string(), "Bash(git:*)".to_string()];
        apply_posture_args(&cc(), Posture::Default, &tools, &mut args);
        let once = args.clone();
        let out = apply_posture_args(&cc(), Posture::Default, &tools, &mut args);
        assert_eq!(args, once, "重复应用必须收敛到同一份参数");
        assert_eq!(out.mode_applied, Some("default"));
    }

    #[test]
    fn allowed_tools_are_written_once_and_joined() {
        let mut args: Vec<String> = Vec::new();
        let tools = vec!["Read".to_string(), "Bash(git:*)".to_string()];
        let out = apply_posture_args(&cc(), Posture::AcceptEdits, &tools, &mut args);
        assert_eq!(out.tools_applied, tools);
        assert_eq!(args.iter().filter(|a| *a == "--allowedTools").count(), 1);
        assert!(args.contains(&"Read,Bash(git:*)".to_string()));
    }

    #[test]
    fn unsupported_backend_is_reported_not_silently_ignored() {
        // opencode 没有声明这两条 flag → 必须如实回报，不能假装生效
        let oc = builtin("opencode");
        let mut args: Vec<String> = Vec::new();
        let out = apply_posture_args(&oc, Posture::AcceptEdits, &["Read".to_string()], &mut args);
        assert!(out.mode_applied.is_none());
        assert!(out.tools_applied.is_empty());
        assert!(out.unsupported_reason.is_some());
        assert!(args.is_empty(), "不支持就不该改参数");
    }

    #[test]
    fn equals_form_of_permission_mode_is_also_stripped() {
        let mut args = vec!["--permission-mode=acceptEdits".to_string(), "--other".to_string()];
        apply_posture_args(&cc(), Posture::Plan, &[], &mut args);
        assert!(!args.iter().any(|a| a.starts_with("--permission-mode=")), "上次的 = 形态必须清掉");
        assert_eq!(args.iter().filter(|a| *a == "--permission-mode").count(), 1);
        assert!(args.contains(&"plan".to_string()), "CLI 只接受 plan，不接受 planMode");
        assert!(!args.contains(&"planMode".to_string()), "绝不能把非法值递出去");
        assert!(args.contains(&"--other".to_string()), "无关参数不能被误删");
    }

    #[test]
    fn tools_without_permission_infers_accept_edits_and_says_so() {
        // 策略决定：白名单在 bypassPermissions 下形同虚设，所以只声明 tools 时
        // 收窄到 acceptEdits —— 但必须标记为"推断"，不能让人以为是自己设的。
        let mut args: Vec<String> = cc().default_args.iter().map(|s| s.to_string()).collect();
        let tools = vec!["Read".to_string(), "Bash".to_string()];
        let out = apply_posture_args(&cc(), Posture::Inherit, &tools, &mut args);
        assert!(out.mode_inferred, "必须是推断");
        assert_eq!(out.mode_applied, Some("acceptEdits"));
        assert_eq!(out.tools_applied, tools);
        assert!(!args.contains(&"bypassPermissions".to_string()));
    }

    #[test]
    fn default_permission_args_are_removed_as_a_whole_slice() {
        // 回归：整段匹配。逐 token 删会删掉 --permission-mode 却留下
        // 孤立的 "bypassPermissions"，重复应用就会累积垃圾。
        let mut args = vec![
            "--verbose".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--other".to_string(),
        ];
        apply_posture_args(&cc(), Posture::Plan, &[], &mut args);
        assert!(!args.contains(&"bypassPermissions".to_string()), "不能留下孤立取值");
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--verbose".to_string()), "无关参数不能被误删");
        assert!(args.contains(&"--other".to_string()));
        assert_eq!(args.iter().filter(|a| *a == "--permission-mode").count(), 1);
        assert!(args.contains(&"plan".to_string()), "CLI 只接受 plan，不接受 planMode");
        assert!(!args.contains(&"planMode".to_string()), "绝不能把非法值递出去");
    }

    // ── 可扩展引擎定义（目标 ①：加引擎不该要重编译）──────────────────

    fn tmp_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-engines-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_engines(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("engines.json");
        std::fs::write(&p, body).unwrap();
        p
    }

    /// 回归：没有配置文件时，内置 7 个必须**一字不差**（尤其 claude-code 的 flag）。
    #[test]
    fn builtins_are_unchanged_without_a_config_file() {
        let reg = Registry::load_layers(Path::new("/definitely/not/here/engines.json"), None);
        assert!(reg.problems.is_empty());
        assert_eq!(reg.profiles.len(), 7);
        assert!(reg.profiles.iter().all(|p| p.source == BackendSource::Builtin));

        let cc = reg.get("claude-code").unwrap();
        assert_eq!(cc.cmd, "claude");
        assert_eq!(cc.system_prompt_file_flag.as_deref(), Some("--append-system-prompt-file"));
        assert_eq!(cc.permission_mode_flag.as_deref(), Some("--permission-mode"));
        assert_eq!(cc.allowed_tools_flag.as_deref(), Some("--allowedTools"));
        assert_eq!(
            cc.default_args,
            vec!["--permission-mode", "bypassPermissions", "--dangerously-skip-permissions"]
        );
        assert_eq!(
            cc.env_key_map,
            vec![
                ("api_key".to_string(), "ANTHROPIC_API_KEY".to_string()),
                ("base_url".to_string(), "ANTHROPIC_BASE_URL".to_string()),
                ("model".to_string(), "ANTHROPIC_MODEL".to_string()),
            ]
        );
        assert!(cc.needs_auth_fix);
    }

    #[test]
    fn a_user_can_add_a_brand_new_engine_without_recompiling() {
        let dir = tmp_dir("new");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"my-cli","cmd":"my-cli","display_name":"My CLI","default_args":["--yes"]}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        assert!(reg.problems.is_empty(), "{:?}", reg.problems);
        assert_eq!(reg.profiles.len(), 8);
        let mine = reg.get("my-cli").expect("新引擎要能被找到");
        assert_eq!(mine.cmd, "my-cli");
        assert_eq!(mine.display_name, "My CLI");
        assert_eq!(mine.default_args, vec!["--yes"]);
        assert_eq!(mine.source, BackendSource::User);
        // 从零开始 → 没有继承来的能力（不能凭空声称支持 system prompt 注入）
        assert!(mine.system_prompt_file_flag.is_none());
        assert!(mine.env_key_map.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `base` 继承：只写要改的字段。这是最常用的形态 —— 抄一整套 flag 必然抄错。
    #[test]
    fn base_inheritance_keeps_the_parent_capabilities() {
        let dir = tmp_dir("base");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"claude-acp","base":"claude-code","cmd":"claude-agent-acp","display_name":"Claude (ACP)"}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        let e = reg.get("claude-acp").unwrap();
        assert_eq!(e.cmd, "claude-agent-acp", "覆盖了 cmd");
        assert_eq!(e.display_name, "Claude (ACP)");
        // 其余全部继承
        assert_eq!(e.system_prompt_file_flag.as_deref(), Some("--append-system-prompt-file"));
        assert_eq!(e.allowed_tools_flag.as_deref(), Some("--allowedTools"));
        assert_eq!(e.default_args, cc().default_args);
        assert!(e.needs_auth_fix);
        assert_eq!(e.source, BackendSource::User);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空串 = **明确表达"这个引擎没有这个能力"**（用来覆盖掉继承来的 flag）。
    #[test]
    fn empty_string_explicitly_disables_an_inherited_capability() {
        let dir = tmp_dir("disable");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"no-inject","base":"claude-code","cmd":"x","system_prompt_file_flag":"","allowed_tools_flag":""}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        let e = reg.get("no-inject").unwrap();
        assert!(e.system_prompt_file_flag.is_none(), "空串要能关掉继承来的能力");
        assert!(e.allowed_tools_flag.is_none());
        // 没写的仍然继承
        assert_eq!(e.permission_mode_flag.as_deref(), Some("--permission-mode"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_definition_with_a_builtin_id_overrides_it() {
        let dir = tmp_dir("override");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"hermes","cmd":"hermes-next","display_name":"Hermes (next)"}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        assert_eq!(reg.profiles.len(), 7, "覆盖不该让档位变多");
        let h = reg.get("hermes").unwrap();
        assert_eq!(h.cmd, "hermes-next");
        assert_eq!(h.source, BackendSource::User, "来源要标成用户定义");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏定义不能静默丢掉 —— 用户会以为它生效了。
    #[test]
    fn broken_definitions_are_reported_and_builtins_still_work() {
        let dir = tmp_dir("broken");
        let path = write_engines(&dir, "{ this is not json");
        let reg = Registry::load_layers(&path, None);
        assert_eq!(reg.profiles.len(), 7, "坏文件不影响内置档");
        assert_eq!(reg.problems.len(), 1);
        assert!(reg.problems[0].problem.contains("JSON"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsafe_and_duplicate_and_missing_cmd_are_each_reported() {
        let dir = tmp_dir("problems");
        let path = write_engines(
            &dir,
            r#"{"engines":[
                {"id":"../escape","cmd":"x"},
                {"id":"dup","cmd":"a"},
                {"id":"dup","cmd":"b"},
                {"id":"no-cmd"}
            ]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        let ids: Vec<&str> = reg.problems.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"../escape"), "不安全的 id 要报出来：{ids:?}");
        assert!(ids.contains(&"dup"), "重复定义要报出来");
        assert!(ids.contains(&"no-cmd"), "没有 cmd 要报出来（必然启动失败）");
        assert!(reg.get("dup").is_some(), "第一个 dup 仍然可用");
        assert!(reg.get("dup").map(|p| p.cmd.as_str()) == Some("a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_base_is_reported_and_engine_starts_from_blank() {
        let dir = tmp_dir("badbase");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"x","base":"nope","cmd":"x"}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        assert!(reg.problems.iter().any(|p| p.problem.contains("base")));
        let e = reg.get("x").unwrap();
        assert!(e.system_prompt_file_flag.is_none(), "base 找不到就不能假装继承了能力");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_id_whitelist() {
        assert!(is_safe_engine_id("claude-code"));
        assert!(is_safe_engine_id("my.acp_2"));
        assert!(!is_safe_engine_id(""));
        assert!(!is_safe_engine_id("../escape"));
        assert!(!is_safe_engine_id("has space"));
        assert!(!is_safe_engine_id(&"x".repeat(65)));
    }

    /// 用户定义要能真的**驱动 spawn 参数**（不只是被读到）。
    #[test]
    fn a_user_engine_drives_posture_args_by_inheritance() {
        let dir = tmp_dir("drives");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"claude-acp","base":"claude-code","cmd":"acp"}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        let e = reg.get("claude-acp").unwrap();
        let mut args: Vec<String> = e.default_args.clone();
        let out = apply_posture_args(e, Posture::Plan, &[], &mut args);
        assert_eq!(out.mode_applied, Some("plan"), "继承来的权限能力必须仍然生效");
        assert!(!args.contains(&"bypassPermissions".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engines_text_marks_source_and_problems() {
        let dir = tmp_dir("text");
        let path = write_engines(
            &dir,
            r#"{"engines":[{"id":"mine","cmd":"mine","display_name":"Mine"},{"id":"bad"}]}"#,
        );
        let reg = Registry::load_layers(&path, None);
        assert_eq!(reg.profiles.len(), 8, "没 cmd 的那条要被跳过");
        assert!(reg.get("bad").is_none(), "必定启动失败的条目不该出现在选择器里");
        let detected: Vec<(BackendProfile, bool)> =
            reg.profiles.iter().cloned().map(|p| (p, true)).collect();
        let text = engines_text(&path, &detected, &reg.problems);
        eprintln!("===== 引擎列表（人看到的）=====\n{text}\n===== end =====");
        assert!(text.contains("**你自己加的**"), "来源要标出来：{text}");
        assert!(text.contains("内置"));
        assert!(text.contains("定义有问题"), "坏条目必须显眼：{text}");
        assert!(text.contains("没有 cmd"), "{text}");
        assert!(text.contains("加引擎：写"), "要告诉用户文件在哪");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engines_text_lists_only_the_capabilities_a_backend_really_has() {
        let detected = vec![(cc(), true), (builtin("opencode"), false)];
        let text = engines_text(Path::new("/tmp/engines.json"), &detected, &[]);
        assert!(text.contains("身份注入"), "claude-code 有身份注入");
        assert!(text.contains("✗ 未装"), "没装的要标出来");
        // opencode 没有任何一项能力 → 不应该出现"能力："行两次
        assert_eq!(text.matches("能力：").count(), 1, "{text}");
    }

    /// ★ 结构性防护：引擎**不接受**的权限档绝不能递出去 ——
    /// 传错不是警告，是 CLI 直接退出、终端根本起不来（实测 2.1.63 的 planMode 事故）。
    #[test]
    fn an_unaccepted_permission_mode_is_reported_and_never_passed_through() {
        // 造一个"只接受 default"的引擎（模拟引擎升级后取值收窄）
        let mut narrow = cc();
        narrow.permission_mode_values = vec!["default".to_string()];
        let mut args = narrow.default_args.clone();

        let out = apply_posture_args(&narrow, Posture::Plan, &[], &mut args);
        assert!(out.mode_applied.is_none(), "不能递出不被接受的值");
        let reason = out.unsupported_reason.unwrap_or_default();
        assert!(reason.contains("plan"), "要说清是哪个值被拒：{reason}");
        assert!(reason.contains("default"), "还要把可用值列出来：{reason}");
        assert!(!args.contains(&"plan".to_string()), "参数里绝不能出现它");
        // 但默认权限参数已经被清掉了（姿态仍然生效为"不 bypass"）
        assert!(!args.contains(&"bypassPermissions".to_string()));
    }

    #[test]
    fn claude_code_declares_exactly_the_values_it_accepts() {
        // 实测白名单（cli.js 报错原文）：acceptEdits / bypassPermissions / default / dontAsk / plan
        let values = cc().permission_mode_values;
        assert_eq!(values, vec!["acceptEdits", "bypassPermissions", "default", "dontAsk", "plan"]);
        assert!(!values.iter().any(|v| v == "planMode"), "planMode 不在可接受集合里");
        for p in [
            Posture::AcceptEdits,
            Posture::BypassPermissions,
            Posture::Default,
            Posture::Plan,
            Posture::DontAsk,
        ] {
            let mode = p.mode().unwrap();
            assert!(values.iter().any(|v| v == mode), "{mode} 必须在白名单里");
        }
    }

    #[test]
    fn posture_values_serialise_and_apply_for_every_variant() {
        for p in [Posture::AcceptEdits, Posture::BypassPermissions, Posture::Default, Posture::Plan, Posture::DontAsk] {
            let mut args = cc().default_args.clone();
            let out = apply_posture_args(&cc(), p, &[], &mut args);
            assert_eq!(out.mode_applied, p.mode(), "{p:?} 应生效");
            assert!(out.unsupported_reason.is_none(), "{p:?} 不该有未生效原因");
            assert_eq!(
                args.iter().filter(|a| *a == "--permission-mode").count(),
                1,
                "{p:?} 只能有一个 --permission-mode"
            );
        }
    }

    // ── ★ 安全不变量：**默认不提权** ──────────────────────────────────
    //
    // 这是本产品最要紧的一条默认值：调度台会拿 AI 员工去跑 shell。
    // 所以"员工没写权限"时必须是**引擎自己的默认**（Claude Code = 会先问），
    // 绝不能是任何形式的跳过确认。以下几条把这件事钉死。

    #[test]
    fn an_empty_permission_field_means_inherit_never_bypass() {
        // 空字段（员工根本没写）→ 继承。**绝不能**解析成 bypass。
        assert_eq!(Posture::parse(""), Some(Posture::Inherit));
        assert_eq!(Posture::parse("   "), Some(Posture::Inherit));
        assert_ne!(Posture::parse(""), Some(Posture::BypassPermissions));
    }

    #[test]
    fn an_unrecognised_permission_value_is_not_silently_treated_as_bypass() {
        // 拼错/乱写 → None（由 spawn_plan 报出来并退回继承），不能"猜"成提权
        assert_eq!(Posture::parse("bypas"), None);
        assert_eq!(Posture::parse("随便写的"), None);
        assert_eq!(Posture::parse("yes"), None);
    }

    #[test]
    fn inherit_appends_no_permission_arguments_at_all() {
        let profile = builtin_profiles()
            .into_iter()
            .find(|p| p.id == "claude-code")
            .expect("内置 claude-code");
        let mut args: Vec<String> = vec!["-p".into(), "你好".into()];
        let before = args.clone();
        let outcome = apply_posture_args(&profile, Posture::Inherit, &[], &mut args);
        assert!(outcome.mode_applied.is_none(), "继承不该写 mode");
        assert_eq!(args, before, "继承不该改动参数：{args:?}");
        // 任何形式的"跳过确认"参数都不许出现
        for a in &args {
            assert!(
                !a.contains("dangerously") && !a.contains("bypass") && !a.contains("yolo"),
                "默认路径出现了提权参数：{a}"
            );
        }
    }

    #[test]
    fn only_an_explicit_bypass_escalates() {
        let profile = builtin_profiles()
            .into_iter()
            .find(|p| p.id == "claude-code")
            .expect("内置 claude-code");
        let mut args: Vec<String> = Vec::new();
        let outcome = apply_posture_args(&profile, Posture::BypassPermissions, &[], &mut args);
        assert_eq!(outcome.mode_applied.as_deref(), Some("bypassPermissions"));
        assert!(args.iter().any(|a| a == "bypassPermissions"), "{args:?}");
        // 反过来：`plan` 是只读档，必须用它自己的值（历史坑：planMode 会让 CLI 秒退）
        let mut args2: Vec<String> = Vec::new();
        apply_posture_args(&profile, Posture::Plan, &[], &mut args2);
        assert!(args2.iter().any(|a| a == "plan"), "{args2:?}");
        assert!(!args2.iter().any(|a| a == "planMode"), "planMode 是非法值：{args2:?}");
    }

    // ── ★ 项目层引擎定义：**只能新增，不能覆盖** ──────────────────────
    //
    // 安全理由：`git clone` 一个仓库、在调度台里打开它 —— 如果项目层能覆盖
    // `claude-code` 的 `cmd`，那么开一个槽位就等于执行仓库里写的任意命令。

    fn tmp_project(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("dsh-eng-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".onecode")).unwrap();
        d
    }

    fn write_project_engines(cwd: &Path, body: &str) {
        std::fs::write(project_engines_path(cwd), body).unwrap();
    }

    #[test]
    fn a_project_can_add_its_own_engine() {
        let cwd = tmp_project("add");
        write_project_engines(
            &cwd,
            r#"{"engines":[{"id":"proj-echo","cmd":"/bin/echo","display_name":"项目里的回声","protocol":"print"}]}"#,
        );
        let reg = Registry::load_layers(Path::new("/definitely/not/here.json"), Some(&cwd));
        let p = reg.get("proj-echo").expect("项目引擎该出现");
        assert_eq!(p.source, BackendSource::Project, "来源要标成项目");
        assert_eq!(p.cmd, "/bin/echo");
        // 内置的还在
        assert!(reg.get("claude-code").is_some());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// ★ 安全不变量：项目层**不能覆盖**任何已有引擎。
    #[test]
    fn a_project_may_not_override_an_existing_engine() {
        let cwd = tmp_project("override");
        write_project_engines(
            &cwd,
            r#"{"engines":[{"id":"claude-code","cmd":"/tmp/evil.sh","default_args":["--pwn"]}]}"#,
        );
        let reg = Registry::load_layers(Path::new("/definitely/not/here.json"), Some(&cwd));
        let p = reg.get("claude-code").expect("内置 claude-code");
        assert_eq!(p.source, BackendSource::Builtin, "内置档不能被项目改掉");
        assert_eq!(p.cmd, "claude", "命令绝不能被项目覆盖");
        assert!(!p.default_args.iter().any(|a| a == "--pwn"));
        assert!(
            reg.problems.iter().any(|pr| pr.problem.contains("项目层不能覆盖")),
            "要如实报出来：{:?}",
            reg.problems
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 想在项目里改行为 → **新 id + base 继承**（这是被允许的路）。
    #[test]
    fn a_project_engine_can_inherit_from_a_builtin_with_a_new_id() {
        let cwd = tmp_project("base");
        write_project_engines(
            &cwd,
            r#"{"engines":[{"id":"claude-code-p","base":"claude-code","display_name":"本项目的 Claude"}]}"#,
        );
        let reg = Registry::load_layers(Path::new("/definitely/not/here.json"), Some(&cwd));
        let p = reg.get("claude-code-p").expect("继承档该出现");
        assert_eq!(p.source, BackendSource::Project);
        let base = reg.get("claude-code").unwrap();
        assert_eq!(p.cmd, base.cmd, "没写的字段继承 base");
        assert_eq!(p.permission_mode_flag, base.permission_mode_flag);
        assert_eq!(p.display_name, "本项目的 Claude", "写了的字段覆盖");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn without_a_project_layer_nothing_changes() {
        let reg = Registry::load_layers(Path::new("/definitely/not/here.json"), None);
        assert_eq!(reg.profiles.len(), 7);
        assert!(reg.profiles.iter().all(|p| p.source == BackendSource::Builtin));
    }

    #[test]
    fn the_engine_list_marks_a_project_engine_as_coming_from_the_repo() {
        let cwd = tmp_project("text");
        write_project_engines(
            &cwd,
            r#"{"engines":[{"id":"proj-x","cmd":"/bin/true","display_name":"X"}]}"#,
        );
        let reg = Registry::load_layers(Path::new("/definitely/not/here.json"), Some(&cwd));
        let detected: Vec<(BackendProfile, bool)> = reg
            .profiles
            .iter()
            .cloned()
            .map(|p| (p, true))
            .collect();
        let text = engines_text(Path::new("/tmp/x.json"), &detected, &reg.problems);
        assert!(text.contains("来自项目"), "要说清它跟着仓库走：{text}");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
