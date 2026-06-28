// Setup Wizard — 3 步引导：环境检测 → 后端 + API 配置 → 完成
//
// 状态机：WELCOME → CHECKING → ENV_RESULT → CONFIG_FORM → SAVING → DONE / ERROR
// 挂载为全屏 overlay，完成后淡出移除，主界面自然显现。

import * as ipc from './ipc-bridge.js';

// ── 状态枚举 ──────────────────────────────────────────────────────

const S = {
  WELCOME:     'WELCOME',
  CHECKING:    'CHECKING',
  ENV_RESULT:  'ENV_RESULT',
  CONFIG_FORM: 'CONFIG_FORM',
  SAVING:      'SAVING',
  DONE:        'DONE',
  ERROR:       'ERROR',
};

let state = S.WELCOME;
let envResult = null;       // CheckEnvironmentResult
let backends = [];          // BackendInfo[] from list_backends
let formData = { apiKey: '', baseUrl: 'https://api.anthropic.com', model: 'claude-sonnet-4-6', backend: 'claude-code' };
let errorMsg = '';
let resolveWizard = null;   // 外部 Promise 的 resolve

// Backend-specific form label/placeholder overrides
const BACKEND_FORM_CONFIG = {
  'claude-code': {
    apiKeyLabel: 'API Key',
    apiKeyPlaceholder: 'sk-ant-…',
    baseUrlDefault: 'https://api.anthropic.com',
    modelDefault: 'claude-sonnet-4-6',
    envKeyHint: 'ANTHROPIC_API_KEY',
  },
  'opencode': {
    apiKeyLabel: 'API Key',
    apiKeyPlaceholder: 'sk-…',
    baseUrlDefault: 'https://api.anthropic.com',
    modelDefault: 'claude-sonnet-4-6',
    envKeyHint: 'ANTHROPIC_API_KEY',
  },
};

function getBackendFormConfig(backendId) {
  return BACKEND_FORM_CONFIG[backendId] || BACKEND_FORM_CONFIG['claude-code'];
}

// ── 公共接口 ──────────────────────────────────────────────────────

/**
 * 初始化 Wizard（若首次启动则挂载 overlay）。
 * 返回 Promise：Wizard 完成时 resolve。
 */
export async function initWizard() {
  let firstRun;
  try {
    firstRun = await ipc.isFirstRun();
  } catch (e) {
    console.warn('[wizard] isFirstRun IPC failed, skip wizard (fail-open):', e);
    return; // fail-open：不阻塞用户
  }
  if (!firstRun) return;

  return new Promise((resolve) => {
    resolveWizard = resolve;
    mountOverlay();
    transition(S.WELCOME);
  });
}

/** 移除 Wizard overlay DOM */
export function destroyWizard() {
  const overlay = document.getElementById('wizardOverlay');
  if (overlay) {
    overlay.style.opacity = '0';
    overlay.style.transition = 'opacity 300ms ease-out';
    setTimeout(() => overlay.remove(), 320);
  }
}

// ── 状态转换 ──────────────────────────────────────────────────────

function transition(next, payload) {
  state = next;
  render(payload);
}

// ── DOM 挂载 ──────────────────────────────────────────────────────

function mountOverlay() {
  const overlay = document.createElement('div');
  overlay.id = 'wizardOverlay';
  overlay.className = 'wizard-overlay';
  overlay.innerHTML = `
    <div class="wizard-container">
      <div class="wizard-header">
        <div class="wizard-logo">OneCode</div>
        <div class="wizard-stepper">
          <span class="step" data-step="1">1 · 环境</span>
          <span class="step-sep">›</span>
          <span class="step" data-step="2">2 · 配置</span>
          <span class="step-sep">›</span>
          <span class="step" data-step="3">3 · 完成</span>
        </div>
      </div>
      <div class="wizard-body" id="wizardBody"></div>
      <div class="wizard-footer" id="wizardFooter"></div>
    </div>
  `;
  document.body.appendChild(overlay);
  // 入场动画
  requestAnimationFrame(() => overlay.style.opacity = '1');
}

