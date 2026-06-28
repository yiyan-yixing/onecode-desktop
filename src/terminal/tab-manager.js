// 多终端管理器（PTY 生命周期 + 终端实例管理）。
//
// 解耦 SidebarManager → OrbitalController。
// 使用 visibility/opacity 替代 display:none 实现终端切换。

import { createTerminal } from './terminal.js';
import { ScrollThumb } from './scroll-thumb.js';
import { initImeFix } from './ime-fix.js';
import { MentionController } from './mention.js';
import * as ipc from '../ipc-bridge.js';

const AUTOSAVE_DEBOUNCE_MS = 350;

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
  }

  init() {
    this.termContainer = document.getElementById('termViewport');
    this.mentionPop = document.getElementById('mentionPortal');
  }

  async restoreOrInit(slots) {
    // Only restore chat terminal (no projectId) — project terminals are created
    // on-demand by clicking project cards, not from session persistence.
    if (slots && slots.length) {
      const chatSlot = slots.find(s => !s.project_id);
      if (chatSlot) {
        await this.createTab({
          label: chatSlot.label,
          cmd: chatSlot.cmd,
          args: chatSlot.args,
          cwd: chatSlot.cwd,
          env: chatSlot.env,
          projectId: null,
        });
      } else {
        await this.createTab({ cwd: undefined, label: undefined });
      }
    } else {
      await this.createTab({ cwd: undefined, label: undefined });
    }
  }

  async createTab(opts = {}) {
    // Chat (no projectId) = single terminal — switch to existing instead of creating
    if (!opts.projectId) {
      const chatId = this.getChatTabId();
      if (chatId) {
        this.switchTo(chatId);
        return chatId;
      }
    } else {
      // Project terminal — one per project, switch to existing
      for (const [tid, st] of this.tabs) {
        if (st.projectId === opts.projectId && !st.isError) {
          this.switchTo(tid);
          return tid;
        }
      }
    }

    const label = opts.label || 'New Chat';

    const termEl = document.createElement('div');
    termEl.className = 'term-instance';
    this.termContainer.appendChild(termEl);

    const { term, fitAddon } = createTerminal(termEl);
    const scrollThumb = new ScrollThumb(term, termEl);
    initImeFix(term);

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
        onData: (bytes) => term.write(bytes),
      });
      id = result.id;
      pid = result.pid || null;
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
        term.write('  • 工作目录不存在\r\n\r\n');
      }
      term.write('  \x1b[36m点击左侧光球状态点可重新启动，或关闭此标签页\x1b[0m\r\n');
      id = 'err-' + Date.now();
      termEl.dataset.id = id;
      this.orbital.addOrb(id, label, 'exited', opts.cwd);
      this.switchTo(id);
      // Mark as error tab: no IPC callbacks registered, closeTab will skip ptyKill
      this.tabs.set(id, {
        id, label, cmd: opts.cmd, args: opts.args, cwd: opts.cwd, env: opts.env,
        projectId: opts.projectId || null,
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

    const dataDisposable = term.onData((data) => {
      ipc.ptyWrite(id, term.imeFilter ? term.imeFilter(data) : data);
    });
    const resizeDisposable = term.onResize(({ cols, rows }) => ipc.ptyResize(id, cols, rows));

    const mention = new MentionController({
      term,
      termEl,
      popEl: this.mentionPop,
      sendInput: (s) => ipc.ptyWrite(id, s),
      getAgents: () => (this.agentProvider ? this.agentProvider() : []),
    });

    const state = {
      id, label, cmd: opts.cmd, args: opts.args, cwd: opts.cwd, env: opts.env,
      projectId: opts.projectId || null,
      pid,
      term, fitAddon, scrollThumb, termEl, mention, unlistenExit,
      dataDisposable, resizeDisposable,
      status: 'running',
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
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          try { st.fitAddon.fit(); } catch (_) {}
          st.scrollThumb.update();
          st.term.focus();
        });
      });
    }
    this.activeId = id;
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
    this.tabs.delete(id);
    this.order = this.order.filter((x) => x !== id);
    this.orbital.removeOrb(id);

    if (this.ripple) this.ripple.emitById(id, 'failure');

    if (this.activeId === id) {
      if (this.order.length > 0) {
        this.switchTo(this.order[0]);
      } else {
        this.activeId = null;
        this.createTab();
      }
    }
    this._scheduleSave();
    this._notifyChange();
  }

  async restartTab(id) {
    const st = this.tabs.get(id);
    if (!st) return;
    // For error tabs, create a fresh PTY instead of calling ptyRestart (invalid id)
    if (st.isError) {
      try {
        const result = await ipc.ptySpawn({
          cmd: st.cmd, args: st.args, cwd: st.cwd, env: st.env, label: st.label,
          onData: (bytes) => st.term.write(bytes),
        });
        // Replace the error tab with a valid PTY tab
        const realId = result.id;
        st.term.reset();
        // Register as a valid tab with the real PTY id
        st.termEl.dataset.id = realId;
        const unlistenExit = ipc.onPtyExit(realId, (code) => {
          this.updateStatus(realId, 'exited', code);
        });
        const dataDisposable = st.term.onData((data) => {
          ipc.ptyWrite(realId, st.term.imeFilter ? st.term.imeFilter(data) : data);
        });
        const resizeDisposable = st.term.onResize(({ cols, rows }) => ipc.ptyResize(realId, cols, rows));
        const mention = new MentionController({
          term: st.term, termEl: st.termEl, popEl: this.mentionPop,
          sendInput: (s) => ipc.ptyWrite(realId, s),
          getAgents: () => (this.agentProvider ? this.agentProvider() : []),
        });
        const newState = {
          id: realId, label: st.label, cmd: st.cmd, args: st.args, cwd: st.cwd, env: st.env,
          term: st.term, fitAddon: st.fitAddon, scrollThumb: st.scrollThumb, termEl: st.termEl,
          mention, unlistenExit, dataDisposable, resizeDisposable,
          status: 'running', isError: false,
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
        this._scheduleSave();
        this._notifyChange();
      } catch (err) {
        st.term.reset();
        st.term.write(`\x1b[31m✗ 重启仍然失败: ${err}\x1b[0m\r\n`);
      }
      return;
    }
    try {
      await ipc.ptyRestart(id, (bytes) => st.term.write(bytes));
      st.term.reset();
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

  /** Find the single chat terminal (no projectId). Returns tab id or null. */
  getChatTabId() {
    for (const [id, st] of this.tabs) {
      if (!st.isError && !st.projectId) return id;
    }
    return null;
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
