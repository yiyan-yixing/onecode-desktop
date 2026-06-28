# PRD: OneCode Desktop GUI Setup Wizard

| Field | Value |
|-------|-------|
| Product | onecode-desktop |
| Version | v0.2.0 |
| Status | Draft |
| Author | PM |
| Scope | MVP (1-week delivery) |
| Platform | macOS only |

---

## 1. Product Goal & Success Metrics

### 1.1 User Value

首次启动 OneCode Desktop 的用户面临三个障碍：(1) 不知道本机是否已安装必要依赖；(2) 不知道如何配置 API 连接信息；(3) 面对空白终端界面无所适从。Setup Wizard 将首次启动体验从"黑盒猜测"变为"三步引导"，让用户在 60 秒内从零到可用。

### 1.2 Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Wizard 完成率 | >= 90% | `wizard_completed=true` 的配置文件数 / 首次启动检测数 |
| Wizard 平均耗时 | <= 90 秒 | `mark_wizard_completed` 时间戳 - `is_first_run` 检测时间戳 |
| Wizard 完成后首次终端创建率 | >= 85% | Wizard 完成后 5 分钟内 `pty_spawn` 调用数 / Wizard 完成数 |
| 环境检测误报率 | <= 5% | 用户在 Step 1 标记"已安装"但检测为缺失的反馈 / 总缺失检测数 |

---

## 2. User Stories

### US-1: 首次启动环境检测

> As a first-time user, I want to see which dependencies are installed on my Mac, so that I know what to install before using OneCode.

### US-2: 缺失依赖的明确提示

> As a first-time user, when a dependency is missing, I want to see a clear message telling me what to install and where to get it, so that I can fix the problem without searching online.

### US-3: API 配置

> As a first-time user, I want to enter my API Key, Base URL, and Model name in a guided form, so that OneCode can connect to the AI backend without me editing config files manually.

### US-4: 配置验证反馈

> As a first-time user, I want immediate feedback if my API Key format is invalid, so that I can correct it before proceeding.

### US-5: 顺利进入主界面

> As a first-time user, after completing the setup, I want to seamlessly enter the main terminal interface, so that I can start using OneCode immediately.

### US-6: 非首次启动跳过

> As a returning user, I want the wizard to never appear again after my first completion, so that I am not interrupted on subsequent launches.

### US-7: 检测失败可重试

> As a user who installed a missing dependency during the wizard, I want to re-run the environment check, so that I can see the updated status without restarting the app.

### US-8: 配置保存失败可恢复

> As a user whose config file write fails (e.g., disk full), I want to see an error message and retry, so that I do not lose my entered configuration.

---

## 3. Detailed Feature Specification

### 3.1 Step 1: Environment Detection

#### 3.1.1 Dependencies to Check

| Dependency | Check Method | Minimum Version | Purpose |
|------------|-------------|----------------|---------|
| Claude Code CLI | `which claude` + `claude --version` | N/A (any) | AI backend |
| Node.js | `which node` + `node --version` | v18.0.0 | Claude Code runtime dependency |
| Git | `which git` + `git --version` | 2.0.0 | Version control |

Note: V1 does NOT auto-install any dependency. V1 only detects and provides manual install instructions.

#### 3.1.2 UI Interaction

- Full-screen wizard overlay replaces main app content (wizard overlay on top of `#app`).
- Each dependency shown as a row:
  - Left: dependency name + minimum version text (e.g., "Node.js v18+")
  - Right: status icon
    - Green check: installed and version OK
    - Yellow warning: installed but version below minimum
    - Red X: not found
  - Below row (if not OK): install hint text with link-like styling
- "Re-check" button at bottom left (re-runs detection without leaving step).
- "Next" button at bottom right:
  - Enabled regardless of detection results (user can proceed with missing deps, but a warning badge appears on the Step 2 indicator)
  - Button text: "Next"

Install hint texts:

| Dependency | Hint |
|------------|------|
| Claude Code CLI | "Install: npm install -g @anthropic-ai/claude-code" |
| Node.js | "Install: brew install node (or visit nodejs.org)" |
| Git | "Install: brew install git (or Xcode Command Line Tools)" |

#### 3.1.3 Data Flow