// ── 渲染 ──────────────────────────────────────────────────────────

function render(payload) {
  const body = document.getElementById('wizardBody');
  const footer = document.getElementById('wizardFooter');
  const steps = document.querySelectorAll('#wizardOverlay .step');

  // 更新 stepper 高亮
  const stepMap = {
    [S.WELCOME]: 1, [S.CHECKING]: 1, [S.ENV_RESULT]: 1,
    [S.CONFIG_FORM]: 2, [S.SAVING]: 2,
    [S.DONE]: 3, [S.ERROR]: 2,
  };
  const activeStep = stepMap[state] || 1;
  steps.forEach(s => {
    const n = parseInt(s.dataset.step);
    s.classList.toggle('active', n === activeStep);
    s.classList.toggle('done', n < activeStep);
  });

  switch (state) {
    case S.WELCOME:
      renderWelcome(body, footer);
      setTimeout(() => transition(S.CHECKING), 800);
      break;
    case S.CHECKING:
      renderChecking(body, footer);
      runCheck();
      break;
    case S.ENV_RESULT:
      renderEnvResult(body, footer);
      break;
    case S.CONFIG_FORM:
      renderConfigForm(body, footer);
      break;
    case S.SAVING:
      renderSaving(body, footer);
      doSave();
      break;
    case S.DONE:
      renderDone(body, footer);
      setTimeout(() => {
        if (resolveWizard) resolveWizard();
        destroyWizard();
      }, 1500);
      break;
    case S.ERROR:
      renderError(body, footer, payload);
      break;
  }
}

// ── Step 1: 环境 ────────────────────────────────────────────────────

function renderWelcome(body, footer) {
  body.innerHTML = `
    <div class="wizard-welcome">
      <div class="wizard-welcome-icon">✦</div>
      <h2>Setting up OneCode</h2>
      <p>Checking your environment…</p>
    </div>
  `;
  footer.innerHTML = '';
}

function renderChecking(body, footer) {
  body.innerHTML = `
    <div class="wizard-checking">
      <div class="wizard-spinner"></div>
      <p>Detecting dependencies…</p>
    </div>
  `;
  footer.innerHTML = '';
}

async function runCheck() {
  try {
    const [envRes, backendRes] = await Promise.all([
      ipc.checkEnvironment(),
      ipc.listBackends(),
    ]);
    envResult = envRes;
    backends = backendRes || [];
    transition(S.ENV_RESULT);
  } catch (e) {
    console.warn('[wizard] checkEnvironment/listBackends failed:', e);
    envResult = {
      dependencies: [
        makeFakeDep('claude', 'Detection failed'),
        makeFakeDep('node', 'Detection failed'),
        makeFakeDep('git', 'Detection failed'),
      ],
      all_ok: false,
    };
    backends = [];
    transition(S.ENV_RESULT);
  }
}

function makeFakeDep(name, hint) {
  return { name, found: false, version: null, min_version: '', version_ok: false, install_hint: hint };
}

