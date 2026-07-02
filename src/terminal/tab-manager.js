// 多终端管理器（PTY 生命周期 + 终端实例管理）。
//
// 解耦 SidebarManager → OrbitalController。
// 使用 visibility/opacity 替代 display:none 实现终端切换。
// 所有终端必须属于某个项目（projectId 必填），无 chat 概念。

import { createTerminal } from './terminal.js';
import { ScrollThumb } from './scroll-thumb.js';
import { initImeFix } from './ime-fix.js';
import { MentionController } from './mention.js';
import * as ipc from '../ipc-bridge.js';

const AUTOSAVE_DEBOUNCE_MS = 350;
const INTERACTIVE_BYTES = 256; // 交互式小数据阈值：<256B 直接写入不等 rAF

// Cached backend install hints (populated from listBackends IPC)
// Avoids hardcoding which would drift from server-side BackendInfo.install_hint.

export class TabManager {
  constructor() {
    this.tabs = new Map();
    this.order = [];
    this.activeId = null;
    this.termContainer = null;
    this.mentionPop = null;
    this.agentProvider = null;
    this.onChange = null;
    this.orbital = null;   // OrbitalController
    this.ripple = null;     // RippleController
    this._saveTimer = null;
    this._backendHints = new Map(); // backend id → install_hint
  }