```
Frontend (wizard.js)
  --> ipc.checkEnvironment()
    --> Tauri invoke "check_environment"
      --> Rust: runs `which` + `--version` for each dep
        --> returns CheckEnvironmentResult
  <-- renders result to UI
```

#### 3.1.4 Boundary Conditions

| Condition | Behavior |
|-----------|----------|
| `which` command returns non-zero exit code | Mark dependency as "not found", show red X + install hint |
| `--version` output cannot be parsed | Mark dependency as "installed (version unknown)", show green check with note |
| `--version` returns version below minimum | Mark as "version too low", show yellow warning + hint |
| Detection command hangs (timeout) | 5-second timeout per command; on timeout, mark as "detection failed" with red X + hint "detection timed out, try re-check" |
| ~/.onecode/ directory does not exist yet | Not relevant for this step; detection does not write anything |

---

### 3.2 Step 2: API Configuration

#### 3.2.1 Fields to Configure

| Field | Label | Type | Default | Validation |
|-------|-------|------|---------|------------|
| API Key | "API Key" | password (masked) | "" (empty) | Non-empty, starts with `sk-ant-` (for Anthropic keys) OR any non-empty string (for compatible APIs) |
| Base URL | "Base URL" | text | "https://api.anthropic.com" | Must be valid URL (starts with `http://` or `https://`) |
| Model | "Model" | text | "claude-sonnet-4-6" | Non-empty |

Note: These three fields map to environment variables `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL` which are the standard Claude Code configuration. They do NOT go into `desktop.json`'s `AppConfig` struct directly -- they are stored as a new `api` section in the config file (see Section 6).

#### 3.2.2 UI Interaction

- Form layout: single column, vertically stacked fields.
- Each field: label above, input below.
- API Key input: type="password" with a toggle button (eye icon) to show/hide.
- Base URL input: pre-filled with default, user can change for compatible API endpoints.
- Model input: pre-filled with default.
- Validation:
  - On blur: validate the field, show inline error below input if invalid.
  - On "Next" click: validate all fields; if any invalid, shake the first invalid field and focus it.
- "Back" button at bottom left to return to Step 1.
- "Next" button at bottom right: enabled only when all fields pass validation.

#### 3.2.3 Data Flow

```
Frontend (wizard.js)
  --> user fills form
  --> clicks "Next"
  --> validates locally
  --> ipc.saveWizardConfig({ apiKey, baseUrl, model })
    --> Tauri invoke "save_wizard_config"
      --> Rust: builds ConfigUpdate with new api fields + wizard_completed=true
        --> ConfigManager::update() + save_to_file()
  <-- success: proceed to Step 3
  <-- error: show error message, enable retry
```

#### 3.2.4 Boundary Conditions

| Condition | Behavior |
|-----------|----------|
| API Key empty | Inline error: "API Key cannot be empty" |
| API Key format invalid (empty after trimming) | Inline error: "API Key cannot be whitespace only" |
| Base URL not starting with http:// or https:// | Inline error: "Base URL must start with http:// or https://" |
| Model empty | Inline error: "Model name cannot be empty" |
| ~/.onecode/ directory does not exist | Rust side `save_to_file()` already creates it via `create_dir_all` |
| Write permission denied on ~/.onecode/desktop.json | Return error from `save_to_file`, frontend shows: "Failed to save configuration. Please check write permission for ~/.onecode/ directory." with retry button |
| Disk full during write | Same as write permission error |
| Config file exists but is corrupted JSON | `load_from_file` already falls back to defaults; wizard writes fresh config |

---

### 3.3 Step 3: Completion

#### 3.3.1 UI Interaction

- Full-screen completion view:
  - Large checkmark icon at center.
  - Title: "Setup Complete"
  - Subtitle: "OneCode is ready. Your terminal is loading..."
  - Optional summary of what was configured (e.g., "API: Anthropic | Model: claude-sonnet-4-6").
- No buttons needed; auto-transition to main interface after 1.5 seconds.
- The wizard overlay fades out, main app content fades in (CSS transition, 300ms).

#### 3.3.2 Data Flow

```
Frontend (wizard.js)
  --> save_wizard_config returns success
  --> mark wizard state as DONE
  --> after 1.5s delay:
    --> hide wizard overlay (#wizardOverlay)
    --> unblock main.js init() continuation
    --> main.js continues: session restore, terminal init, etc.
```

