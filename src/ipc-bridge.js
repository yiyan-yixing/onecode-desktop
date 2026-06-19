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
