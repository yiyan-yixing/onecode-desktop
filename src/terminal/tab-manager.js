// 多终端 Tab 管理（P0 核心 + P1 接线）。
//
// 设计：每个 Tab 对应一个 xterm 实例 + 一个 Rust slot。M1 策略——
// xterm 实例始终 attached（display:none 隐藏非活跃），Channel.onmessage 持续
// 把 PTY 输出 write 到对应 term，故 Tab 切换无需显式 replay（内容已在 buffer）。
// pty_replay 命令保留，供 M2「detach 非活跃终端」优化使用。
//
// P1 接线：
// - @mention：每个终端挂一个 MentionController，共享 .mention-pop 弹窗。
// - 会话持久化：create/close/rename 后去抖 sessionPersist；app:before-quit 时立即落库。
// - CC Status：暴露 getActiveCwd() 供徽章读取 {cwd}/.claude。

import { createTerminal } from './terminal.js';
import { ScrollThumb } from './scroll-thumb.js';
import { initImeFix } from './ime-fix.js';
import { MentionController } from './mention.js';
import * as ipc from '../ipc-bridge.js';

const AUTOSAVE_DEBOUNCE_MS = 350;

export class TabManager {
  constructor() {
    this.tabs = new Map(); // id → state
    this.order = []; // Tab 顺序（用于 Cmd+1~9 / Shift+[/]）
    this.activeId = null;
    this.tabBar = null;
    this.termContainer = null;
    this.mentionPop = null; // 共享 @mention 弹窗
    this.agentProvider = null; // () => AgentInfo[]
    this.onChange = null; // 外部钩子（更新状态栏）
    this._saveTimer = null;
  }

  init() {
    this.tabBar = document.getElementById('tabBar');
    this.termContainer = document.getElementById('termContainer');
    this.mentionPop = document.getElementById('mentionPop');
    document.getElementById('tabNew').addEventListener('click', () => this.createTab());
  }

  /** 启动恢复：按上次保存的配置重建终端；无记录则建默认 main。 */
  async restoreOrInit(slots) {
    if (slots && slots.length) {
      for (const s of slots) {
        await this.createTab({
          label: s.label,
          cmd: s.cmd,
          args: s.args,
          cwd: s.cwd,
          env: s.env,
        });
      }
    } else {
      await this.createTab({ label: 'main' });
    }
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

    // @mention 控制器
    const mention = new MentionController({
      term,
      termEl,
      popEl: this.mentionPop,
      sendInput: (s) => ipc.ptyWrite(id, s),
      getAgents: () => (this.agentProvider ? this.agentProvider() : []),
    });

    const state = {
      id,
      label,
      cmd: opts.cmd,
      args: opts.args,
      cwd: opts.cwd,
      env: opts.env,
      term,
      fitAddon,
      scrollThumb,
      termEl,
      mention,
      unlistenExit,
      status: 'running',
    };
    this.tabs.set(id, state);
    this.order.push(id);

    this.addTabDom(id, label, 'running');
    this.switchTo(id);
    this._scheduleSave();
    this._notifyChange();
    return id;
  }

  switchTo(id) {
    for (const [tid, st] of this.tabs) {
      st.termEl.style.display = tid === id ? 'block' : 'none';
      // 切走时隐藏残留的 mention 弹窗
      if (tid !== id && st.mention) st.mention.hide();
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
    this._scheduleSave();
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

  /** 活跃终端的 cwd（CC Status 项目目录用）。 */
  getActiveCwd() {
    if (!this.activeId) return null;
    const st = this.tabs.get(this.activeId);
    return st ? st.cwd || null : null;
  }

  /** 立即落库（app:before-quit / 托盘退出用）。 */
  async persistNow() {
    if (this._saveTimer) {
      clearTimeout(this._saveTimer);
      this._saveTimer = null;
    }
    try {
      await ipc.sessionPersist();
    } catch (_) {}
  }

  /** 去抖保存：create/close/rename 后 350ms 合并落库。 */
  _scheduleSave() {
    if (this._saveTimer) clearTimeout(this._saveTimer);
    this._saveTimer = setTimeout(() => {
      this._saveTimer = null;
      ipc.sessionPersist().catch(() => {});
    }, AUTOSAVE_DEBOUNCE_MS);
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
      this._scheduleSave();
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