#### 3.3.3 Boundary Conditions

| Condition | Behavior |
|-----------|----------|
| `mark_wizard_completed` write fails (rare, since config was just saved) | Log error in console; still proceed to main interface. Do not block user. |
| User force-quits during the 1.5s transition | On next launch, `is_first_run` checks `wizard_completed` field; if true, skip wizard. If false (write did not complete), wizard re-appears -- acceptable behavior. |
| Main interface fails to initialize after wizard | This is outside wizard scope; existing error handling in main.js applies. |

---

## 4. Rust-Side New Command Interface Definitions

### 4.1 `check_environment`

```rust
#[derive(Clone, Debug, Serialize)]
pub struct DependencyStatus {
    pub name: String,           // "claude" | "node" | "git"
    pub found: bool,           // `which` succeeded
    pub version: Option<String>,// parsed version string (e.g., "22.5.1")
    pub min_version: String,    // minimum required (e.g., "18.0.0")
    pub version_ok: bool,      // version >= min_version (false if version is None and found)
    pub install_hint: String,   // human-readable install instruction
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckEnvironmentResult {
    pub dependencies: Vec<DependencyStatus>,
    pub all_ok: bool,          // true if all found && version_ok
}

#[tauri::command]
pub fn check_environment() -> Result<CheckEnvironmentResult, String>
```

**Implementation notes:**
- Use `std::process::Command` to run `which <name>` and `<name> --version`.
- 5-second timeout per command using `std::process::Command::stdout(Stdio::piped())` + `child.wait_timeout()` (or `tokio::time::timeout` wrapping a `tokio::process::Command`).
- Version parsing: regex extract first semver-like pattern from `--version` output.
- Version comparison: use `semver` crate or simple major.minor.patch numeric comparison.
- This command is stateless; no Tauri State dependency needed.

### 4.2 `is_first_run`

```rust
#[tauri::command]
pub fn is_first_run(cfg_mgr: State<'_, ConfigManager>) -> Result<bool, String>
```

**Implementation notes:**
- Read `ConfigManager` inner RwLock.
- Return `!cfg.wizard_completed`.
- If config file does not exist (defaults loaded), `wizard_completed` defaults to `false`, so `is_first_run` returns `true`.

### 4.3 `save_wizard_config`

```rust
#[derive(Debug, Deserialize)]
pub struct WizardConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

#[tauri::command]
pub fn save_wizard_config(
    config: WizardConfig,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String>
```

**Implementation notes:**
- Build a `ConfigUpdate`-like structure that also includes `api_key`, `base_url`, `model`, and `wizard_completed: true`.
- Actually: extend `ConfigUpdate` with the new fields (see Section 6), then call `ConfigManager::update()` + `save_to_file()`.
- Also writes the API configuration to the appropriate location (see Section 6 for storage design).
- Returns error string if `save_to_file` fails.

### 4.4 Existing `save_config` -- Extension Needed

The existing `save_config` command takes a `ConfigUpdate`. It must be extended to support the new `api_key`, `base_url`, `model`, and `wizard_completed` fields. See Section 6 for `ConfigUpdate` extension.

Alternatively, `save_wizard_config` can be a dedicated command that internally constructs the full update including `wizard_completed: true`, avoiding changes to `save_config`. This approach is preferred for MVP -- it keeps `save_config` stable and adds a wizard-specific command.

---

## 5. Frontend Wizard State Machine

### 5.1 State Definitions

| State | Description | UI Visible |
|-------|-------------|-----------|
| `WELCOME` | Initial splash, auto-transitions to CHECKING | Wizard overlay, splash message "Setting up OneCode..." |
| `CHECKING` | Environment detection in progress | Wizard overlay, Step 1 with spinner icons |
| `ENV_RESULT` | Detection results displayed | Wizard overlay, Step 1 with status icons, "Re-check" + "Next" buttons |
| `CONFIG_FORM` | API configuration form | Wizard overlay, Step 2 form |
| `SAVING` | Config being written | Wizard overlay, Step 2 with spinner on "Next" button |
| `DONE` | Setup complete | Wizard overlay, Step 3 completion view |
| `ERROR` | Unrecoverable error (config write fail) | Wizard overlay, error message + "Retry" button |

