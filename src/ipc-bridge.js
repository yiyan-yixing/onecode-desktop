// Tauri IPC 封装（架构修订 §2：Channel<Vec<u8>> 流式二进制，无 base64）。
// withGlobalTauri: true → window.__TAURI__.core / event 全局可用，无需 npm 包。

const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/** 规范化 Channel 收到的字节为 Uint8Array（兼容 number[] 与 Uint8Array 两种传输）。 */
function toBytes(chunk) {
  return chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
}

/**
 * 创建终端。onData 回调接收 PTY 输出字节（已规范化）。
 * 内部 new Channel()，onmessage → onData；Channel 对象作为 dataChannel 传入 pty_spawn。
 */
export async function ptySpawn({ cmd, args, cwd, env, label, onData }) {
  const channel = new Channel();
  channel.onmessage = (chunk) => onData(toBytes(chunk));
  return invoke('pty_spawn', { cmd, args, cwd, env, label, dataChannel: channel });
}

/** 手动重启：前端传入新的 onData 回调（旧 Channel 已失效）。 */
export async function ptyRestart(id, onData) {
  const channel = new Channel();
  channel.onmessage = (chunk) => onData(toBytes(chunk));
  return invoke('pty_restart', { id, dataChannel: channel });
}

export async function ptyKill(id) {
  return invoke('pty_kill', { id });
}

/** data 为 term.onData 给的 string（UTF-8），转字节后传 Rust Vec<u8>。 */
export async function ptyWrite(id, data) {
  const bytes = Array.from(new TextEncoder().encode(data));
  return invoke('pty_write', { id, data: bytes });
}

export async function ptyResize(id, cols, rows) {
  return invoke('pty_resize', { id, cols, rows });
}

export async function ptyList() {
  return invoke('pty_list');
}

export async function ptyRename(id, label) {
  return invoke('pty_rename', { id, label });
}

/** Tab 切换时拉取 ring buffer 回放（返回 number[]，前端转 Uint8Array）。 */
export async function ptyReplay(id) {
  const arr = await invoke('pty_replay', { id });
  return toBytes(arr);
}

/** 进程退出事件（低频，Rust 用 app.emit，不走 Channel）。 */
export function onPtyExit(id, callback) {
  return listen(`pty:exit:${id}`, (event) => callback(event.payload));
}

// ── P1：会话持久化 ──────────────────────────────────────────────────

/** 以 Rust 侧 slot 为事实源快照并落库（无需前端回传配置）。返回持久化条数。 */
export async function sessionPersist() {
  return invoke('session_persist');
}

/** 启动恢复：返回上次保存的终端配置列表。 */
export async function sessionRestore() {
  return invoke('session_restore');
}

/** 显式保存（前端构造的配置，兜底用）。 */
export async function sessionSave(slots) {
  return invoke('session_save', { slots });
}

// ── P1：CC Status（skills/hooks/plugins/tasks/agents） ──────────────

/** 读取 CC Status。project_dir 为当前活跃终端 cwd（读 {cwd}/.claude）。 */
export async function ccStatus(projectDir) {
  return invoke('cc_status', { projectDir });
}

export async function ccStatusInvalidate() {
  return invoke('cc_status_invalidate');
}

// ── P1：健康检测 ────────────────────────────────────────────────────

export async function healthCheck() {
  return invoke('health_check');
}

/** 后台健康告警（Rust 每 5s emit，仅有告警项时推送）。 */
export function onHealthReport(callback) {
  return listen('health:report', (event) => callback(event.payload));
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
