// 指令调色板 — Cmd+K 打开的浮动玻璃面板，替代侧栏/设置/搜索。
//
// 前缀语法：
//   无前缀 → 模糊搜索终端 + agents
//   @      → Agent 列表，选择→插入 @mention
//   >      → 设置模式，内联编辑
//   new/+  → 新建终端

import * as ipc from './ipc-bridge.js';

const ICONS = {
  skills: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M8 1l2 4 4.5.7-3.3 3.1.8 4.5L8 11l-4 2.3.8-4.5L1.5 5.7 6 5z"/></svg>',
  hooks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M6 2v4l-3 3v2h4v3l1 2 1-2v-3h4V9l-3-3V2"/><rect x="5" y="1" width="6" height="2" rx="1"/></svg>',
  plugins: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>',
  tasks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><circle cx="8" cy="8" r="6"/><polyline points="8 4 8 8 11 9.5"/></svg>',
};

export class PaletteController {
  constructor() {
    this.tm = null;
    this.overlay = null;
    this.input = null;
    this.results = null;
    this.agentProvider = null;
    this._selIdx = 0;
    this._items = [];
    this._open = false;
  }

  init(tm) {
    this.tm = tm;
    this.overlay = document.getElementById('paletteOverlay');
    this.input = document.getElementById('paletteInput');
    this.results = document.getElementById('paletteResults');

    // Input
    this.input.addEventListener('input', () => this._render());
    this.input.addEventListener('keydown', (e) => this._onKey(e));

    // Click outside to dismiss
    this.overlay.addEventListener('click', (e) => {
      if (e.target === this.overlay) this.close();
    });
  }

  setAgentProvider(fn) {
    this.agentProvider = fn;
  }

  open() {
    if (this._open) return;
    this._open = true;
    this.overlay.classList.remove('dismissing');
    this.overlay.classList.add('on');
    this.input.value = '';
    this._selIdx = 0;
    this._render();
    requestAnimationFrame(() => this.input.focus());
  }

  close() {
    if (!this._open) return;
    this._open = false;
    this.overlay.classList.add('dismissing');
    this.overlay.classList.remove('on');
    setTimeout(() => {
      this.overlay.classList.remove('dismissing');
      // Return focus to terminal
      if (this.tm && this.tm.activeId) {
        const st = this.tm.tabs.get(this.tm.activeId);
        if (st) st.term.focus();
      }
    }, 200);
  }

  toggle() {
    this._open ? this.close() : this.open();
  }

