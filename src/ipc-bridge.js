// Tauri IPC 封装（架构修订 §2：Channel<Vec<u8>> 流式二进制，无 base64）。
// withGlobalTauri: true → window.__TAURI__.core / event 全局可用，无需 npm 包。

const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/** 规范化 Channel 收到的字节为 Uint8Array（兼容 number[] 与 Uint8Array 两种传输）。 */
function toBytes(chunk) {
  return chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
}

// ── P1-13: Backend crash detection ──────────────────────────────────
// Track consecutive IPC failures. After 3, dispatch 'ipc-disconnected'.
// On 2 consecutive successes after disconnect, dispatch 'ipc-reconnected'.
// Hysteresis prevents banner flicker on a flaky backend.

const FAIL_THRESHOLD = 3;
const RECOVER_THRESHOLD = 2;
let _consecutiveFailures = 0;
let _consecutiveRecoveries = 0;

/**
 * Safe wrapper around Tauri invoke(). Counts consecutive failures and
 * dispatches custom DOM events when the backend appears disconnected
 * or recovers. All module-level invoke() calls should go through this.
 */
async function safeInvoke(cmd, args) {
  try {
    const result = await invoke(cmd, args);
    if (_consecutiveFailures >= FAIL_THRESHOLD) {
      _consecutiveRecoveries++;
      if (_consecutiveRecoveries >= RECOVER_THRESHOLD) {
        window.dispatchEvent(new CustomEvent('ipc-reconnected'));
        _consecutiveRecoveries = 0;
      }
    } else {
      _consecutiveRecoveries = 0;
    }
    _consecutiveFailures = 0;
    return result;
  } catch (err) {
    _consecutiveRecoveries = 0;
    _consecutiveFailures++;
    if (_consecutiveFailures === FAIL_THRESHOLD) {
      window.dispatchEvent(new CustomEvent('ipc-disconnected'));
    }
    throw err;
  }
}

/**
 * 创建终端。onData 回调接收 PTY 输出字节（已规范化）。
 * 内部 new Channel()，onmessage → onData；Channel 对象作为 dataChannel 传入 pty_spawn。
 * projectId: 可选，关联的项目 ID。
 * backend: 可选，后端 ID（如 "claude-code"、"opencode"）。
 */
export async function ptySpawn({ cmd, args, cwd, env, label, projectId, backend, cols, rows, onData }) {
  const channel = new Channel();
  channel.onmessage = (chunk) => onData(toBytes(chunk));
  return safeInvoke('pty_spawn', { cmd, args, cwd, env, label, projectId, backend: backend || null, cols, rows, dataChannel: channel });
}

/** 手动重启：前端传入新的 onData 回调（旧 Channel 已失效）+ 终端当前尺寸。 */
export async function ptyRestart(id, onData, cols, rows) {
  const channel = new Channel();
  channel.onmessage = (chunk) => onData(toBytes(chunk));
  return safeInvoke('pty_restart', { id, cols, rows, dataChannel: channel });
}

export async function ptyKill(id) {
  return safeInvoke('pty_kill', { id });
}

/** data 为 term.onData 给的 string（UTF-8），转字节后传 Rust Vec<u8>。 */
export async function ptyWrite(id, data) {
  const bytes = Array.from(new TextEncoder().encode(data));
  return safeInvoke('pty_write', { id, data: bytes });
}

export async function ptyResize(id, cols, rows) {
  return safeInvoke('pty_resize', { id, cols, rows });
}

export async function ptyList() {
  return safeInvoke('pty_list');
}

export async function ptyRename(id, label) {
  return safeInvoke('pty_rename', { id, label });
}

/** Tab 切换时拉取 ring buffer 回放（返回 number[]，前端转 Uint8Array）。 */
export async function ptyReplay(id) {
  const arr = await safeInvoke('pty_replay', { id });
  return toBytes(arr);
}

/** 进程退出事件（低频，Rust 用 app.emit，不走 Channel）。 */
export function onPtyExit(id, callback) {
  return listen(`pty:exit:${id}`, (event) => callback(event.payload));
}

/** 进程自动重启成功事件（Rust auto-restart 成功后 emit，payload 为新 pid）。 */
export function onPtyRestart(id, callback) {
  return listen(`pty:restart:${id}`, (event) => callback(event.payload));
}

// ── P1：会话持久化 ──────────────────────────────────────────────────