function renderEnvResult(body, footer) {
  const deps = envResult?.dependencies || [];
  const allOk = envResult?.all_ok;

  // Build backend status section
  const installedBackends = backends.filter(b => b.installed);
  const uninstalledBackends = backends.filter(b => !b.installed);

  let backendSection = '';
  if (backends.length > 0) {
    backendSection = `
      <div class="wizard-backend-section">
        <h3>AI 后端</h3>
        ${backends.map(b => `
          <div class="wizard-dep ${b.installed ? 'ok' : 'fail'}">
            <div class="wizard-dep-icon">
              ${b.installed ? '✓' : '✗'}
            </div>
            <div class="wizard-dep-info">
              <div class="wizard-dep-name">${escapeHtml(b.display_name)}</div>
              <div class="wizard-dep-ver">${b.installed ? `命令: ${escapeHtml(b.cmd)}` : '未安装'}</div>
              ${!b.installed ? `<div class="wizard-dep-hint">${escapeHtml(b.install_hint)}</div>` : ''}
            </div>
            ${!b.installed ? `
              <button class="wizard-copy-btn" data-hint="${escapeAttr(b.install_hint)}" title="复制安装命令">📋</button>
            ` : ''}
          </div>
        `).join('')}
      </div>
    `;
  }

  body.innerHTML = `
    <div class="wizard-env">
      <h2>环境检测</h2>
      <p class="wizard-env-subtitle">${allOk && installedBackends.length > 0 ? '✅ 所有依赖已就绪' : '部分依赖缺失，但不影响继续配置'}</p>
      <div class="wizard-deps">
        ${deps.map(dep => `
          <div class="wizard-dep ${dep.found && dep.version_ok ? 'ok' : dep.found ? 'warn' : 'fail'}">
            <div class="wizard-dep-icon">
              ${dep.found && dep.version_ok ? '✓' : dep.found ? '⚠' : '✗'}
            </div>
            <div class="wizard-dep-info">
              <div class="wizard-dep-name">${depName(dep.name)}${dep.min_version ? ` (≥ ${dep.min_version})` : ''}</div>
              <div class="wizard-dep-ver">${dep.found ? (dep.version ? `v${dep.version}` : '已安装') : '未找到'}</div>
              ${!dep.found || !dep.version_ok ? `<div class="wizard-dep-hint">${escapeHtml(dep.install_hint)}</div>` : ''}
            </div>
            ${!dep.found || !dep.version_ok ? `
              <button class="wizard-copy-btn" data-hint="${escapeAttr(dep.install_hint)}" title="复制安装命令">📋</button>
            ` : ''}
          </div>
        `).join('')}
      </div>
      ${backendSection}
    </div>
  `;

  // 复制按钮事件
  body.querySelectorAll('.wizard-copy-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const hint = btn.dataset.hint;
      navigator.clipboard.writeText(hint).then(() => {
        btn.textContent = '✓';
        setTimeout(() => btn.textContent = '📋', 1500);
      }).catch(() => {
        btn.textContent = '✗';
        setTimeout(() => btn.textContent = '📋', 1500);
      });
    });
  });

  footer.innerHTML = `
    <button class="wizard-btn-secondary" id="wizardRecheck">重新检测</button>
    <button class="wizard-btn-primary" id="wizardNext1">下一步</button>
  `;

  document.getElementById('wizardRecheck')?.addEventListener('click', () => transition(S.CHECKING));
  document.getElementById('wizardNext1')?.addEventListener('click', () => transition(S.CONFIG_FORM));
}

function depName(n) {
  return { claude: 'Claude Code CLI', node: 'Node.js', git: 'Git' }[n] || n;
}

// ── Step 2: Backend selector + API 配置 ────────────────────────────────

