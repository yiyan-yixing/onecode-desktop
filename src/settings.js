// 设置面板 — ⌘, 打开的独立设置窗口（P1-4）
//
// 替代隐藏的 ⌘K > 设置模式，提供 tab 分类的偏好设置界面。
// Tab: [ AI后端 | 高级 ]
//
// 数据流：settings.js ← ipc-bridge.js → Rust config.rs

import * as ipc from './ipc-bridge.js';

// 当前设置缓存
let currentConfig = null;
let backends = [];

export class SettingsController {
  constructor() {
    this._overlay = null;
    this._open = false;
    this._dirty = false;
    this._activeTab = 'backend';
  }

  /** 重载配置（从后端重新拉取，用于外部变更后刷新面板内容） */
  async _reloadConfig() {
    try {
      currentConfig = await ipc.loadConfig();
      // 如果面板已打开，重新填充表单字段
      if (this._open && this._overlay) {
        this._populateForm(this._overlay);
      }
    } catch (e) {
      console.warn('[settings] reload failed:', e);
    }
  }

  /** 用 currentConfig 刷新面板中的表单字段值（不重建 DOM） */
  _populateForm(overlay) {
    const setVal = (id, val) => {
      const el = overlay.querySelector(`#${id}`);
      if (el) el.value = val;
    };
    setVal('settingsBackend', currentConfig?.active_backend || 'claude-code');
    setVal('settingsApiKey', currentConfig?.api_key || '');
    setVal('settingsBaseUrl', currentConfig?.base_url || 'https://api.anthropic.com');
    setVal('settingsModel', currentConfig?.model || 'claude-sonnet-4-6');
    setVal('settingsCmd', currentConfig?.default_cmd || 'claude');
    setVal('settingsArgs', (currentConfig?.default_args || []).join(' '));
    setVal('settingsMaxTerms', String(currentConfig?.max_terminals || 10));
    setVal('settingsBuffer', String(currentConfig?.ring_buffer_max_mb || 10));
  }

  async open() {
    if (this._open) return;
    this._open = true;

    // Load current config + backends
    try {
      currentConfig = await ipc.loadConfig();
    } catch (e) {
      console.warn('[settings] load config failed:', e);
      currentConfig = {};
    }
    try {
      backends = await ipc.listBackends();
    } catch (e) {
      backends = [];
    }

    this._mount();
  }

  /** 重载配置并刷新面板（供外部变更后调用） */
  async reloadConfig() {
    return this._reloadConfig();
  }

  close() {
    if (!this._open) return;
    this._open = false;
    this._dirty = false;
    this._dismiss();
  }

  toggle() {
    this._open ? this.close() : this.open();
  }

