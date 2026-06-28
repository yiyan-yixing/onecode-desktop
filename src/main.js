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
import { ThemeManager } from './theme.js';
import { AmbientController } from './ambient.js';
import { initWizard, destroyWizard } from './wizard.js';
import * as ipc from './ipc-bridge.js';
const tabManager = new TabManager();
const orbital = new OrbitalController();
const palette = new PaletteController();
const ripple = new RippleController();
const fileExplorer = new FileExplorerController();
const themeManager = new ThemeManager();
const ambientController = new AmbientController();
let ccView = null;

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
    '<span class="ipc-disconnect-msg">⚠ 连接断开 — 后端服务不可用</span>' +
    '<button class="ipc-disconnect-dismiss" title="关闭">✕</button>';
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
  // Theme (dark-only Aurora paradigm)
  themeManager.init();

  // Ambient: fade statusbar after 8s idle
  ambientController.init();

  // Tab + Orbital + Palette
  tabManager.init();
  tabManager.orbital = orbital;
  tabManager.ripple = ripple;
  orbital.init(tabManager);
  orbital.setFileExplorer(fileExplorer);
  palette.init(tabManager);
  ripple.init();
  tabManager.onChange = (tm) => {
    updateStatusbar(tm);
    orbital._loadProjects(); // refresh project list (hides cards with no terminals)
    // 同步文件浏览器 cwd
    if (orbital._activeTab === 'files' && fileExplorer) {
      fileExplorer.syncCwd(tm.getActiveCwd());
    }
    // 切换标签页/目录时刷新 CC Status 徽章
    if (ccView) ccView.refresh();
  };

  initKeybindings(tabManager);

  // CC Status — badges in statusbar
  ccView = new CcStatusView({
    badgeRoot: document.getElementById('ribbonBadges'),
    getProjectDir: () => tabManager.getActiveCwd(),
    getActiveBackend: () => tabManager.getActiveBackend(),
  });
  ccView.onAgents = (agents) => {
    tabManager.agentProvider = () => agents;
    palette.setAgentProvider(() => agents);
  };
  ccView.onStatus = (data) => {
    // Also update statusbar badges
    updateStatusbarBadges(data);
  };
  ccView.start();

  // Wizard gate — 首次启动引导（controller 初始化后、session restore 前）
  try {
    await initWizard();   // blocks until wizard completes (skips if not first run)
  } catch (e) {
    console.warn('[wizard] failed, proceeding to main interface:', e);
  }

  // Empty orb click → create first terminal
  document.getElementById('emptyOrb')?.addEventListener('click', () => {
    const orb = document.getElementById('emptyOrb');
    if (orb) orb.classList.add('exploding');
    setTimeout(() => tabManager.createTab(), 400);
  });

  // Sidebar toggle button in titlebar
  document.getElementById('sidebarToggle')?.addEventListener('click', () => {
    const sidebar = document.getElementById('orbital');
    if (sidebar) sidebar.classList.toggle('collapsed');
  });

  // Session restore
  try {
    const slots = await ipc.sessionRestore();
    await tabManager.restoreOrInit(slots);
  } catch (e) {
    console.warn('[session] restore failed, fallback to default', e);
    await tabManager.restoreOrInit(null);
  }

  // Lifecycle
  ipc.onTrayNewTerminal(() => tabManager.createTab());
  ipc.onAppBeforeQuit(() => tabManager.persistNow());
  ipc.onHealthReport((reports) => showHealthWarning(reports));
}

function updateStatusbar(tm) {
  const countEl = document.getElementById('ribbonCount');
  const statusEl = document.getElementById('ribbonStatus');
  if (countEl) countEl.textContent = `${tm.tabs.size} terminals`;
  const active = tm.activeId ? tm.tabs.get(tm.activeId) : null;
  if (statusEl && active) {
    statusEl.textContent = active.status === 'exited'
      ? `${active.label} · exited`
      : active.label;
  }
}

function updateStatusbarBadges(data) {
  const root = document.getElementById('ribbonBadges');
  if (!root) return;
  const items = [
    { key: 'skills', count: (data.skills || []).length, icon: ICONS.skills },
    { key: 'hooks', count: data.hooks ? Object.values(data.hooks).reduce((a, v) => a + v.length, 0) : 0, icon: ICONS.hooks },
    { key: 'plugins', count: (data.plugins || []).length, icon: ICONS.plugins },
    { key: 'tasks', count: (data.tasks || []).length, icon: ICONS.tasks },
  ];
  const keys = ['skills', 'hooks', 'plugins', 'tasks'];
  for (const t of keys) {
    const badge = root.querySelector(`[data-cc="${t}"]`);
    const n = items.find(i => i.key === t)?.count || 0;
    if (badge) {
      const numEl = badge.querySelector('.n');
      if (numEl && numEl.textContent !== String(n)) numEl.textContent = n;
      badge.classList.toggle('has', n > 0);
    }
  }
  if (!root.querySelector('[data-cc]')) {
    root.innerHTML = items.map(it =>
      `<span class="ribbon-badge ${it.count > 0 ? 'has' : ''}" data-cc="${it.key}">${it.icon}<span class="n">${it.count}</span></span>`
    ).join('');
  }
}

const ICONS = {
  skills: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><path d="M8 1l2 4 4.5.7-3.3 3.1.8 4.5L8 11l-4 2.3.8-4.5L1.5 5.7 6 5z"/></svg>',
  hooks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><path d="M6 2v4l-3 3v2h4v3l1 2 1-2v-3h4V9l-3-3V2"/><rect x="5" y="1" width="6" height="2" rx="1"/></svg>',
  plugins: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>',
  tasks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:10px;height:10px"><circle cx="8" cy="8" r="6"/><polyline points="8 4 8 8 11 9.5"/></svg>',
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
  el.textContent = '⚠ ' + tip;
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
    // Cmd+B → toggle sidebar
    if (e[mod] && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      const sidebar = document.getElementById('orbital');
      if (sidebar) sidebar.classList.toggle('collapsed');
      return;
    }
    // Cmd+Shift+F → switch to Files tab
    if (e[mod] && e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      orbital.switchToFiles();
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
      if (tm.activeId) tm.closeTab(tm.activeId);
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

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}
