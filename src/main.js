// 应用入口：Cowork Shell + Orbital + Palette + CC Status + 全局快捷键 + 生命周期接线。

// Dev-mode console noise filter: suppress harmless Tauri IPC warnings that
// appear during hot-reload (callback id mismatch, custom protocol fallback).
// These are inherent to dev mode and never occur in production builds.
if (typeof window !== 'undefined') {
  const _origWarn = console.warn;
  console.warn = function (...args) {
    const msg = args.join(' ');
    if (
      msg.includes("Couldn't find callback id") ||  // hot-reload: old IPC promises orphaned
      msg.includes('IPC custom protocol failed')    // WKWebView: auto-fallback to postMessage
    ) return;
    _origWarn.apply(this, args);
  };
}

import { TabManager } from './terminal/tab-manager.js';
import { OrbitalController } from './orbital.js';
import { PaletteController } from './palette.js';
import { RippleController } from './ripple.js';
import { CcStatusView } from './cc-status.js';
import { FileExplorerController } from './file-explorer.js';
import { AgentsListController } from './agents-list.js';
import { ThemeManager } from './theme.js';
import { AmbientController } from './ambient.js';
import { initWizard, destroyWizard } from './wizard.js';
import { SettingsController } from './settings.js';
import * as ipc from './ipc-bridge.js';
const tabManager = new TabManager();
const orbital = new OrbitalController();
const palette = new PaletteController();
const ripple = new RippleController();
const fileExplorer = new FileExplorerController();
const agentsList = new AgentsListController();
const settings = new SettingsController();
const themeManager = new ThemeManager();
const ambientController = new AmbientController();
let ccView = null;

// ── Right panel Tab state ──────────────────────────────────────────
let _activeRightTab;
try { _activeRightTab = localStorage.getItem('fe-active-tab') || 'file'; } catch { _activeRightTab = 'file'; }

function switchRightTab(tabId) {
  _activeRightTab = 'file'; // P1-6: right panel only has file tab

  // Update fileExplorer visibility based on panel open state
  const panel = document.getElementById('filePanel');
  const panelOpen = panel && !panel.classList.contains('collapsed');
  fileExplorer.setVisible(panelOpen);
}

// ── P1-13: Backend disconnected banner ──────────────────────────────
let _disconnectBanner = null;

function showDisconnectBanner() {
  if (_disconnectBanner) return; // already shown
  const viewport = document.getElementById('termViewport');
  if (!viewport) return;
  const banner = document.createElement('div');
  banner.id = 'ipcDisconnectBanner';
  banner.className = 'ipc-disconnect-banner';
  banner.innerHTML =
    '<span class="ipc-disconnect-msg"><svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" style="width:12px;height:12px;vertical-align:-2px;margin-right:4px"><circle cx="7" cy="7" r="6"/><line x1="7" y1="4" x2="7" y2="7"/><line x1="7" y1="9" x2="7.01" y2="9"/></svg>连接断开 — 后端服务不可用</span>' +
    '<button class="ipc-disconnect-dismiss" title="关闭"><svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" style="width:10px;height:10px"><line x1="3" y1="3" x2="9" y2="9"/><line x1="9" y1="3" x2="3" y2="9"/></svg></button>';
  banner.querySelector('.ipc-disconnect-dismiss').addEventListener('click', () => {
    dismissDisconnectBanner();
  });
  viewport.prepend(banner);
  _disconnectBanner = banner;
}

function dismissDisconnectBanner() {
  if (_disconnectBanner) {
    _disconnectBanner.remove();
    _disconnectBanner = null;
  }
}

window.addEventListener('ipc-disconnected', () => {
  console.warn('[ipc] backend disconnected — consecutive IPC failures reached threshold');
  showDisconnectBanner();
});

window.addEventListener('ipc-reconnected', () => {
  console.info('[ipc] backend reconnected');
  dismissDisconnectBanner();
});