### 5.2 State Transitions

```
WELCOME ──(1s auto)──> CHECKING
CHECKING ──(check_environment returns)──> ENV_RESULT
ENV_RESULT ──("Next" click)──> CONFIG_FORM
ENV_RESULT ──("Re-check" click)──> CHECKING
CONFIG_FORM ──("Back" click)──> ENV_RESULT
CONFIG_FORM ──("Next" click, validation pass)──> SAVING
CONFIG_FORM ──("Next" click, validation fail)──> CONFIG_FORM (shake + focus invalid field)
SAVING ──(save success)──> DONE
SAVING ──(save failure)──> ERROR
ERROR ──("Retry" click)──> SAVING
DONE ──(1.5s auto)──> [wizard overlay removed, main app visible]
```

### 5.3 Error Recovery Paths

| From State | Error | Recovery |
|-----------|-------|----------|
| CHECKING | `check_environment` returns error | Transition to ENV_RESULT with all deps shown as "detection failed" (red X); "Re-check" button available |
| SAVING | `save_wizard_config` returns error | Transition to ERROR; show error message; "Retry" button re-enters SAVING with same form data |
| SAVING | `save_wizard_config` timeout (10s) | Transition to ERROR; show "Configuration save timed out. Please check disk space and permissions." |
| Any | Unexpected JS error | Catch in wizard module; show generic error with "Restart Wizard" button that resets to WELCOME |

### 5.4 Wizard Module Architecture

New file: `src/wizard.js`

```js
// wizard.js -- exported interface
export function initWizard()          // Check is_first_run, mount overlay if true
export function destroyWizard()      // Remove overlay DOM, resolve init promise
```

The wizard module returns a Promise from `initWizard()` that resolves when the wizard completes (DONE state). The main.js `init()` flow awaits this promise before proceeding to session restore.

---

## 6. Configuration File Format

### 6.1 Extended `desktop.json` Schema

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "default_cmd": { "type": "string" },
    "default_args": { "type": "array", "items": { "type": "string" } },
    "default_cwd": { "type": "string" },
    "max_terminals": { "type": "integer", "minimum": 1 },
    "ring_buffer_max_mb": { "type": "integer", "minimum": 1 },
    "api_key": { "type": "string", "description": "API Key for AI backend" },
    "base_url": { "type": "string", "description": "API Base URL" },
    "model": { "type": "string", "description": "AI model identifier" },
    "wizard_completed": { "type": "boolean", "description": "Whether setup wizard has been completed" }
  },
  "required": ["default_cmd", "default_args", "default_cwd", "max_terminals", "ring_buffer_max_mb"]
}
```

### 6.2 `wizard_completed` Field Design

- Type: `bool`
- Default (when absent from file): `false`
- Set to `true` by `save_wizard_config` on wizard completion
- Checked by `is_first_run` on every app launch
- Once `true`, never reset to `false` by the app (manual edit possible for testing)

### 6.3 `api_key`, `base_url`, `model` Fields

- Stored directly in `desktop.json` alongside existing fields.
- On app launch, the Rust side sets environment variables `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL` from these fields before spawning PTY processes. This is done in `lib.rs` setup closure or in `pty_spawn` command.
- `api_key` is stored in plaintext in the JSON file. V1 accepts this trade-off for MVP speed. V2 candidate: use macOS Keychain.

### 6.4 Backward Compatibility

When loading `desktop.json`:
- If `wizard_completed` field is missing: treat as `false`.
- If `api_key`, `base_url`, `model` fields are missing: treat as empty strings (no env vars set).
- This is already the behavior of `serde_json::from_reader` with `#[serde(default)]` annotations.

**Rust changes needed in `config.rs`:**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    // existing fields (unchanged)
    pub default_cmd: String,
    pub default_args: Vec<String>,
    pub default_cwd: String,
    pub max_terminals: usize,
    pub ring_buffer_max_mb: usize,

    // new fields with serde defaults
    #[serde(default)]
    pub api_key: String,

    #[serde(default = "default_base_url")]
    pub base_url: String,

    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default)]
    pub wizard_completed: bool,
}