  init() {
    this.termContainer = document.getElementById('termViewport');
    this.mentionPop = document.getElementById('mentionPortal');

    // ── 窗口/app 焦点恢复：切回 app 时自动重新聚焦活跃终端 ──
    // 解决 Cmd+Tab 切走再切回后键盘输入无响应的问题
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible' && this.activeId) {
        const st = this.tabs.get(this.activeId);
        if (st && !st.isError) {
          requestAnimationFrame(() => { try { st.term.focus(); } catch (_) {} });
        }
      }
    });
    // macOS focus 事件（窗口级别失焦/聚焦，visibilitychange 可能不触发）
    window.addEventListener('focus', () => {
      if (this.activeId) {
        const st = this.tabs.get(this.activeId);
        if (st && !st.isError) {
          requestAnimationFrame(() => { try { st.term.focus(); } catch (_) {} });
        }
      }
    });
  }

  async restoreOrInit(slots) {
    // 恢复所有 project 终端（跳过无 project_id 的旧 chat slot）。
    // 按 last_active_at 排序，自动切换到上次最后操作的项目。
    // ★ 去重：同一 project_id 只恢复一个终端（取 last_active_at 最新的），
    // 防止 session 累积导致启动时创建大量重复终端。
    if (slots && slots.length > 0) {
      const projectSlots = slots.filter(s => s.project_id);
      // ★ 按 project_id 去重：每个项目只保留最新活跃的终端
      const dedupMap = new Map();
      for (const slot of projectSlots) {
        const existing = dedupMap.get(slot.project_id);
        if (!existing || (slot.last_active_at && (!existing.last_active_at || slot.last_active_at > existing.last_active_at))) {
          dedupMap.set(slot.project_id, slot);
        }
      }
      const uniqueSlots = [...dedupMap.values()];
      for (const slot of uniqueSlots) {
        await this.createTab({
          label: slot.label,
          cmd: slot.cmd,
          args: slot.args,
          cwd: slot.cwd,
          env: slot.env,
          backend: slot.backend || null,
          projectId: slot.project_id,
        });
      }
      // 按 last_active_at 找到上次活跃的终端并切换
      const lastActive = projectSlots
        .filter(s => s.last_active_at)
        .sort((a, b) => b.last_active_at.localeCompare(a.last_active_at))[0];
      if (lastActive) {
        for (const [tid, st] of this.tabs) {
          if (st.projectId === lastActive.project_id && !st.isError) {
            this.switchTo(tid);
            break;
          }
        }
      } else if (this.order.length > 0) {
        this.switchTo(this.order[0]);
      }
    }
    // 无 slots（全新安装或所有终端已关闭）→ 空状态，用户通过侧栏或空球创建项目
  }

  async createTab(opts = {}) {
    // 所有终端必须属于某个项目。无 projectId 时打开新建项目对话框。
    if (!opts.projectId) {
      if (this.orbital) {
        this.orbital._newProject();
      }
      return null;
    }

    const label = opts.label || opts.projectId || 'Terminal';
    const backend = opts.backend || null;

    const termEl = document.createElement('div');
    termEl.className = 'term-instance';
    this.termContainer.appendChild(termEl);

    const { term, fitAddon } = createTerminal(termEl);
    const scrollThumb = new ScrollThumb(term, termEl);
    initImeFix(term, termEl);

    // ── PTY 输出回调：活跃终端 rAF 合并写入，非活跃终端缓冲 ──
    // term.write() 每次调用都触发 ANSI 解析 + canvas 重绘。
    // 在 33ms flush 间隔内可能多次 Channel.onmessage，合并到下一帧写入
    // 可大幅减少重绘次数（从 N 次/帧 → 1 次/帧）。
    // ★ 交互式响应（用户输入后 100ms 内的小数据）直接写入 — 低延迟。
    // ★ 非交互式输出（Claude Code spinner 等）走 rAF — 释放主线程处理按键。
    let _inactiveBuffer = [];
    let _writeBatch = [];
    let _writeRaf = null;
    const onData = (bytes) => {
      if (this.activeId === id) {
        _writeBatch.push(bytes);
        const byteLen = bytes.length || bytes.byteLength || 0;
        // ★ 快速路径：仅在用户刚输入（<100ms）且数据量小时直接写入
        // 根因：Claude Code spinner 每 50ms 输出 ~90B，若全部直写 term.write()，
        //   主线程持续被 ANSI 解析+重绘占用 → keydown 事件延迟 → 退格卡顿。
        //   限制直写仅对用户输入的响应，其他输出走 rAF 合并，释放主线程。
        const isInputResponse = term._lastKeyTime && (performance.now() - term._lastKeyTime) < 100;
        if (_writeBatch.length === 1 && isInputResponse && byteLen < INTERACTIVE_BYTES) {
          term.write(bytes);
          _writeBatch = [];
          return;
        }
        if (!_writeRaf) {
          _writeRaf = requestAnimationFrame(() => {
            // 合并所有缓冲块为一次 write — 减少 ANSI 解析 + 重绘次数
            if (_writeBatch.length === 1) {
              term.write(_writeBatch[0]);
            } else if (_writeBatch.length > 1) {
              let total = 0;
              for (const b of _writeBatch) total += b.length || b.byteLength || 0;
              const merged = new Uint8Array(total);
              let offset = 0;
              for (const b of _writeBatch) {
                const chunk = (b instanceof Uint8Array) ? b : new Uint8Array(b);
                merged.set(chunk, offset);
                offset += chunk.length;
              }
              term.write(merged);
            }
            _writeBatch = [];
            _writeRaf = null;
          });
        }
      } else {
        _inactiveBuffer.push(bytes);
        if (_inactiveBuffer.length > 512) {
          _inactiveBuffer = _inactiveBuffer.slice(-256);
        }
      }
    };

    let id;
    let pid = null;
    try {
      const result = await ipc.ptySpawn({
        cmd: opts.cmd,
        args: opts.args,
        cwd: opts.cwd,
        env: opts.env,
        label,
        projectId: opts.projectId || null,
        backend,
        cols: term.cols,
        rows: term.rows,
        onData,
      });
      id = result.id;
      pid = result.pid || null;
      // IME 直接字符插入的发送回调（ime-fix.js Fix 3 需要）
      term._imeSendFn = (data) => ipc.ptyWrite(id, data);
    } catch (err) {
      // Display clear error in terminal for failed spawn
      const errMsg = String(err);
      term.write('\x1b[31m✗ 终端启动失败\x1b[0m\r\n\r\n');
      term.write(`  错误: ${errMsg}\r\n\r\n`);
      if (errMsg.includes('最大终端数')) {
        term.write('  \x1b[33m终端数已达上限，请先关闭不需要的标签页再创建新终端\x1b[0m\r\n\r\n');
      } else {
        term.write('  \x1b[2m可能原因:\x1b[0m\r\n');
        term.write('  • 命令不在 PATH 中（检查 ~/.local/bin）\r\n');
        if (backend && this._backendHints.has(backend)) {
          term.write(`  • 后端 ${backend} 未安装: ${this._backendHints.get(backend)}\r\n`);
        } else {
          term.write('  • 工作目录不存在\r\n');
        }
        term.write('\r\n');
      }
      term.write('  \x1b[36m点击左侧光球状态点可重新启动，或关闭此标签页\x1b[0m\r\n');
      // P1-21: Use random alphanumeric suffix instead of Date.now() to avoid collisions
      id = 'err-' + Math.random().toString(36).slice(2, 8);
      termEl.dataset.id = id;
      this.orbital.addOrb(id, label, 'exited', opts.cwd);
      this.switchTo(id);
      // Mark as error tab: no IPC callbacks registered, closeTab will skip ptyKill
      this.tabs.set(id, {
        id, label, cmd: opts.cmd, args: opts.args, cwd: opts.cwd, env: opts.env,
        projectId: opts.projectId || null, backend,
        term, fitAddon, scrollThumb, termEl,
        mention: null, unlistenExit: null, status: 'exited', isError: true,
      });
      this.order.push(id);
      this._scheduleSave();
      this._notifyChange();
      return id;
    }
    termEl.dataset.id = id;

    const unlistenExit = ipc.onPtyExit(id, (code) => {
      this.updateStatus(id, 'exited', code);
    });

    // ★ 监听 Rust 自动重启成功事件：前端状态同步 exited → running
    const unlistenRestart = ipc.onPtyRestart(id, (pid) => {
      const st = this.tabs.get(id);
      if (st) {
        st.status = 'running';
        st.isError = false;
        if (pid) st.pid = pid;
      }
      this.updateStatus(id, 'running');
    });

    // ★ 统一 PTY 写入去重：onData 和 _imeSendFn 都经过此函数
    // 根因: WKWebView 中 keydown 的 preventDefault() 无法阻止字符插入 textarea，
    // xterm 的 _inputEvent handler 会再次 triggerDataEvent 发送相同字符 → 重复。
    // _imeSendFn 路径 (IME Shift+符号等) 和 onData 路径 (xterm keydown/_inputEvent)
    // 都汇合到此去重。只在不同来源(imeSendFn vs onData) 200ms 内发送相同数据时去重，
    // 同一来源的连续相同输入（如按两次 l）不去重。
    // ★ 修复 7: 窗口从 30ms 扩大到 200ms，覆盖 Caps Lock 场景下
    // compositionend → input → keydown 的完整事件链（~100ms）。
    let _lastPtyData = '';
    let _lastPtyTime = 0;
    let _lastPtySource = '';
    const ptyWriteDedup = (data, source) => {
      const now = Date.now();
      const isDup = data === _lastPtyData && (now - _lastPtyTime) < 200 && source !== _lastPtySource;
      if (isDup) {
        return; // 不同来源 200ms 内重复 → 丢弃
      }
      _lastPtyData = data;
      _lastPtyTime = now;
      _lastPtySource = source;
      // 记录按键时刻到 term 实例，供 onData 端到端计算
      term._lastKeyTime = performance.now();
      ipc.ptyWrite(id, data).catch((e) => {
        console.warn(`[ptyWrite] failed for ${id}:`, e);
      });
    };

    // IME 直接字符插入的发送回调（ime-fix.js Fix 3 需要）→ 走统一去重
    term._imeSendFn = (data) => ptyWriteDedup(data, 'ime');

    const dataDisposable = term.onData((data) => {
      // ★ 修复 7: 先过滤再去重！
      // 根因：IME 提交的拼音 "wo men" 过滤后变成 "women"，但 _xtermSentData
      //   去重检查在过滤前执行，比较 raw "wo men" vs filtered "women" → 不匹配 →
      //   去重失败 → 双发。修复：先应用拼音空格过滤，再做 _xtermSentData 比较。
      // ★ 修复 5: onData 路径过滤 IME 拼音空格（不限 recentlyComposed）
      // 安全性：正常英文逐字符发送，不会一次发送 "wo men" 这样的多词块。
      if (/^[a-zA-Z0-9]+( +[a-zA-Z0-9]+)+$/.test(data)) {
        data = data.replace(/ +/g, '');
      }

      // ★ onData 内部去重：keydown/_inputEvent/_handleAnyTextareaChanges 双发
      // 现在比较的是过滤后的数据，确保 "women" === "women" → 正确去重
      //
      // ★★★ 关键修复：去重窗口从 200ms 缩短为 30ms ★★★
      // 根因：200ms 窗口会吞噬快速重复按键（退格、方向键等）。
      //   用户按住退格时，xterm 每 33-50ms 触发一次 onData("\x7f")，
      //   200ms 窗口内所有相同数据都被当作"xterm 内部重复"丢弃 → 退格极慢。
      //   xterm 内部双发（keydown + input 同 tick）间隔 <5ms，30ms 足够覆盖。
      //
      // IME 兼容：compositionend 后 300ms 内仍用 200ms 窗口。
      //   Caps Lock 场景下 compositionend → input → keydown 事件链跨度 ~100ms，
      //   需要更长窗口去重。ptyWriteDedup 的 source 检查作为第二层防线。
      const now = Date.now();
      const dedupWindow = (term._compositionEndedAt && (now - term._compositionEndedAt) < 300) ? 200 : 30;
      const isXtermDup = data === term._xtermSentData && (now - term._xtermSentTime) < dedupWindow;
      if (isXtermDup) {
        return; // xterm 内部重复 → 丢弃
      }

      // ★ 修复 4: 记录过滤后的数据，供 ime-fix.js Fix 3 去重
      term._xtermSentData = data;
      term._xtermSentTime = Date.now();

      // ★ 统一去重写入 PTY
      ptyWriteDedup(data, 'onData');
    });
    const resizeDisposable = term.onResize(({ cols, rows }) => ipc.ptyResize(id, cols, rows));

    const mention = new MentionController({
      term,
      termEl,
      popEl: this.mentionPop,
      sendInput: (s) => ipc.ptyWrite(id, s).catch(() => {}),
      getAgents: () => (this.agentProvider ? this.agentProvider() : []),
    });
    // 让 ime-fix.js paste handler 能通知 mention 禁止触发
    term._mentionController = mention;

    // Flush inactive buffer: 切回此终端时一次性合并刷入
    const _flushInactiveBuffer = () => {
      if (_inactiveBuffer.length === 0) return;
      if (_inactiveBuffer.length === 1) {
        term.write(_inactiveBuffer[0]);
      } else {
        let total = 0;
        for (const b of _inactiveBuffer) total += b.length || b.byteLength || 0;
        const merged = new Uint8Array(total);
        let offset = 0;
        for (const b of _inactiveBuffer) {
          const chunk = (b instanceof Uint8Array) ? b : new Uint8Array(b);
          merged.set(chunk, offset);
          offset += chunk.length;
        }
        term.write(merged);
      }
      _inactiveBuffer = [];
    };

    const state = {
      id, label, cmd: opts.cmd, args: opts.args, cwd: opts.cwd, env: opts.env,
      projectId: opts.projectId || null, backend,
      pid,
      term, fitAddon, scrollThumb, termEl, mention, unlistenExit, unlistenRestart,
      dataDisposable, resizeDisposable,
      status: 'running',
      _flushInactiveBuffer,
      _inactiveBuffer, _writeBatch, _writeRaf, onData,
    };
    this.tabs.set(id, state);
    this.order.push(id);

    this.orbital.addOrb(id, label, 'running', opts.cwd);
    this.switchTo(id);
    this._scheduleSave();
    this._notifyChange();
    return id;
  }

  switchTo(id) {
    for (const [tid, st] of this.tabs) {
      if (tid === id) {
        st.termEl.classList.add('active');
      } else {
        st.termEl.classList.remove('active');
        if (st.mention) st.mention.hide();
      }
    }
    this.orbital.setActive(id);
    const st = this.tabs.get(id);
    if (st) {
      // 保存当前滚动位置，防止 fit+flushInactiveBuffer 导致位置跳到底部
      const savedViewportY = st.term.buffer.active.viewportY;
      // 刷入非活跃期间缓冲的 PTY 输出（避免切换后内容缺失）
      if (st._flushInactiveBuffer) st._flushInactiveBuffer();
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          try { st.fitAddon.fit(); } catch (_) {}
          // 恢复滚动位置（fit 可能因 resize 改变 buffer 结构导致 ydisp 重置）
          if (savedViewportY != null && st.term.buffer.active.viewportY !== savedViewportY) {
            // 确保不超出新的 ybase 上限
            const maxY = st.term.buffer.active.ybase;
            st.term.scrollToLine(Math.min(savedViewportY, maxY));
          }
          st.scrollThumb.update();
          st.term.focus();
        });
      });
    }
    this.activeId = id;
    // 标记此终端为活跃（更新 last_active_at 时间戳，供 session 恢复）
    ipc.ptySetActive(id).catch(() => {});
    this._notifyChange();
  }

  async closeTab(id) {
    const st = this.tabs.get(id);
    if (!st) return;
    // Skip ptyKill for error tabs — their id is not a valid PTY UUID
    if (!st.isError) {
      try { await ipc.ptyKill(id); } catch (e) {
        console.warn(`[closeTab] ptyKill failed for ${id}:`, e);
      }
    }
    // Dispose in order: mention → scrollThumb → data/resize listeners → term
    // Each dispose() removes its own event listeners to prevent memory leaks.
    try { if (st.mention) st.mention.dispose(); } catch (_) {}
    try { if (st.scrollThumb) st.scrollThumb.dispose(); } catch (_) {}
    try { if (st.dataDisposable) st.dataDisposable.dispose(); } catch (_) {}
    try { if (st.resizeDisposable) st.resizeDisposable.dispose(); } catch (_) {}
    try { st.term.dispose(); } catch (_) {}
    st.termEl.remove();
    try { st.unlistenExit(); } catch (_) {}
    try { st.unlistenRestart(); } catch (_) {}
    this.tabs.delete(id);
    this.order = this.order.filter((x) => x !== id);
    this.orbital.removeOrb(id);

    if (this.ripple) this.ripple.emitById(id, 'failure');

    if (this.activeId === id) {
      if (this.order.length > 0) {
        this.switchTo(this.order[0]);
      } else {
        this.activeId = null;
        // 最后一个终端关闭 → 空状态，用户通过侧栏或空球创建项目
      }
    }
    this._scheduleSave();
    this._notifyChange();
  }

  async restartTab(id) {
    const st = this.tabs.get(id);
    if (!st) return;

    // ── 构造带 rAF 合并的 onData 回调（复用 createTab 的逻辑）──
    // 重启后必须走 rAF 合并，否则每个 Channel.onmessage 直写 term.write
    // 导致快速输出时每次都触发 ANSI 解析 + canvas 重绘，严重卡顿
    // ★ 交互式响应（用户输入后 100ms 内的小数据）直接写入 — 低延迟。
    let _inactiveBuffer = st._inactiveBuffer || [];
    let _writeBatch = [];
    let _writeRaf = null;
    const rafOnData = (bytes) => {
      if (this.activeId === id) {
        _writeBatch.push(bytes);
        const byteLen = bytes.length || bytes.byteLength || 0;
        const isInputResponse = st.term._lastKeyTime && (performance.now() - st.term._lastKeyTime) < 100;
        if (_writeBatch.length === 1 && isInputResponse && byteLen < INTERACTIVE_BYTES) {
          st.term.write(bytes);
          _writeBatch = [];
          return;
        }
        if (!_writeRaf) {
          _writeRaf = requestAnimationFrame(() => {
            if (_writeBatch.length === 1) {
              st.term.write(_writeBatch[0]);
            } else if (_writeBatch.length > 1) {
              let total = 0;
              for (const b of _writeBatch) total += b.length || b.byteLength || 0;
              const merged = new Uint8Array(total);
              let offset = 0;
              for (const b of _writeBatch) {
                const chunk = (b instanceof Uint8Array) ? b : new Uint8Array(b);
                merged.set(chunk, offset);
                offset += chunk.length;
              }
              st.term.write(merged);
            }
            _writeBatch = [];
            _writeRaf = null;
          });
        }
      } else {
        _inactiveBuffer.push(bytes);
        if (_inactiveBuffer.length > 512) {
          _inactiveBuffer = _inactiveBuffer.slice(-256);
        }
      }
    };
    // 更新 state 中的缓冲和回调引用，供后续 switchTo 复用
    st._inactiveBuffer = _inactiveBuffer;
    st._writeBatch = _writeBatch;
    st._writeRaf = _writeRaf;
    st.onData = rafOnData;

    // For error tabs, create a fresh PTY instead of calling ptyRestart (invalid id)
    if (st.isError) {
      try {
        const result = await ipc.ptySpawn({
          cmd: st.cmd, args: st.args, cwd: st.cwd, env: st.env, label: st.label,
          backend: st.backend || null,
          projectId: st.projectId,
          cols: st.term.cols,
          rows: st.term.rows,
          onData: rafOnData,
        });
        // Replace the error tab with a valid PTY tab
        const realId = result.id;
        st.term.reset();
        // Register as a valid tab with the real PTY id
        st.termEl.dataset.id = realId;
        const unlistenExit = ipc.onPtyExit(realId, (code) => {
          this.updateStatus(realId, 'exited', code);
        });
        // ★ 统一 PTY 写入去重（与 createTab 相同逻辑，区分来源）
        let _lastPtyDataReal = '';
        let _lastPtyTimeReal = 0;
        let _lastPtySourceReal = '';
        const ptyWriteDedupReal = (data, source) => {
          const now = Date.now();
          if (data === _lastPtyDataReal && (now - _lastPtyTimeReal) < 200 && source !== _lastPtySourceReal) {
            return;
          }
          _lastPtyDataReal = data;
          _lastPtyTimeReal = now;
          _lastPtySourceReal = source;
          ipc.ptyWrite(realId, data).catch((e) => {
            console.warn(`[ptyWrite] failed for ${realId}:`, e);
          });
        };
        const dataDisposable = st.term.onData((data) => {
          // ★ 修复 7: 先过滤再去重（与 createTab 中 onData handler 一致）
          if (/^[a-zA-Z0-9]+( +[a-zA-Z0-9]+)+$/.test(data)) {
            data = data.replace(/ +/g, '');
          }

          // ★ onData 内部去重：比较过滤后的数据
          const now = Date.now();
          if (data === st.term._xtermSentData && (now - st.term._xtermSentTime) < 200) {
            return;
          }

          // ★ 修复 4: 记录过滤后的数据，供 ime-fix.js Fix 3 去重
          st.term._xtermSentData = data;
          st.term._xtermSentTime = Date.now();

          // ★ 统一去重写入 PTY
          ptyWriteDedupReal(data, 'onData');
        });
        const resizeDisposable = st.term.onResize(({ cols, rows }) => ipc.ptyResize(realId, cols, rows));
        const mention = new MentionController({
          term: st.term, termEl: st.termEl, popEl: this.mentionPop,
          sendInput: (s) => ipc.ptyWrite(realId, s).catch(() => {}),
          getAgents: () => (this.agentProvider ? this.agentProvider() : []),
        });
        // ★ 修复: 设置 _imeSendFn，error tab 重启后走统一去重
        st.term._imeSendFn = (data) => ptyWriteDedupReal(data, 'ime');
        // 让 ime-fix.js paste handler 能通知 mention 禁止触发
        st.term._mentionController = mention;

        // Flush inactive buffer 辅助函数
        const _flushInactiveBuffer = () => {
          if (_inactiveBuffer.length === 0) return;
          if (_inactiveBuffer.length === 1) {
            st.term.write(_inactiveBuffer[0]);
          } else {
            let total = 0;
            for (const b of _inactiveBuffer) total += b.length || b.byteLength || 0;
            const merged = new Uint8Array(total);
            let offset = 0;
            for (const b of _inactiveBuffer) {
              const chunk = (b instanceof Uint8Array) ? b : new Uint8Array(b);
              merged.set(chunk, offset);
              offset += chunk.length;
            }
            st.term.write(merged);
          }
          _inactiveBuffer = [];
        };

        const newState = {
          id: realId, label: st.label, cmd: st.cmd, args: st.args, cwd: st.cwd, env: st.env,
          projectId: st.projectId, backend: st.backend,
          term: st.term, fitAddon: st.fitAddon, scrollThumb: st.scrollThumb, termEl: st.termEl,
          mention, unlistenExit, dataDisposable, resizeDisposable,
          status: 'running', isError: false,
          _flushInactiveBuffer, _inactiveBuffer, _writeBatch, _writeRaf, onData: rafOnData,
        };
        // Remove old error tab AFTER new one is fully set up (avoids orphan on partial failure)
        this.tabs.delete(id);
        this.order = this.order.filter((x) => x !== id);
        this.orbital.removeOrb(id);
        this.tabs.set(realId, newState);
        this.order.push(realId);
        this.orbital.addOrb(realId, st.label, 'running', st.cwd);
        if (this.activeId === id) {
          this.activeId = realId;
          this.orbital.setActive(realId);
        }
        // ★ 修复: 重启后立即同步终端尺寸，避免换行/光标错位
        try {
          await ipc.ptyResize(realId, st.term.cols, st.term.rows);
        } catch (_) {}
        this._scheduleSave();
        this._notifyChange();
      } catch (err) {
        st.term.reset();
        st.term.write(`\x1b[31m✗ 重启仍然失败: ${err}\x1b[0m\r\n`);
      }
      return;
    }
    try {
      // ★ 修复: 传入带 rAF 合并的 onData 回调 + 终端当前尺寸
      await ipc.ptyRestart(id, rafOnData, st.term.cols, st.term.rows);
      st.term.reset();
      // ★ 修复: 重启后立即同步终端尺寸
      try {
        await ipc.ptyResize(id, st.term.cols, st.term.rows);
      } catch (_) {}
      this.updateStatus(id, 'running');
    } catch (err) {
      st.term.reset();
      st.term.write(`\x1b[31m✗ 重启失败: ${err}\x1b[0m\r\n`);
    }
  }

  switchByOffset(delta) {
    if (!this.activeId || this.order.length === 0) return;
    const idx = this.order.indexOf(this.activeId);
    const next = (idx + delta + this.order.length) % this.order.length;
    this.switchTo(this.order[next]);
  }

  switchByIndex(i) {
    if (i >= 0 && i < this.order.length) this.switchTo(this.order[i]);
  }

  getActiveCwd() {
    if (!this.activeId) return null;
    const st = this.tabs.get(this.activeId);
    return st ? st.cwd || null : null;
  }

  /** Return the backend ID of the currently active tab, or null. */
  getActiveBackend() {
    if (!this.activeId) return null;
    const st = this.tabs.get(this.activeId);
    return st ? st.backend || null : null;
  }

  /** Return the projectId of the currently active tab, or null. */
  getActiveProjectId() {
    if (!this.activeId) return null;
    const st = this.tabs.get(this.activeId);
    return st ? st.projectId || null : null;
  }

  /** Update cached backend install hints (called from orbital which fetches listBackends). */
  setBackendHints(hintsMap) {
    this._backendHints = hintsMap instanceof Map ? hintsMap : new Map(Object.entries(hintsMap || {}));
  }

  async persistNow() {
    if (this._saveTimer) { clearTimeout(this._saveTimer); this._saveTimer = null; }
    try { await ipc.sessionPersist(); } catch (_) {}
  }

  _scheduleSave() {
    if (this._saveTimer) clearTimeout(this._saveTimer);
    this._saveTimer = setTimeout(() => {
      this._saveTimer = null;
      ipc.sessionPersist().catch(() => {});
    }, AUTOSAVE_DEBOUNCE_MS);
  }

  updateStatus(id, status, code) {
    const st = this.tabs.get(id);
    if (st) { st.status = status; st.exitCode = code; }
    this.orbital.updateOrbStatus(id, status);

    // Emit ripple on terminal exit
    if (this.ripple && (status === 'exited' || status === 'crashed')) {
      const type = code === 0 ? 'success' : 'failure';
      this.ripple.emitById(id, type);
    }

    this._notifyChange();
  }

  _notifyChange() {
    if (typeof this.onChange === 'function') this.onChange(this);
  }
}