async function init() {
  // Theme (warm-shell + dark-terminal paradigm)
  themeManager.init();

  // Ambient: fade statusbar after 8s idle
  ambientController.init();

  // Tab + Orbital + Palette
  tabManager.init();
  tabManager.orbital = orbital;
  tabManager.ripple = ripple;
  orbital.init(tabManager);
  palette.init(tabManager);
  ripple.init();

  // 监听配置文件外部变更（agent 直接修改后热刷新）
  ipc.onConfigChanged(() => {
    settings.reloadConfig();
    updateStatusbarConfig();
  });
  tabManager.onChange = (tm) => {
    updateStatusbar(tm);
    // 切 tab 时只更新 active 状态，不触发全量 DOM 重建（避免闪烁）
    orbital.setActive(tm.activeId);
    // 定期刷新项目列表（项目增删时），但不阻塞切换体验
    orbital._loadProjects();
    // 同步文件浏览器 cwd（右侧面板展开且 file tab 活跃时）
    const filePanel = document.getElementById('filePanel');
    const panelOpen = filePanel && !filePanel.classList.contains('collapsed');
    if (panelOpen && _activeRightTab === 'file' && fileExplorer) {
      fileExplorer.syncCwd(tm.getActiveCwd());
    }
    // 切换标签页/目录时刷新 CC Status 徽章
    if (ccView) ccView.refresh();
    // 切换标签页时刷新后端配置显示
    updateStatusbarConfig();
    // P1-1 fix: 标记终端状态变更，确保下次切到 agents tab 时 disabled 状态正确更新
    agentsList.markTerminalStateChanged();
    // P1-6: 切换项目时刷新左侧栏 agents 列表
    if (panelOpen) {
      orbital.refreshAgents();
    }
  };

  initKeybindings(tabManager);
  wireMenuEvents(tabManager);

  // CC Status — badges in statusbar
  ccView = new CcStatusView({
    badgeRoot: document.getElementById('ribbonBadges'),
    getProjectDir: () => tabManager.getActiveCwd(),
    getActiveBackend: () => tabManager.getActiveBackend(),
  });
  ccView.onAgents = (agents) => {
    tabManager.agentProvider = () => agents;
    palette.setAgentProvider(() => agents);
    palette.setOpenSettings(() => settings.open());
    // Also update agents list controller (right panel, kept for transition)
    agentsList.setProvider(() => agents);
    agentsList.refresh();
    // P1-6: Also provide agents to left sidebar
    orbital.setAgentProvider(() => agents);
    orbital.refreshAgents();
  };
  ccView.onStatus = (data) => {
    // Also update statusbar badges
    updateStatusbarBadges(data);
  };
  ccView.start();
  updateStatusbarConfig();

  // Wizard gate — 首次启动引导（controller 初始化后、session restore 前）
  let wizardResult = null;
  try {
    wizardResult = await initWizard();   // blocks until wizard completes (returns config or undefined if skipped)
  } catch (e) {
    console.warn('[wizard] failed, proceeding to main interface:', e);
  }

  // 首次体验合并（P1-5）：wizard 完成后自动创建第一个终端
  const isFirstRun = !!wizardResult;
  if (isFirstRun) {
    // wizard 刚完成，跳过空状态、直接创建首个终端
    try {
      const homeDir = await ipc.getHomeDir();
      const projectDir = `${homeDir}/my-project`;
      const tabId = await tabManager.createTab({
        label: 'my-project',
        cwd: projectDir,
        projectId: 'my-project',
        backend: wizardResult.backend || null,
      });
      // 等终端就绪后写入欢迎语
      setTimeout(async () => {
        if (tabId) {
          ipc.ptyWrite(tabId, `echo "\\n🎉 欢迎使用 OneCode！这是你的第一个 AI 员工终端。\\n输入 @dev 召唤 AI 开发者，或输入 claude 开始对话。\\n"\r`);
        }
      }, 2000);
    } catch (e) {
      console.warn('[wizard] auto-create first terminal failed:', e);
    }
  }

  // Empty state — orb / 新建终端 / 命令面板 共用创建入口；最近项目卡片
  const emptyOrb = document.getElementById('emptyOrb');
  const emptyNewTerm = document.getElementById('emptyNewTerm');
  const emptyPalette = document.getElementById('emptyPalette');

  // 点击 orb 或「新建终端」按钮：爆裂动画 → 创建首个终端
  const createFirstTerminal = () => {
    if (emptyOrb) emptyOrb.classList.add('exploding');
    setTimeout(() => tabManager.createTab(), 400);
  };
  emptyOrb?.addEventListener('click', createFirstTerminal);
  emptyNewTerm?.addEventListener('click', createFirstTerminal);
  // 「⌘K 命令面板」次按钮：明确第二条入口
  emptyPalette?.addEventListener('click', () => palette.toggle());

  // 最近项目卡片（复用 ipc.listProjects，取最近 3 个）
  renderEmptyRecent();
  // 项目增删后刷新空状态卡片
  orbital.onProjectsChanged = () => renderEmptyRecent();

  // Sidebar toggle button in titlebar
  document.getElementById('sidebarToggle')?.addEventListener('click', () => toggleSidebar());

  // Right panel: init file explorer
  const filePanel = document.getElementById('filePanel');
  const feTabFile = document.getElementById('feTabFile');

  // FileExplorerController renders into the file tab content container
  fileExplorer.init(feTabFile, tabManager);

  // P1-6: Agents 数据注入到左侧栏 orbital
  if (ccView && ccView.agents && ccView.agents.length > 0) {
    orbital.setAgentProvider(() => ccView.agents);
    orbital.refreshAgents();
  } else if (tabManager.agentProvider) {
    orbital.setAgentProvider(tabManager.agentProvider);
  }

  // Wire tab bar click handlers
  const tabButtons = filePanel.querySelectorAll('.fe-tab');
  tabButtons.forEach((btn) => {
    btn.addEventListener('click', () => {
      switchRightTab(btn.dataset.tab);
    });
  });

  // Restore last active tab
  switchRightTab(_activeRightTab);

  // Right panel toggle button in titlebar
  const rpToggle = document.getElementById('rightPanelToggle');
  rpToggle?.addEventListener('click', () => toggleRightPanel());

  // Session restore
  try {
    const slots = await ipc.sessionRestore();
    await tabManager.restoreOrInit(slots);
  } catch (e) {
    console.warn('[session] restore failed, fallback to default', e);
    await tabManager.restoreOrInit(null);
  }

  // 配置状态栏
  updateStatusbarConfig();

  // Lifecycle
  ipc.onTrayNewTerminal(() => tabManager.createTab());
  ipc.onAppBeforeQuit(() => tabManager.closeAllTabs());
  ipc.onHealthReport((reports) => showHealthWarning(reports));
}

