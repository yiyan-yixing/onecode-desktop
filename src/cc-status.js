// CC Status 徽章（P1-7）。
// 移植自 onecode.html 的 ccBadge/renderCcBadges：定期拉取 cc_status，
// 在状态栏渲染 skills/hooks/plugins/tasks 计数徽章；
// 同时把 agents 列表回调出去供 @mention 使用。
//
// 当活跃后端不是 "claude-code" 时，跳过 IPC 调用并清空徽章
// （cc_status 仅适用于 Claude Code 的 .claude 目录结构）。

import * as ipc from './ipc-bridge.js';

const REFRESH_MS = 15000;

const ICONS = {
  skills:
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M8 1l2 4 4.5.7-3.3 3.1.8 4.5L8 11l-4 2.3.8-4.5L1.5 5.7 6 5z"/></svg>',
  hooks:
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M6 2v4l-3 3v2h4v3l1 2 1-2v-3h4V9l-3-3V2"/><rect x="5" y="1" width="6" height="2" rx="1"/></svg>',
  plugins:
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>',
  tasks:
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><circle cx="8" cy="8" r="6"/><polyline points="8 4 8 8 11 9.5"/></svg>',
};

export class CcStatusView {
  constructor({ badgeRoot, getProjectDir, getActiveBackend }) {
    this.badgeRoot = badgeRoot; // 徽章容器 DOM
    this.getProjectDir = getProjectDir; // () => string|null 活跃终端 cwd
    this.getActiveBackend = getActiveBackend || (() => null); // () => string|null 活跃后端 ID
    this.agents = [];
    this.onAgents = null; // agents 列表变更回调（@mention 用）
    this.onStatus = null; // status 数据变更回调（sidebar 用）
    this._lastData = null; // 缓存最新数据供 sidebar 使用
    this._timer = null;
  }

  start() {
    this.refresh();
    this._timer = setInterval(() => this.refresh(), REFRESH_MS);
  }

  stop() {
    if (this._timer) clearInterval(this._timer);
  }

  async refresh() {
    const backend = this.getActiveBackend();

    // cc_status only works with claude-code backend (reads .claude/ directories)
    // For other backends, clear badges and skip the IPC call
    if (backend && backend !== 'claude-code') {
      this._clearBadges();
      // Notify empty agents list
      if (this.agents.length > 0) {
        this.agents = [];
        if (typeof this.onAgents === 'function') this.onAgents([]);
      }
      return;
    }

    const dir = this.getProjectDir();
    const data = await ipc.ccStatus(dir).catch(() => null);
    if (!data) return;
    const agents = data.agents || [];
    // 仅在列表变化时回调，避免每轮触发
    if (JSON.stringify(agents) !== JSON.stringify(this.agents)) {
      this.agents = agents;
      if (typeof this.onAgents === 'function') this.onAgents(agents);
    }
    this._lastData = data;
    this._render(data);
    if (typeof this.onStatus === 'function') this.onStatus(data);
  }

  _clearBadges() {
    if (!this.badgeRoot) return;
    const keys = ['skills', 'hooks', 'plugins', 'tasks'];
    for (const t of keys) {
      const pill = this.badgeRoot.querySelector(`[data-cc="${t}"]`);
      if (pill) {
        const numEl = pill.querySelector('.n');
        if (numEl && numEl.textContent !== '0') numEl.textContent = '0';
        pill.classList.remove('has');
      }
    }
  }

  _render(data) {
    if (!this.badgeRoot) return;
    const counts = {
      skills: (data.skills || []).length,
      hooks: Object.values(data.hooks || {}).reduce(
        (n, v) => n + ((v && v.length) || 0),
        0,
      ),
      plugins: (data.plugins || []).length,
      tasks: (data.tasks || []).length,
    };
    // 增量更新：只在数值变化时才更新 DOM
    const keys = ['skills', 'hooks', 'plugins', 'tasks'];
    for (const t of keys) {
      const pill = this.badgeRoot.querySelector(`[data-cc="${t}"]`);
      const n = counts[t];
      if (pill) {
        const numEl = pill.querySelector('.n');
        if (numEl && numEl.textContent !== String(n)) {
          numEl.textContent = n;
        }
        const hasClass = pill.classList.contains('has');
        if (n > 0 && !hasClass) pill.classList.add('has');
        if (n === 0 && hasClass) pill.classList.remove('has');
      }
    }
    // 首次渲染时创建 DOM — statusbar badge style
    if (!this.badgeRoot.querySelector('[data-cc]')) {
      this.badgeRoot.innerHTML = keys
        .map(
          (t) =>
            `<span class="ribbon-badge ${counts[t] > 0 ? 'has' : ''}" data-cc="${t}" ` +
            `title="${t}: ${counts[t]}">${ICONS[t]}<span class="n">${counts[t]}</span></span>`,
        )
        .join('');
    }
  }
}