fn default_base_url() -> String { "https://api.anthropic.com".to_string() }
fn default_model() -> String { "claude-sonnet-4-6".to_string() }
```

**`ConfigUpdate` extension:**

```rust
#[derive(Debug, Deserialize)]
pub struct ConfigUpdate {
    // existing fields (unchanged)
    pub default_cmd: Option<String>,
    pub default_args: Option<Vec<String>>,
    pub default_cwd: Option<String>,
    pub max_terminals: Option<usize>,
    pub ring_buffer_max_mb: Option<usize>,

    // new fields
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub wizard_completed: Option<bool>,
}
```

The `apply_to` method already follows the `Some`/`None` pattern; new fields follow the same convention.

---

## 7. Integration Points with Existing Code

### 7.1 `lib.rs` Setup Closure

No structural change needed. The setup closure already:
1. Loads config (which now includes `wizard_completed` via `#[serde(default)]`)
2. Creates `ConfigManager` with the loaded config
3. Manages `ConfigManager` as `State<ConfigManager>`

The `is_first_run` check happens on the frontend side, after Tauri window is ready. The Rust setup closure does NOT need to gate on wizard completion.

### 7.2 `commands.rs` New Commands

Register three new commands in the invoke handler array:

```rust
.invoke_handler(tauri::generate_handler![
    // existing 21 commands...
    commands::check_environment,
    commands::is_first_run,
    commands::save_wizard_config,
])
```

New command functions defined in `commands.rs` (or a new `wizard.rs` module imported into `commands.rs` -- team preference, but a separate `wizard.rs` module is cleaner).

### 7.3 `main.js` init() Flow Modification

Current flow:

```
init() {
  tabManager.init()
  wire cross-refs
  orbital.init()
  palette.init()
  ripple.init()
  ...
  await ipc.sessionRestore()
  await tabManager.restoreOrInit(slots)
  ...
}
```

Modified flow:

```
init() {
  tabManager.init()
  wire cross-refs
  orbital.init()
  palette.init()
  ripple.init()
  ...
  // NEW: wizard gate
  const firstRun = await ipc.isFirstRun()
  if (firstRun) {
    await initWizard()   // blocks until wizard completes
    destroyWizard()      // remove overlay DOM
  }
  // existing flow continues
  await ipc.sessionRestore()
  await tabManager.restoreOrInit(slots)
  ...
}
```

Key design decision: `initWizard()` is called AFTER all controller initialization (tabManager, orbital, palette, ripple, ccView) but BEFORE session restore. This ensures:
- The main app DOM structure exists (wizard overlay mounts on top of `#app`)
- Terminal subsystems are initialized (needed after wizard completes)
- No session restore happens until wizard writes config (API key needed for PTY spawn)

### 7.4 `ipc-bridge.js` New IPC Functions

```js
// Wizard
export async function isFirstRun() {
  return invoke('is_first_run');
}

export async function checkEnvironment() {
  return invoke('check_environment');
}

export async function saveWizardConfig({ apiKey, baseUrl, model }) {
  return invoke('save_wizard_config', {
    config: { api_key: apiKey, base_url: baseUrl, model }
  });
}
```

### 7.5 Wizard Completion to Main Interface Transition

When `initWizard()` resolves:
1. `destroyWizard()` removes the `#wizardOverlay` element from DOM.
2. `main.js` continues with `await ipc.sessionRestore()`.
3. Session restore now has access to the API key that was just written.
4. TabManager creates first terminal with correct environment variables.
5. The transition is seamless -- wizard overlay fades out (CSS opacity transition 300ms), main content was always underneath.

### 7.6 API Key Injection into PTY

When `pty_spawn` is called, the Rust side must inject API environment variables from the config:

In `pty_spawn` (commands.rs), the `env` parameter already accepts `Option<HashMap<String, String>>`. The `MultiPtyManager::spawn` method should merge config-level API env vars into the spawned process environment:

```rust
// In pty_spawn, before spawning:
let mut env_map = env.unwrap_or_default();
let cfg = config.read().map_err(|e| e.to_string())?;
if !cfg.api_key.is_empty() {
    env_map.insert("ANTHROPIC_API_KEY".to_string(), cfg.api_key.clone());
}
if !cfg.base_url.is_empty() {
    env_map.insert("ANTHROPIC_BASE_URL".to_string(), cfg.base_url.clone());
}
if !cfg.model.is_empty() {
    env_map.insert("ANTHROPIC_MODEL".to_string(), cfg.model.clone());
}
```

This ensures that Claude Code CLI spawned in the terminal inherits the API configuration from the wizard.

---

## 8. Boundary Conditions & Error Handling

### 8.1 Complete Error Scenario List

| # | Scenario | Where | Error Type | User-Visible Feedback | Recovery Strategy |
|---|----------|-------|-----------|----------------------|-------------------|
| E1 | `which` command not found (PATH issue) | Rust `check_environment` | Process spawn error | Red X icon + "Not found" + install hint | User installs dependency; click "Re-check" |
| E2 | `--version` hangs | Rust `check_environment` | Timeout (5s) | Red X icon + "Detection timed out" | Click "Re-check" |
| E3 | `--version` output unparseable | Rust `check_environment` | Parse error | Yellow warning + "Version unknown" | Not blocking; user can proceed |
| E4 | Version below minimum | Rust `check_environment` | Logic | Yellow warning + version + hint | User can proceed (non-blocking) |
| E5 | API Key empty on "Next" click | Frontend validation | Validation error | Inline error: "API Key cannot be empty" | User fills field |
| E6 | Base URL invalid format | Frontend validation | Validation error | Inline error: "Base URL must start with http:// or https://" | User corrects URL |
| E7 | Model name empty | Frontend validation | Validation error | Inline error: "Model cannot be empty" | User fills field |
| E8 | `~/.onecode/` directory creation fails | Rust `save_to_file` | `std::io::Error` | "Failed to create ~/.onecode/ directory. Check permissions." + Retry button | User fixes permissions; clicks Retry |
| E9 | `desktop.json` write fails | Rust `save_to_file` | `std::io::Error` | "Failed to save configuration. Check disk space and write permissions for ~/.onecode/" + Retry button | User frees space / fixes permissions; clicks Retry |
| E10 | `desktop.json` write timeout | Rust `save_to_file` | Timeout (10s) | "Configuration save timed out." + Retry button | User clicks Retry |
| E11 | `is_first_run` Tauri invoke fails | Frontend | IPC error | Console error; fall back to NOT showing wizard (fail open) | App proceeds to main interface; user may need to manually configure |
| E12 | `check_environment` Tauri invoke fails | Frontend | IPC error | Transition to ENV_RESULT with all deps as "detection failed" | "Re-check" button available |
| E13 | App crash during wizard | System | Process exit | N/A (app crashed) | On relaunch, `wizard_completed` is false, wizard re-appears |
| E14 | User force-quit during wizard | System | Process exit | N/A | Same as E13 |
| E15 | Corrupted `desktop.json` on launch | Rust `load_from_file` | Parse error | `load_from_file` already falls back to defaults; `wizard_completed` defaults to false | Wizard re-appears; user re-enters config |

### 8.2 Fail-Open Policy

For `is_first_run` check failure (E11), the wizard fails open -- it does NOT show. This is because a broken IPC on first launch likely indicates a deeper issue, and blocking the user behind a broken wizard is worse than letting them into the main interface.

For all wizard-step errors (E1-E10, E12), the wizard stays visible and provides retry actions. The user is never stuck without an actionable next step.

---

## 9. V2 Candidate Features (Explicitly NOT in MVP)

| Feature | Why Deferred |
|---------|-------------|
| Auto-install dependencies (Homebrew, npm) | CEO decision: V1 only detects + prompts |
| Windows / Linux support | CEO decision: MVP is macOS only |
| API Key validation via test request | Network dependency in wizard; adds latency and error surface |
| Proxy configuration step | Not critical for MVP; power users can edit config file |
| Theme / appearance selection | Not relevant to setup core flow |
| Workspace directory selection | Current default `~/.onecode/workspace` is sufficient for V1 |
| Import configuration from existing Claude Code CLI | Nice-to-have; manual re-entry is acceptable for V1 |
| macOS Keychain storage for API Key | Security hardening; V1 uses plaintext JSON |
| Progressive wizard (skip steps, jump to step) | Adds complexity; linear 3-step is simpler |
| Wizard "Skip" button (skip API config) | Risky: user enters main interface with no API key, terminal will not work |
| Accessibility (VoiceOver, keyboard navigation) | Important but not MVP; wizard is mouse-driven with basic keyboard support (Tab to navigate fields) |
| Localization (English UI) | V1 is Chinese-only per existing product direction |