function updateStatusbar(tm) {
  const countEl = document.getElementById('ribbonCount');
  const statusEl = document.getElementById('ribbonStatus');
  if (countEl) countEl.textContent = `${tm.tabs.size} 个终端`;
  const active = tm.activeId ? tm.tabs.get(tm.activeId) : null;
  if (statusEl && active) {
    statusEl.textContent = active.status === 'exited'
      ? `${active.label} · 已退出`
      : active.label;
  } else if (statusEl && !active) {
    statusEl.textContent = '就绪';
  }
}

// 空状态「最近项目」卡片：取最近 3 个项目，点击即为其创建终端。
async function renderEmptyRecent() {
  const root = document.getElementById('emptyRecent');
  if (!root) return;
  let projects = [];
  try {
    projects = (await ipc.listProjects()) || [];
  } catch (e) {
    console.warn('[empty] list projects failed', e);
  }
  // 按最近活跃排序（有 last_active 的优先），取前 3
  const recent = projects.slice(0, 3);
  if (recent.length === 0) {
    root.innerHTML =
      `<button class="recent-card recent-card-empty" id="emptyNewProject">` +
      `<span class="recent-card-icon"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" style="width:13px;height:13px"><rect x="2" y="3" width="12" height="10" rx="2"/><line x1="8" y1="6" x2="8" y2="10"/><line x1="6" y1="8" x2="10" y2="8"/></svg></span>` +
      `<span class="recent-card-name">新建第一个项目</span></button>`;
    document.getElementById('emptyNewProject')?.addEventListener('click', () => {
      // 复用 orbital 的「新建项目」对话框
      orbital._newProject();
    });
    return;
  }
  root.innerHTML = recent.map((proj, i) => {
    const initial = esc((proj.name || '?')[0].toUpperCase());
    const colors = ['#10B981', '#A78BFA', '#22D3EE', '#F472B6', '#F7C948', '#7DD3FC'];
    const c = colors[i % colors.length];
    const dirTail = proj.dir ? proj.dir.split('/').pop() : '';
    return `<button class="recent-card" data-name="${esc(proj.name)}">` +
      `<span class="recent-card-icon" style="background:${c}22;color:${c}">${esc(initial)}</span>` +
      `<span class="recent-card-meta">` +
      `<span class="recent-card-name">${esc(proj.name)}</span>` +
      (dirTail ? `<span class="recent-card-dir">${esc(dirTail)}</span>` : '') +
      `</span></button>`;
  }).join('');
  root.querySelectorAll('.recent-card').forEach((card) => {
    card.addEventListener('click', () => {
      const name = card.dataset.name;
      const proj = recent.find((p) => p.name === name);
      if (!proj) return;
      const projId = proj.id || proj.name;
      tabManager.createTab({
        label: proj.name,
        cwd: proj.dir,
        projectId: projId,
        backend: proj.backend || null,
      });
    });
  });
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function updateStatusbarBadges(data) {
  const root = document.getElementById('ribbonBadges');
  if (!root) return;
  const items = [
    { key: 'skills', count: (data.skills || []).length, icon: ICONS.skills },
    { key: 'hooks', count: data.hooks ? Object.values(data.hooks).reduce((a, v) => a + v.length, 0) : 0, icon: ICONS.hooks },
    { key: 'plugins', count: (data.plugins || []).length, icon: ICONS.plugins },
    { key: 'tasks', count: (data.tasks || []).length, icon: ICONS.tasks },
    { key: 'statusline', count: (data.statusline || []).length, icon: ICONS.statusline },
  ];
  const keys = ['skills', 'hooks', 'plugins', 'tasks', 'statusline'];
  for (const t of keys) {
    const badge = root.querySelector(`[data-cc="${t}"]`);
    const item = items.find(i => i.key === t);
    const n = item?.count || 0;
    if (badge) {
      const numEl = badge.querySelector('.n');
      if (numEl && numEl.textContent !== String(n)) numEl.textContent = n;
      badge.classList.toggle('has', n > 0);
      const title = t === 'statusline' && n > 0 ? 'statusline: 已配置' : `${t}: ${n}`;
      badge.setAttribute('title', title);
    }
  }
  if (!root.querySelector('[data-cc]')) {
    root.innerHTML = items.map(it =>
      `<span class="ribbon-badge ${it.count > 0 ? 'has' : ''}" data-cc="${it.key}" title="${it.key}: ${it.count}">${it.icon}<span class="n">${it.count}</span></span>`
    ).join('');
  }
}

/** 状态栏后端 + 模型配置显示 */
async function updateStatusbarConfig() {
  const labelEl = document.getElementById('ribbonBackendLabel');
  const modelEl = document.getElementById('ribbonModelLabel');
  const root = document.getElementById('ribbonBackend');
  if (!labelEl || !root) return;
  try {
    const cfg = await ipc.loadConfig();
    const backend = cfg.active_backend || 'claude-code';
    const model = cfg.model || '';
    const BACKEND_SHORT = { 'claude-code': 'CC', 'opencode': 'OC', 'codex': 'CX' };
    labelEl.textContent = BACKEND_SHORT[backend] || backend;
    modelEl.textContent = model;
    root.dataset.backend = backend;
  } catch (e) {
    labelEl.textContent = '--';
    modelEl.textContent = '';
    root.dataset.backend = '';
  }
}

const ICONS = {
  skills: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><path d="M8 1l2 4 4.5.7-3.3 3.1.8 4.5L8 11l-4 2.3.8-4.5L1.5 5.7 6 5z"/></svg>',
  hooks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><path d="M6 2v4l-3 3v2h4v3l1 2 1-2v-3h4V9l-3-3V2"/><rect x="5" y="1" width="6" height="2" rx="1"/></svg>',
  plugins: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>',
  tasks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><circle cx="8" cy="8" r="6"/><polyline points="8 4 8 8 11 9.5"/></svg>',
  statusline: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><polyline points="2 4 6 8 2 12"/><polyline points="6 4 14 4 14 12 6 12"/></svg>',
};

function showHealthWarning(reports) {
  const el = document.getElementById('healthWarn');
  if (!el) return;
  const warn = reports.find(r => r.action === 'warn' || r.action === 'stale' || r.action === 'kill');
  if (!warn) { el.classList.remove('on'); el.textContent = ''; return; }
  const tip = warn.action === 'warn'
    ? `#${warn.pid || '?'} RSS 偏高`
    : warn.action === 'kill'
      ? `#${warn.pid || '?'} 进程僵尸`
      : `#${warn.pid || '?'} 进程异常`;
  el.innerHTML = '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" style="width:12px;height:12px;vertical-align:-2px;margin-right:3px"><circle cx="7" cy="7" r="6"/><line x1="7" y1="4" x2="7" y2="7"/><line x1="7" y1="9" x2="7.01" y2="9"/></svg>' + tip;
  el.classList.add('on');
}

function initKeybindings(tm) {
  const mod = navigator.platform.includes('Mac') ? 'metaKey' : 'ctrlKey';
  window.addEventListener('keydown', (e) => {
    // Cmd+K → toggle palette
    if (e[mod] && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      palette.toggle();
      return;
    }
    // Cmd+, → settings panel (P1-4)
    if (e[mod] && e.key === ',') {
      e.preventDefault();
      settings.toggle();
      return;
    }
    // Cmd+B → toggle sidebar
    if (e[mod] && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      toggleSidebar();
      return;
    }
    // Cmd+Shift+F → open right panel and switch to file tab
    if (e[mod] && e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      toggleRightPanel(true);
      return;
    }
    if (!e[mod]) return;
    const key = e.key.toLowerCase();

    // ★ 修复: 非 Mac 平台 Ctrl+W/T 在终端内不应被应用截获
    // 终端内这些快捷键应发送到 PTY（Ctrl+W = 删除前一个词, Ctrl+T = 交换字符）
    // 只在非终端焦点时才拦截
    if (!navigator.platform.includes('Mac')) {
      const activeTerm = tm.activeId ? tm.tabs.get(tm.activeId) : null;
      const xtermTa = activeTerm?.term?.textarea;
      if (xtermTa && (document.activeElement === xtermTa ||
          xtermTa.contains(document.activeElement))) {
        // 终端 textarea 有焦点 — 不拦截，让按键传到 PTY
        return;
      }
    }

    if (key === 't') {
      e.preventDefault();
      if (e.shiftKey) {
        // Shift+Cmd+T → New Chat in current project context
        const cwd = tm.getActiveCwd() || undefined;
        tm.createTab({ cwd });
      } else {
        // Cmd+T → New Chat (default cwd)
        tm.createTab();
      }
      return;
    }
    if (key === 'w') {
      e.preventDefault();
      if (tm.activeId) tm.closeTab(tm.activeId); // _closing 标记防止快速连按双重 close
      return;
    }
    const n = parseInt(e.key, 10);
    if (n >= 1 && n <= 9) {
      e.preventDefault();
      tm.switchByIndex(n - 1);
      return;
    }
    if (e.shiftKey && (e.key === '[' || e.key === ']')) {
      e.preventDefault();
      tm.switchByOffset(e.key === '[' ? -1 : 1);
    }
  });
}

// ── 共享视图动作（菜单栏 + 快捷键 + 标题栏按钮共用）──

function toggleSidebar() {
  const sidebar = document.getElementById('orbital');
  const btn = document.getElementById('sidebarToggle');
  if (!sidebar) return;
  sidebar.classList.toggle('collapsed');
  const isOpen = !sidebar.classList.contains('collapsed');
  btn?.classList.toggle('on', isOpen);
  btn?.setAttribute('aria-expanded', String(isOpen));
}

function toggleRightPanel(forceOpen) {
  const panel = document.getElementById('filePanel');
  if (!panel) return;
  if (forceOpen) panel.classList.remove('collapsed');
  else panel.classList.toggle('collapsed');
  const isOpen = !panel.classList.contains('collapsed');
  const rpBtn = document.getElementById('rightPanelToggle');
  rpBtn?.classList.toggle('on', isOpen);
  if (isOpen) {
    fileExplorer.setVisible(true);
    fileExplorer.syncCwd(tabManager.getActiveCwd());
  } else {
    fileExplorer.setVisible(false);
  }
}

// ── 应用菜单栏事件（menu.rs emit `menu:<id>`，P0-4）──
function wireMenuEvents(tm) {
  ipc.onMenuEvent('new', () => tm.createTab());
  ipc.onMenuEvent('new-in-project', () => tm.createTab({ cwd: tm.getActiveCwd() || undefined }));
  ipc.onMenuEvent('close', () => { if (tm.activeId) tm.closeTab(tm.activeId); });
  ipc.onMenuEvent('toggle-sidebar', () => toggleSidebar());
  ipc.onMenuEvent('toggle-right-panel', () => toggleRightPanel(true));
  for (let n = 1; n <= 9; n++) {
    ipc.onMenuEvent(`switch-${n}`, () => tm.switchByIndex(n - 1));
  }
  ipc.onMenuEvent('help', () => showHelpOverlay());
}

// 快捷键说明弹窗（帮助菜单入口）
function showHelpOverlay() {
  const existing = document.getElementById('helpOverlay');
  if (existing) { existing.remove(); return; }
  const rows = [
    ['⌘K', '命令面板'],
    ['⌘T', '新建终端'],
    ['⇧⌘T', '项目内新建终端'],
    ['⌘W', '关闭终端'],
    ['⌘B', '侧栏'],
    ['⇧⌘F', '右侧面板'],
    ['⌘1-9', '切换到终端 1-9'],
    ['⌘[ / ⌘]', '上一个 / 下一个终端'],
    ['⌘Q', '退出 OneCode'],
  ];
  const overlay = document.createElement('div');
  overlay.id = 'helpOverlay';
  overlay.className = 'np-overlay';
  overlay.innerHTML =
    `<div class="np-dialog" style="max-width:420px">` +
      `<div class="np-header"><div class="np-title">快捷键说明</div>` +
      `<button class="np-close" title="关闭">✕</button></div>` +
      `<div class="np-body">` +
        rows.map(([k, d]) =>
          `<div style="display:flex;justify-content:space-between;align-items:center;padding:7px 0;border-bottom:1px solid var(--sand)">` +
          `<span style="color:var(--tx-warm2)">${esc(d)}</span>` +
          `<kbd style="font-family:var(--font-mono);font-size:11px;color:var(--tx-warm);background:var(--cream);border:1px solid var(--sand);border-radius:4px;padding:2px 7px">${esc(k)}</kbd>` +
          `</div>`).join('') +
      `</div>` +
    `</div>`;
  document.body.appendChild(overlay);
  requestAnimationFrame(() => overlay.classList.add('on'));
  const close = () => {
    overlay.classList.remove('on');
    overlay.classList.add('dismissing');
    setTimeout(() => overlay.remove(), 200);
  };
  overlay.querySelector('.np-close').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  window.addEventListener('keydown', function onEsc(e) {
    if (e.key === 'Escape') { close(); window.removeEventListener('keydown', onEsc); }
  });
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}