  _mount() {
    if (this._overlay) return;

    const overlay = document.createElement('div');
    overlay.id = 'settingsOverlay';
    overlay.className = 'np-overlay';

    // Build backend selector options
    const installed = backends.filter(b => b.installed);
    const uninstalled = backends.filter(b => !b.installed);
    const sortedBackends = [...installed, ...uninstalled];

    const backendOptions = sortedBackends.map(b => {
      const icon = b.installed ? '●' : '○';
      const disabled = b.installed ? '' : ' disabled';
      const selected = (currentConfig?.active_backend || 'claude-code') === b.id ? ' selected' : '';
      return `<option value="${esc(b.id)}"${disabled}${selected}>${icon} ${esc(b.display_name)}</option>`;
    }).join('');

    overlay.innerHTML = `
      <div class="np-dialog settings-dialog">
        <div class="np-header">
          <div class="np-title">偏好设置</div>
          <button class="np-close settings-close" title="关闭 (⎋)"><svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" style="width:12px;height:12px"><line x1="3" y1="3" x2="11" y2="11"/><line x1="11" y1="3" x2="3" y2="11"/></svg></button>
        </div>

        <!-- Tab Bar -->
        <div class="settings-tabs">
          <button class="settings-tab active" data-settings-tab="backend">AI 后端</button>
          <button class="settings-tab" data-settings-tab="advanced">高级</button>
        </div>

        <!-- Backend Tab -->
        <div class="settings-tab-content active" data-settings-content="backend">
          <div class="settings-field">
            <label class="settings-label">选择后端</label>
            <select id="settingsBackend" class="np-select">${backendOptions}</select>
            <div class="settings-hint" id="settingsBackendHint"></div>
          </div>
          <div class="settings-field">
            <label class="settings-label">API Key</label>
            <div class="settings-input-row">
              <input type="password" id="settingsApiKey" class="np-input np-mono"
                     value="${esc(currentConfig?.api_key || '')}" placeholder="sk-ant-…"
                     autocomplete="off" spellcheck="false">
              <button class="settings-eye-btn" id="settingsEyeBtn" title="显示/隐藏">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" style="width:14px;height:14px"><path d="M1 8s3-5 7-5 7 5 7 5-3 5-7 5-7-5-7-5z"/><circle cx="8" cy="8" r="2"/></svg>
              </button>
            </div>
          </div>
          <div class="settings-field">
            <label class="settings-label">Base URL</label>
            <input type="text" id="settingsBaseUrl" class="np-input np-mono"
                   value="${esc(currentConfig?.base_url || 'https://api.anthropic.com')}"
                   placeholder="https://api.anthropic.com" autocomplete="off" spellcheck="false">
          </div>
          <div class="settings-field">
            <label class="settings-label">Model</label>
            <input type="text" id="settingsModel" class="np-input np-mono"
                   value="${esc(currentConfig?.model || 'claude-sonnet-4-6')}"
                   placeholder="claude-sonnet-4-6" autocomplete="off" spellcheck="false">
          </div>
        </div>

        <!-- Advanced Tab -->
        <div class="settings-tab-content" data-settings-content="advanced">
          <div class="settings-field">
            <label class="settings-label">默认命令</label>
            <input type="text" id="settingsCmd" class="np-input np-mono"
                   value="${esc(currentConfig?.default_cmd || 'claude')}" autocomplete="off" spellcheck="false">
          </div>
          <div class="settings-field">
            <label class="settings-label">默认参数</label>
            <input type="text" id="settingsArgs" class="np-input np-mono"
                   value="${esc((currentConfig?.default_args || []).join(' '))}"
                   placeholder="--permission-mode bypassPermissions" autocomplete="off" spellcheck="false">
          </div>
          <div class="settings-field">
            <label class="settings-label">最大终端数</label>
            <input type="number" id="settingsMaxTerms" class="np-input np-mono"
                   value="${currentConfig?.max_terminals || 10}" min="1" max="50">
          </div>
          <div class="settings-field">
            <label class="settings-label">缓冲区大小 (MB)</label>
            <input type="number" id="settingsBuffer" class="np-input np-mono"
                   value="${currentConfig?.ring_buffer_max_mb || 10}" min="1" max="100">
          </div>
          <div class="settings-hint" style="margin-top:8px;font-size:11px">重启终端后生效</div>
        </div>

        <!-- Footer -->
        <div class="settings-footer">
          <span class="settings-save-status" id="settingsSaveStatus"></span>
          <button class="np-btn np-btn-confirm" id="settingsSaveBtn">保存</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    requestAnimationFrame(() => overlay.classList.add('on'));

    this._overlay = overlay;
    this._wireEvents();
  }

  _wireEvents() {
    const overlay = this._overlay;
    if (!overlay) return;

    // Close button
    overlay.querySelector('.settings-close').addEventListener('click', () => this.close());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) this.close(); });

    // Escape key
    overlay.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') this.close();
    });

    // Tab switching
    overlay.querySelectorAll('.settings-tab').forEach(btn => {
      btn.addEventListener('click', () => {
        const tabId = btn.dataset.settingsTab;
        overlay.querySelectorAll('.settings-tab').forEach(t => t.classList.toggle('active', t.dataset.settingsTab === tabId));
        overlay.querySelectorAll('.settings-tab-content').forEach(c => c.classList.toggle('active', c.dataset.settingsContent === tabId));
      });
    });

    // Backend selector change → show install hint for uninstalled
    const backendSelect = overlay.querySelector('#settingsBackend');
    const hintEl = overlay.querySelector('#settingsBackendHint');
    backendSelect.addEventListener('change', () => {
      const beId = backendSelect.value;
      const beInfo = backends.find(b => b.id === beId);
      if (beInfo && !beInfo.installed) {
        hintEl.innerHTML = `📦 请先安装: <span class="np-mono">${esc(beInfo.install_hint)}</span>`;
        hintEl.style.display = '';
      } else {
        hintEl.style.display = 'none';
      }
    });

    // Eye toggle for API key
    overlay.querySelector('#settingsEyeBtn').addEventListener('click', () => {
      const inp = overlay.querySelector('#settingsApiKey');
      inp.type = inp.type === 'password' ? 'text' : 'password';
    });

    // Save
    overlay.querySelector('#settingsSaveBtn').addEventListener('click', () => this._save());

    // Enter to save
    overlay.querySelectorAll('input, select').forEach(el => {
      el.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') this._save();
      });
    });

    // Focus first input
    setTimeout(() => {
      const firstInput = overlay.querySelector('#settingsBackend');
      if (firstInput) firstInput.focus();
    }, 100);
  }

  async _save() {
    const overlay = this._overlay;
    const statusEl = overlay.querySelector('#settingsSaveStatus');

    const cfg = {
      default_cmd: overlay.querySelector('#settingsCmd')?.value?.trim() || 'claude',
      default_args: (overlay.querySelector('#settingsArgs')?.value || '').split(/\s+/).filter(Boolean),
      max_terminals: parseInt(overlay.querySelector('#settingsMaxTerms')?.value) || 10,
      ring_buffer_max_mb: parseInt(overlay.querySelector('#settingsBuffer')?.value) || 10,
      api_key: overlay.querySelector('#settingsApiKey')?.value?.trim() || '',
      base_url: overlay.querySelector('#settingsBaseUrl')?.value?.trim() || 'https://api.anthropic.com',
      model: overlay.querySelector('#settingsModel')?.value?.trim() || '',
      active_backend: overlay.querySelector('#settingsBackend')?.value || 'claude-code',
    };

    try {
      const merged = { ...currentConfig, ...cfg };
      await ipc.saveConfig(merged);
      statusEl.textContent = '✓ 已保存';
      statusEl.style.color = 'var(--em-bright)';
      setTimeout(() => this.close(), 800);
    } catch (e) {
      console.warn('[settings] save failed:', e);
      statusEl.textContent = '✗ 保存失败';
      statusEl.style.color = '#ef4444';
    }
  }

  _dismiss() {
    if (!this._overlay) return;
    this._overlay.classList.remove('on');
    this._overlay.classList.add('dismissing');
    setTimeout(() => {
      if (this._overlay) {
        this._overlay.remove();
        this._overlay = null;
      }
    }, 200);
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}