---

## 10. Acceptance Criteria

### 10.1 Per-User-Story Given-When-Then

#### US-1: First Launch Environment Detection

```
Given a fresh macOS machine with OneCode Desktop installed and no ~/.onecode/desktop.json
When the user launches OneCode Desktop for the first time
Then a wizard overlay appears showing Step 1: Environment Detection
And each dependency (Claude Code CLI, Node.js, Git) is listed with a status icon
And the detection completes within 15 seconds total
```

#### US-2: Missing Dependency Clear Prompt

```
Given the wizard is on Step 1 and Node.js is not installed
When the environment detection completes
Then Node.js shows a red X icon
And an install hint "Install: brew install node (or visit nodejs.org)" is displayed below the Node.js row
And the "Next" button is enabled (proceeding is not blocked)
```

#### US-3: API Configuration

```
Given the wizard is on Step 2
When the user enters a valid API Key, Base URL, and Model
Then the "Next" button becomes enabled
And clicking "Next" saves the configuration to ~/.onecode/desktop.json
```

#### US-4: Configuration Validation Feedback

```
Given the wizard is on Step 2
When the user enters an empty API Key and clicks "Next"
Then an inline error "API Key cannot be empty" appears below the API Key field
And the API Key input field is focused and highlighted
And the wizard does NOT proceed to Step 3
```

```
Given the wizard is on Step 2
When the user enters "ftp://example.com" as Base URL and clicks "Next"
Then an inline error "Base URL must start with http:// or https://" appears below the Base URL field
And the wizard does NOT proceed to Step 3
```

#### US-5: Seamless Entry to Main Interface

```
Given the wizard is on Step 3 (DONE)
When 1.5 seconds have elapsed
Then the wizard overlay fades out (300ms transition)
And the main app interface becomes visible with a terminal tab created
And the terminal has ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL, ANTHROPIC_MODEL set in its environment
```

#### US-6: Non-First-Run Skip

```
Given ~/.onecode/desktop.json exists with "wizard_completed": true
When the user launches OneCode Desktop
Then the wizard does NOT appear
And the main interface loads directly with session restore
```

#### US-7: Detection Retry

```
Given the wizard is on Step 1 with a "Not found" status for Git
When the user installs Git externally and clicks "Re-check"
Then the environment detection re-runs
And Git status updates to green check icon
```

#### US-8: Config Save Failure Recovery

```
Given the wizard is on Step 2 and the user clicks "Next"
And ~/.onecode/ directory is read-only (write permission denied)
When save_wizard_config fails
Then an error message "Failed to save configuration. Check disk space and write permissions for ~/.onecode/" is displayed
And a "Retry" button appears
When the user fixes permissions and clicks "Retry"
Then the save is attempted again
And if successful, the wizard proceeds to Step 3
```

### 10.2 Overall MVP Delivery Acceptance Criteria

| # | Criterion | Pass Condition |
|---|-----------|---------------|
| AC1 | Wizard appears on first run | Fresh install (no `~/.onecode/desktop.json`) launches wizard automatically |
| AC2 | Wizard does not appear on subsequent runs | After wizard completion, relaunching app goes directly to main interface |
| AC3 | Three-step wizard completes in under 90 seconds | Measured from wizard appearance to Step 3 display, with normal user input speed |
| AC4 | Environment detection is accurate | Correctly reports installed/missing status for Claude Code CLI, Node.js, Git on macOS |
| AC5 | API configuration is persisted | After wizard, `~/.onecode/desktop.json` contains `api_key`, `base_url`, `model`, `wizard_completed: true` |
| AC6 | API env vars are available in terminal | First PTY spawn has `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL` in environment |
| AC7 | Wizard is non-blocking for missing deps | User can proceed past Step 1 even if dependencies are missing |
| AC8 | Validation prevents invalid config | Invalid API Key / Base URL / Model are caught before save attempt |
| AC9 | Error states have recovery paths | Every error in the wizard has a visible "Retry" or "Re-check" action |
| AC10 | Backward compatible | Existing `desktop.json` files without new fields load correctly with defaults |
| AC11 | macOS only | Wizard tested and verified on macOS 13+ (Ventura and later) |
| AC12 | No new runtime dependencies | Only `semver` crate added (if needed for version comparison); no new npm packages |
| AC13 | Existing tests pass | `npm test` and `cargo test` pass with wizard module added |
| AC14 | No regression in main interface | After wizard completion, main interface behaves identically to pre-wizard builds |