function renderConfigForm(body, footer) {
  const installed = backends.filter(b => b.installed);
  const uninstalled = backends.filter(b => !b.installed);
  const sortedBackends = [...installed, ...uninstalled];

  // Build backend selector
  let backendSelector = '';
  if (sortedBackends.length > 0) {
    const optionsHtml = sortedBackends.map(b => {
      const icon = b.installed ? '✓' : '✗';
      const disabled = b.installed ? '' : ' disabled';
      const selected = b.id === formData.backend ? ' selected' : '';
      return `<option value="${escapeAttr(b.id)}"${disabled}${selected}>${icon} ${escapeHtml(b.display_name)}</option>`;
    }).join('');
    backendSelector = `
      <div class="wizard-field">
        <label for="wizBackend">AI 后端</label>
        <select id="wizBackend" class="wizard-select">${optionsHtml}</select>
        <div class="wizard-field-error" id="wizBackendErr"></div>
      </div>
    `;
  }

  const beConfig = getBackendFormConfig(formData.backend);

  body.innerHTML = `
    <div class="wizard-config">
      <h2>API 配置</h2>
      <p class="wizard-config-subtitle">连接你的 AI 后端</p>
      <div class="wizard-form">
        ${backendSelector}
        <div class="wizard-field">
          <label for="wizApiKey">${escapeHtml(beConfig.apiKeyLabel)}</label>
          <div class="wizard-input-row">
            <input type="password" id="wizApiKey" value="${escapeAttr(formData.apiKey)}"
                   placeholder="${escapeAttr(beConfig.apiKeyPlaceholder)}" autocomplete="off" spellcheck="false">
            <button class="wizard-eye-btn" id="wizEyeBtn" title="显示/隐藏">👁</button>
          </div>
          <div class="wizard-field-error" id="wizApiKeyErr"></div>
        </div>
        <div class="wizard-field">
          <label for="wizBaseUrl">Base URL</label>
          <input type="text" id="wizBaseUrl" value="${escapeAttr(formData.baseUrl)}"
                 placeholder="${escapeAttr(beConfig.baseUrlDefault)}" autocomplete="off" spellcheck="false">
          <div class="wizard-field-error" id="wizBaseUrlErr"></div>
        </div>
        <div class="wizard-field">
          <label for="wizModel">Model</label>
          <input type="text" id="wizModel" value="${escapeAttr(formData.model)}"
                 placeholder="${escapeAttr(beConfig.modelDefault)}" autocomplete="off" spellcheck="false">
          <div class="wizard-field-error" id="wizModelErr"></div>
        </div>
      </div>
    </div>
  `;

  // Backend selector change → update form labels and defaults
  const backendSelect = document.getElementById('wizBackend');
  if (backendSelect) {
    backendSelect.addEventListener('change', () => {
      const newBackend = backendSelect.value;
      formData.backend = newBackend;
      const newConfig = getBackendFormConfig(newBackend);
      // Update placeholders and labels
      const apiKeyInput = document.getElementById('wizApiKey');
      const apiKeyLabel = body.querySelector('label[for="wizApiKey"]');
      const baseUrlInput = document.getElementById('wizBaseUrl');
      const modelInput = document.getElementById('wizModel');
      if (apiKeyLabel) apiKeyLabel.textContent = newConfig.apiKeyLabel;
      if (apiKeyInput) apiKeyInput.placeholder = newConfig.apiKeyPlaceholder;
      if (baseUrlInput && !formData.baseUrl) baseUrlInput.placeholder = newConfig.baseUrlDefault;
      if (modelInput && !formData.model) modelInput.placeholder = newConfig.modelDefault;
    });
  }

  // Eye toggle
  document.getElementById('wizEyeBtn')?.addEventListener('click', () => {
    const inp = document.getElementById('wizApiKey');
    if (inp) {
      inp.type = inp.type === 'password' ? 'text' : 'password';
    }
  });

  // Store form data on input
  ['wizApiKey', 'wizBaseUrl', 'wizModel'].forEach(id => {
    const el = document.getElementById(id);
    if (el) {
      el.addEventListener('input', () => {
        formData.apiKey = document.getElementById('wizApiKey')?.value || '';
        formData.baseUrl = document.getElementById('wizBaseUrl')?.value || '';
        formData.model = document.getElementById('wizModel')?.value || '';
      });
    }
  });

  footer.innerHTML = `
    <button class="wizard-btn-secondary" id="wizardBack1">上一步</button>
    <button class="wizard-btn-primary" id="wizardNext2">下一步</button>
  `;

  document.getElementById('wizardBack1')?.addEventListener('click', () => transition(S.ENV_RESULT));
  document.getElementById('wizardNext2')?.addEventListener('click', () => {
    if (validateForm()) {
      transition(S.SAVING);
    }
  });
}