/** 以 Rust 侧 slot 为事实源快照并落库（无需前端回传配置）。返回持久化条数。 */
export async function sessionPersist() {
  return safeInvoke('session_persist');
}

/** 启动恢复：返回上次保存的终端配置列表。 */
export async function sessionRestore() {
  return safeInvoke('session_restore');
}

/** 标记指定终端为当前活跃（更新 last_active_at 时间戳）。 */
export async function ptySetActive(id) {
  return safeInvoke('pty_set_active', { id });
}

/** 显式保存（前端构造的配置，兜底用）。 */
export async function sessionSave(slots) {
  return safeInvoke('session_save', { slots });
}

// ── P1：CC Status（skills/hooks/plugins/tasks/agents） ──────────────

/** 读取 CC Status。project_dir 为当前活跃终端 cwd（读 {cwd}/.claude）。 */
export async function ccStatus(projectDir) {
  return safeInvoke('cc_status', { projectDir });
}

export async function ccStatusInvalidate() {
  return safeInvoke('cc_status_invalidate');
}

// ── CC Sessions ──────────────────────────────────────────────────

/** 列出 Claude Code 会话。project_dir 可选过滤（null = 所有项目）。 */
export async function ccSessionsList(projectDir) {
  return safeInvoke('cc_sessions_list', { projectDir: projectDir || null });
}

// ── P1：健康检测 ────────────────────────────────────────────────────

export async function healthCheck() {
  return safeInvoke('health_check');
}

/** 后台健康告警（Rust 每 5s emit，仅有告警项时推送）。 */
export function onHealthReport(callback) {
  return listen('health:report', (event) => callback(event.payload));
}

// ── 配置管理 ──────────────────────────────────────────────────────────

/** 保存应用配置到 ~/.onecode/desktop.json */
export async function saveConfig(config) {
  return safeInvoke('save_config', { config });
}

/** 从 ~/.onecode/desktop.json 加载配置 */
export async function loadConfig() {
  return safeInvoke('load_config');
}

// ── 项目管理 ──────────────────────────────────────────────────────────

/** 保存项目元数据到 ~/.onecode/projects/<name>.json */
export async function saveProject({ name, dir, description, backend }) {
  return safeInvoke('save_project', { project: { name, dir, description, backend: backend || null } });
}

/** 列出所有已保存的项目 */
export async function listProjects() {
  return safeInvoke('list_projects');
}

/** 删除项目元数据（不删除目录和关联终端） */
export async function deleteProject(name) {
  return safeInvoke('delete_project', { name });
}

// ── 托盘 / 应用生命周期事件 ─────────────────────────────────────────

/** 托盘「新建终端」点击。 */
export function onTrayNewTerminal(callback) {
  return listen('tray:new-terminal', () => callback());
}

/** 应用即将退出（托盘「退出」触发）——前端在此做最终会话保存。 */
export function onAppBeforeQuit(callback) {
  return listen('app:before-quit', () => callback());
}

// ── 系统信息 ──────────────────────────────────────────────────────────

/** 获取当前用户的 home 目录（跨平台，Rust 侧读取 HOME/USERPROFILE）。 */
export async function getHomeDir() {
  return safeInvoke('get_home_dir');
}

// ── Backend 管理 ──────────────────────────────────────────────────────

/** 列出所有可用后端及其安装状态。返回 BackendInfo[]。 */
export async function listBackends() {
  return safeInvoke('list_backends');
}

// ── Setup Wizard ──────────────────────────────────────────────────

/** 是否首次启动（wizard 未完成） */
export async function isFirstRun() {
  return safeInvoke('is_first_run');
}

/** 检测环境依赖（Claude Code / Node.js / Git） */
export async function checkEnvironment() {
  return safeInvoke('check_environment');
}

/** 保存 Wizard 配置（API Key / Base URL / Model / Backend + 标记 wizard_completed） */
export async function saveWizardConfig({ apiKey, baseUrl, model, backend }) {
  return safeInvoke('save_wizard_config', {
    config: { api_key: apiKey, base_url: baseUrl, model, backend: backend || 'claude-code' }
  });
}

// ── 文件浏览器 ──────────────────────────────────────────────

/** 列出目录内容（返回 {path, entries[]}）。 */
export async function fsListDir(path) {
  return safeInvoke('fs_list_dir', { path });
}

/** 读取文件内容用于预览（返回 {name, path, fileType, text, dataBase64, size}）。 */
export async function fsReadFile(path) {
  return safeInvoke('fs_read_file', { path });
}