---

## Appendix A: File Change Summary

| File | Change Type | Description |
|------|-----------|-------------|
| `src-tauri/src/config.rs` | Modify | Add `api_key`, `base_url`, `model`, `wizard_completed` fields to `AppConfig` and `ConfigUpdate` |
| `src-tauri/src/commands.rs` | Modify | Add `check_environment`, `is_first_run`, `save_wizard_config` commands |
| `src-tauri/src/lib.rs` | Modify | Register 3 new commands in invoke handler |
| `src-tauri/src/pty/mod.rs` | Modify | Merge API env vars from config into PTY spawn environment |
| `src-tauri/Cargo.toml` | Modify | Add `semver` dependency (if version comparison needed) |
| `src/wizard.js` | **New** | Wizard state machine, UI rendering, transition logic |
| `src/wizard.css` | **New** | Wizard overlay styles (dark theme, consistent with existing app) |
| `src/ipc-bridge.js` | Modify | Add `isFirstRun`, `checkEnvironment`, `saveWizardConfig` functions |
| `src/main.js` | Modify | Add wizard gate in init() before session restore |
| `src/index.html` | Modify | Add `#wizardOverlay` container div |

## Appendix B: Wizard Overlay HTML Structure

```html
<!-- Added inside <body>, before scripts -->
<div class="wizard-overlay" id="wizardOverlay">
  <div class="wizard-container">
    <div class="wizard-header">
      <div class="wizard-logo">OneCode</div>
      <div class="wizard-stepper">
        <span class="step" data-step="1">1. Environment</span>
        <span class="step" data-step="2">2. API Config</span>
        <span class="step" data-step="3">3. Complete</span>
      </div>
    </div>
    <div class="wizard-body" id="wizardBody">
      <!-- Step content rendered dynamically by wizard.js -->
    </div>
    <div class="wizard-footer">
      <!-- Navigation buttons rendered dynamically -->
    </div>
  </div>
</div>
```

## Appendix C: Sequence Diagram (First Launch)

```
User            main.js           wizard.js          ipc-bridge          Rust
  |                |                  |                  |                |
  |---launch----->|                  |                  |                |
  |               |--init()--------->|                  |                |
  |               |                  |--isFirstRun()--->|                |
  |               |                  |                  |--is_first_run->|
  |               |                  |                  |<-true----------|
  |               |                  |<-true-----------|                |
  |               |                  |--mount overlay-->|                |
  |               |                  |--WELCOME 1s----->|                |
  |               |                  |--CHECKING------->|                |
  |               |                  |                  |--check_env---->|
  |               |                  |                  |<-result--------|
  |               |                  |<-ENV_RESULT------|                |
  |<--wizard UI---|                  |                  |                |
  |               |                  |                  |                |
  |---click Next--|                  |                  |                |
  |               |                  |--CONFIG_FORM---->|                |
  |<--API form---|                  |                  |                |
  |               |                  |                  |                |
  |---fill form---|                  |                  |                |
  |---click Next--|                  |                  |                |
  |               |                  |--validate OK---->|                |
  |               |                  |--SAVING--------->|                |
  |               |                  |                  |--save_wiz_cfg->|
  |               |                  |                  |<-ok------------|
  |               |                  |<-DONE------------|                |
  |               |                  |--1.5s delay----->|                |
  |               |                  |--destroy overlay->|               |
  |<--main UI----|                  |                  |                |
  |               |--sessionRestore()|                  |                |
  |               |--restoreOrInit() |                  |                |
  |<--terminal---|                  |                  |                |
```
