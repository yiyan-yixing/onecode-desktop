// 多终端 Tab 管理（P0 核心）。
//
// 设计：每个 Tab 对应一个 xterm 实例 + 一个 Rust slot。M1 策略——
// xterm 实例始终 attached（display:none 隐藏非活跃），Channel.onmessage 持续
// 把 PTY 输出 write 到对应 term，故 Tab 切换无需显式 replay（内容已在 buffer）。
// pty_replay 命令保留，供 M2 「detach 非活跃终端」优化使用。

import { createTerminal } from './terminal.js';
import { ScrollThumb } from './scroll-thumb.js';
import { initImeFix } from './ime-fix.js';
import * as ipc from '../ipc-bridge.js';

export class TabManager {
  constructor() {
    this.tabs = new Map(); // id → state
    this.order = []; // Tab 顺序（用于 Cmd+1~9 / Shift+[/]）
    this.activeId = null;
    this.tabBar = null;
    this.termContainer = null;
    this.onChange = null; // 外部钩子（更新状态栏）
  }

  init() {
    this.tabBar = document.getElementById('tabBar');
    this.termContainer = document.getElementById('termContainer');
    document.getElementById('tabNew').addEventListener('click', () => this.createTab());
    this.createTab({ label: 'main' });
  }

  async createTab(opts = {}) {
    const label = opts.label || `term-${this.tabs.size + 1}`;

    const termEl = document.createElement('div');
    termEl.className = 'term-instance';
    termEl.style.display = 'none';
    this.termContainer.appendChild(termEl);

    const { term, fitAddon } = createTerminal(termEl);
    const scrollThumb = new ScrollThumb(term, termEl);
    initImeFix(term);

    const result = await ipc.ptySpawn({
      cmd: opts.cmd,
      args: opts.args,
      cwd: opts.cwd,
      env: opts.env,
      label,
      onData: (bytes) => term.write(bytes),
    });
    const id = result.id;
    termEl.dataset.id = id;

    const unlistenExit = ipc.onPtyExit(id, (code) => {
      this.updateStatus(id, 'exited', code);
    });

    term.onData((data) => ipc.ptyWrite(id, data));
    term.onResize(({ cols, rows }) => ipc.ptyResize(id, cols, rows));

    const state = {
      id,
      label,
      term,
      fitAddon,
      scrollThumb,
      termEl,
      unlistenExit,
      status: 'running',
    };
    this.tabs.set(id, state);
    this.order.push(id);

    this.addTabDom(id, label, 'running');
    this.switchTo(id);
    this._notifyChange();
    return id;
  }

  switchTo(id) {
    for (const [tid, st] of this.tabs) {
      st.termEl.style.display = tid === id ? 'block' : 'none';
    }
    this.tabBar.querySelectorAll('.tab-item').forEach((el) => {
      el.classList.toggle('active', el.dataset.id === id);
    });
    const st = this.tabs.get(id);
    if (st) {
      setTimeout(() => {
        st.fitAddon.fit();
        st.scrollThumb.update();
      }, 60);
    }
    this.activeId = id;
    this._notifyChange();
  }

  async closeTab(id) {
    const st = this.tabs.get(id);
    if (!st) return;
    await ipc.ptyKill(id);
    try {
      st.term.dispose();
    } catch (_) {}
    st.termEl.remove();
    try {
      st.unlistenExit();
    } catch (_) {}
    this.tabs.delete(id);
    this.order = this.order.filter((x) => x !== id);
    this.removeTabDom(id);

    if (this.activeId === id) {
      if (this.order.length > 0) {
        this.switchTo(this.order[0]);
      } else {
        this.activeId = null;
        this.createTab(); // 至少保留一个
      }
    }
    this._notifyChange();
  }

  async restartTab(id) {
    const st = this.tabs.get(id);
    if (!st) return;
    await ipc.ptyRestart(id, (bytes) => st.term.write(bytes));
    st.term.reset();
    this.updateStatus(id, 'running');
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

  // ── DOM 操作 ──────────────────────────────────────────────────

  addTabDom(id, label, status) {
    const el = document.createElement('div');
    el.className = 'tab-item';
    el.dataset.id = id;
    el.innerHTML =
      `<span class="tab-dot ${status}"></span>` +
      `<span class="tab-label">${escapeHtml(label)}</span>` +
      `<span class="tab-close" title="Ctrl/Cmd + W">×</span>`;
    el.addEventListener('click', (e) => {
      if (e.target.classList.contains('tab-close')) {
        e.stopPropagation();
        this.closeTab(id);
      } else if (e.target.classList.contains('tab-dot') && status === 'exited') {
        this.restartTab(id);
      } else {
        this.switchTo(id);
      }
    });
    const labelEl = el.querySelector('.tab-label');
    labelEl.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      this.startRename(id, labelEl);
    });
    this.tabBar.insertBefore(el, document.getElementById('tabNew'));
    this.tabBar.querySelectorAll('.tab-item').forEach((t) => {
      t.classList.toggle('active', t.dataset.id === id);
    });
  }

  removeTabDom(id) {
    const el = this.tabBar.querySelector(`.tab-item[data-id="${id}"]`);
    if (el) el.remove();
  }

  updateStatus(id, status, code) {
    const st = this.tabs.get(id);
    if (st) {
      st.status = status;
      st.exitCode = code;
    }
    const item = this.tabBar.querySelector(`.tab-item[data-id="${id}"]`);
    if (item) {
      const dot = item.querySelector('.tab-dot');
      dot.classList.remove('running', 'exited', 'crashed');
      dot.classList.add(status);
      item.classList.toggle('exited', status === 'exited');
    }
    this._notifyChange();
  }

  startRename(id, labelEl) {
    const cur = labelEl.textContent;
    const input = document.createElement('input');
    input.className = 'tab-rename-input';
    input.value = cur;
    labelEl.replaceWith(input);
    input.focus();
    input.select();
    const commit = () => {
      const val = (input.value || '').trim() || cur;
      const span = document.createElement('span');
      span.className = 'tab-label';
      span.textContent = val;
      input.replaceWith(span);
      ipc.ptyRename(id, val);
      const st = this.tabs.get(id);
      if (st) st.label = val;
      span.addEventListener('dblclick', (e) => {
        e.stopPropagation();
        this.startRename(id, span);
      });
      this._notifyChange();
    };
    input.addEventListener('blur', commit);
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') input.blur();
      if (e.key === 'Escape') {
        input.value = cur;
        input.blur();
      }
      e.stopPropagation();
    });
  }

  _notifyChange() {
    if (typeof this.onChange === 'function') this.onChange(this);
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  }[c]));
}
