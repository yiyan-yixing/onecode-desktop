// 忠实移植 pty/slot.rs 的 RingBuffer（用于逻辑验证）。
// 来源：onecode-desktop/src-tauri/src/pty/slot.rs:49-125
// 注：用 number[]（字节）模拟 Vec<u8>；trim 完全照搬 Rust 的盲字节 drain
//     （即不做 UTF-8 边界对齐——这是 Rust 实现的真实行为，作为已知缺陷保留）。

const DEFAULT_MAX = 10 * 1024 * 1024; // RING_BUFFER_MAX_BYTES

export class RingBuffer {
  constructor(maxSize = DEFAULT_MAX) {
    this.chunks = [];
    this.totalLen = 0;
    this.maxSize = maxSize;
    this.dirty = true;
    this.cachedReplay = null;
  }

  push(data) {
    if (data.length === 0) return; // :69
    this.totalLen += data.length;
    this.chunks.push([...data]);
    this.dirty = true;
    // 超 1.1x 触发裁剪（:76，与 pty.js 一致）
    if (this.totalLen > this.maxSize + Math.floor(this.maxSize / 10)) {
      this._trim();
    }
  }

  // 移植 trim（:83-96）：拼接全部 → drain 头部超出 → 保留尾部 maxSize 字节。
  _trim() {
    if (this.totalLen <= this.maxSize) return;
    const flat = [];
    for (const c of this.chunks) flat.push(...c);
    const dropLen = Math.max(0, flat.length - this.maxSize);
    const out = flat.slice(dropLen); // 等价 drain(0..drop_len)
    this.chunks = [out];
    this.totalLen = this.chunks[0].length;
    this.dirty = true;
  }

  // 移植 replay（:99-112），带 dirty 缓存。
  replay() {
    if (!this.dirty && this.cachedReplay) return [...this.cachedReplay];
    const out = [];
    for (const c of this.chunks) out.push(...c);
    this.cachedReplay = [...out];
    this.dirty = false;
    return out;
  }

  clear() {
    this.chunks = [];
    this.totalLen = 0;
    this.cachedReplay = null;
    this.dirty = true;
  }

  len() {
    return this.totalLen;
  }
}

// 辅助：number[] → string（仅 ASCII 用例）。
export const bytesToStr = (bs) => bs.map((b) => String.fromCharCode(b)).join('');