  _onKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      this.close();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      this._selIdx = Math.min(this._selIdx + 1, this._items.length - 1);
      this._highlightSel();
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      this._selIdx = Math.max(this._selIdx - 1, 0);
      this._highlightSel();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (this._items[this._selIdx]) this._items[this._selIdx].action();
      this.close();
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      // Autocomplete: if only one result, select it
      if (this._items.length === 1 && this._items[0]) {
        this._items[0].action();
        this.close();
      }
      return;
    }
  }

  _highlightSel() {
    this.results.querySelectorAll('.palette-item').forEach((el, i) => {
      el.classList.toggle('sel', i === this._selIdx);
    });
    // Scroll into view
    const sel = this.results.querySelector('.palette-item.sel');
    if (sel) sel.scrollIntoView({ block: 'nearest' });
  }

  _render() {
    const q = (this.input.value || '').toLowerCase().trim();
    this._items = [];
    this._selIdx = 0;

    const isSettingMode = q.startsWith('>');
    const isAgentMode = q.startsWith('@');
    const isNewMode = q.startsWith('new') || q.startsWith('+');

    if (isSettingMode) {
      this._renderSettings(q.slice(1).trim());
    } else if (isAgentMode) {
      this._renderAgents(q.slice(1).trim());
    } else {
      this._renderDefault(q, isNewMode);
    }

    this._highlightSel();
  }

  _renderDefault(q, isNewMode) {
    let html = '';

    // Terminals
    const terminals = [];
    if (this.tm) {
      for (const [id, st] of this.tm.tabs) {
        if (!q || st.label.toLowerCase().includes(q) || (st.cwd && st.cwd.toLowerCase().includes(q))) {
          terminals.push({ id, label: st.label, cwd: st.cwd, status: st.status, index: this.tm.order.indexOf(id) });
        }
      }
    }

    if (terminals.length) {
      html += '<div class="palette-group-label">Terminals</div>';
      terminals.forEach((t, i) => {
        const dotClass = t.status === 'running' ? 'running' : t.status === 'crashed' ? 'crashed' : 'exited';
        const kbd = t.index < 9 ? `<span class="palette-item-kbd">⌘${t.index + 1}</span>` : '';
        const cwdBasename = t.cwd ? t.cwd.split('/').pop() : '';
        html +=
          `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:${this._orbColorForId(t.id)}">${esc(t.label[0].toUpperCase())}</span>` +
          `<span class="palette-item-label">${esc(t.label)}</span>` +
          `<span class="palette-item-detail">${esc(cwdBasename)}</span>` +
          `${kbd}</div>`;
        this._items.push({ action: () => this.tm.switchTo(t.id) });
      });
    }

    // Agents
    const agents = this.agentProvider ? this.agentProvider() : [];
    const filteredAgents = q ? agents.filter((a) =>
      (a.name || '').toLowerCase().includes(q) || (a.id || '').toLowerCase().includes(q)
    ) : agents;

    if (filteredAgents.length) {
      html += '<div class="palette-group-label">Agents</div>';
      filteredAgents.forEach((a, i) => {
        const color = (a.color && a.color.startsWith('#')) ? a.color : '#A78BFA';
        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:${color}">${esc((a.icon || a.id || '?')[0].toUpperCase())}</span>` +
          `<span class="palette-item-label">@${esc(a.id)}</span>` +
          `<span class="palette-item-detail">${esc(a.name || '')}</span></div>`;
        this._items.push({ action: () => {
          if (this.tm && this.tm.activeId) {
            ipc.ptyWrite(this.tm.activeId, `@${a.id} `);
            const st = this.tm.tabs.get(this.tm.activeId);
            if (st) st.term.focus();
          }
        }});
      });
    }

    // Actions
    html += '<div class="palette-group-label">Actions</div>';

    // New terminal
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--id-emerald)">+</span>` +
      `<span class="palette-item-label">New Terminal</span>` +
      `<span class="palette-item-kbd">⌘T</span></div>`;
    this._items.push({ action: () => this.tm.createTab() });

    // Settings
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--aurora-overcast)">⚙</span>` +
      `<span class="palette-item-label">Preferences</span>` +
      `<span class="palette-item-detail">type &gt; to edit</span></div>`;
    this._items.push({ action: () => { this.input.value = '> '; this._render(); } });

    // CC Status summary
    html += '<div class="palette-group-label">Status</div>';
    html += '<div class="palette-item" style="cursor:default;gap:6px">';
    html += '<span style="font-size:11px;color:var(--text-void)">Skills · Hooks · Plugins · Tasks</span>';
    html += '</div>';

    this.results.innerHTML = html;
  }

  _renderAgents(q) {
    const agents = this.agentProvider ? this.agentProvider() : [];
    const filtered = q ? agents.filter((a) =>
      (a.name || '').toLowerCase().includes(q) || (a.id || '').toLowerCase().includes(q)
    ) : agents;

    let html = '<div class="palette-group-label">Agents</div>';
    filtered.forEach((a, i) => {
      const color = (a.color && a.color.startsWith('#')) ? a.color : '#A78BFA';
      html +=
        `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:${color}">${esc((a.icon || a.id || '?')[0].toUpperCase())}</span>` +
        `<span class="palette-item-label">@${esc(a.id)} ${esc(a.name || '')}</span></div>`;
      this._items.push({ action: () => {
        if (this.tm && this.tm.activeId) {
          ipc.ptyWrite(this.tm.activeId, `@${a.id} `);
          const st = this.tm.tabs.get(this.tm.activeId);
          if (st) st.term.focus();
        }
      }});
    });

    if (!filtered.length) {
      html += '<div class="palette-item" style="cursor:default"><span class="palette-item-label" style="color:var(--text-void)">No agents configured</span></div>';
    }

    this.results.innerHTML = html;
  }

  _renderSettings(q) {
    const settings = [
      { key: 'command', label: 'Default Command', getValue: () => 'claude' },
      { key: 'args', label: 'Default Arguments', getValue: () => '--permission-mode bypassPermissions' },
      { key: 'cwd', label: 'Working Directory', getValue: () => '~/.onecode/workspace' },
      { key: 'max', label: 'Max Terminals', getValue: () => '10' },
      { key: 'buffer', label: 'Buffer Size (MB)', getValue: () => '10' },
    ];

    // Try loading from config
    ipc.loadConfig().then((cfg) => {
      settings[0].getValue = () => cfg.default_cmd || 'claude';
      settings[1].getValue = () => (cfg.default_args || []).join(' ');
      settings[2].getValue = () => cfg.default_cwd || '';
      settings[3].getValue = () => String(cfg.max_terminals || 10);
      settings[4].getValue = () => String(cfg.ring_buffer_max_mb || 10);
      this._renderSettingsItems(settings, q);
    }).catch(() => {
      this._renderSettingsItems(settings, q);
    });
  }

  _renderSettingsItems(settings, q) {
    const filtered = q ? settings.filter((s) => s.label.toLowerCase().includes(q)) : settings;
    let html = '<div class="palette-group-label">Preferences</div>';

    filtered.forEach((s, i) => {
      html +=
        `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:var(--aurora-overcast)">⚙</span>` +
        `<span class="palette-item-label">${esc(s.label)}</span>` +
        `<span class="palette-item-detail">${esc(s.getValue())}</span></div>`;
      this._items.push({ action: () => this._editSetting(s) });
    });

    this.results.innerHTML = html;
  }

  _editSetting(setting) {
    // Replace the palette content with an inline input for this setting
    const current = setting.getValue();
    this.results.innerHTML =
      `<div class="palette-group-label">Edit: ${esc(setting.label)}</div>` +
      `<div class="palette-item" style="padding:8px 20px">` +
      `<input class="palette-setting-input" id="settingInput" value="${esc(current)}">` +
      `</div>` +
      `<div class="palette-item" style="cursor:default;padding:4px 20px">` +
      `<span style="font-size:11px;color:var(--text-void)">Enter to save · Escape to cancel</span></div>`;

    const input = document.getElementById('settingInput');
    input.focus();
    input.select();

    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        const val = input.value.trim();
        this._saveSetting(setting.key, val);
        this.close();
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        this.input.value = '> ';
        this._render();
      }
      e.stopPropagation();
    });

    input.addEventListener('blur', () => {
      // Don't auto-close on blur, let Escape handle it
    });
  }

  async _saveSetting(key, value) {
    const cfg = {};
    if (key === 'command') cfg.default_cmd = value;
    else if (key === 'args') cfg.default_args = value.split(/\s+/).filter(Boolean);
    else if (key === 'cwd') cfg.default_cwd = value;
    else if (key === 'max') cfg.max_terminals = parseInt(value) || 10;
    else if (key === 'buffer') cfg.ring_buffer_max_mb = parseInt(value) || 10;
    try { await ipc.saveConfig(cfg); } catch (e) { console.warn('[palette] save failed', e); }
  }

  _orbColorForId(id) {
    const orb = document.querySelector(`.orb[data-id="${id}"]`);
    if (orb) {
      const c = orb.dataset.identityColor;
      if (c) return c.replace('var(', '').replace(')', '');
    }
    return '--id-emerald';
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}