function validateForm() {
  let valid = true;

  const apiKey = formData.apiKey.trim();
  const baseUrl = formData.baseUrl.trim();
  const model = formData.model.trim();
  const backend = formData.backend;

  // Backend must be installed
  const beInfo = backends.find(b => b.id === backend);
  if (beInfo && !beInfo.installed) {
    const beErr = document.getElementById('wizBackendErr');
    if (beErr) beErr.textContent = '请选择一个已安装的后端';
    valid = false;
  }

  // API Key
  const keyErr = document.getElementById('wizApiKeyErr');
  if (!apiKey) {
    if (keyErr) keyErr.textContent = 'API Key 不能为空';
    shakeField('wizApiKey');
    valid = false;
  } else {
    if (keyErr) keyErr.textContent = '';
  }

  // Base URL
  const urlErr = document.getElementById('wizBaseUrlErr');
  if (!baseUrl.startsWith('http://') && !baseUrl.startsWith('https://')) {
    if (urlErr) urlErr.textContent = 'URL 须以 http:// 或 https:// 开头';
    shakeField('wizBaseUrl');
    valid = false;
  } else {
    if (urlErr) urlErr.textContent = '';
  }

  // Model
  const modelErr = document.getElementById('wizModelErr');
  if (!model) {
    if (modelErr) modelErr.textContent = '模型名称不能为空';
    shakeField('wizModel');
    valid = false;
  } else {
    if (modelErr) modelErr.textContent = '';
  }

  return valid;
}

function shakeField(id) {
  const el = document.getElementById(id);
  if (el) {
    el.classList.add('shake');
    el.focus();
    setTimeout(() => el.classList.remove('shake'), 600);
  }
}

// ── Saving ──────────────────────────────────────────────────────────

function renderSaving(body, footer) {
  body.innerHTML = `
    <div class="wizard-checking">
      <div class="wizard-spinner"></div>
      <p>正在保存配置…</p>
    </div>
  `;
  footer.innerHTML = '';
}

async function doSave() {
  try {
    await ipc.saveWizardConfig({
      apiKey: formData.apiKey.trim(),
      baseUrl: formData.baseUrl.trim(),
      model: formData.model.trim(),
      backend: formData.backend,
    });
    // Clear credential from JS memory after successful save
    formData.apiKey = '';
    transition(S.DONE);
  } catch (e) {
    console.warn('[wizard] save failed:', e);
    transition(S.ERROR, e);
  }
}

// ── Step 3: 完成 ────────────────────────────────────────────────────

function renderDone(body, footer) {
  const beConfig = getBackendFormConfig(formData.backend);
  const beInfo = backends.find(b => b.id === formData.backend);
  const beDisplayName = beInfo ? beInfo.display_name : formData.backend;

  body.innerHTML = `
    <div class="wizard-done">
      <div class="wizard-done-icon">✓</div>
      <h2>配置完成</h2>
      <p>OneCode 已就绪，终端加载中…</p>
      <div class="wizard-done-summary">
        <span>后端: ${escapeHtml(beDisplayName)}</span>
        <span>·</span>
        <span>API: ${formData.baseUrl.includes('anthropic') ? 'Anthropic' : 'Custom'}</span>
        <span>·</span>
        <span>Model: ${escapeHtml(formData.model.trim())}</span>
      </div>
    </div>
  `;
  footer.innerHTML = '';
}

// ── Error ───────────────────────────────────────────────────────────

function renderError(body, footer, err) {
  body.innerHTML = `
    <div class="wizard-error">
      <div class="wizard-error-icon">✗</div>
      <h2>保存失败</h2>
      <p class="wizard-error-msg">${escapeHtml(err || '未知错误')}</p>
      <p class="wizard-error-hint">请检查 ~/.onecode/ 目录的写入权限</p>
    </div>
  `;
  footer.innerHTML = `
    <button class="wizard-btn-primary" id="wizardRetry">重试</button>
  `;
  document.getElementById('wizardRetry')?.addEventListener('click', () => transition(S.SAVING));
}

// ── 工具函数 ────────────────────────────────────────────────────────

function escapeHtml(s) {
  return s?.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;') || '';
}

function escapeAttr(s) {
  return s?.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/'/g, '&#39;') || '';
}
