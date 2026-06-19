// 应用入口：会话恢复 + 多终端 Tab + CC Status 徽章 + 全局快捷键 + 生命周期接线。

import { TabManager } from './terminal/tab-manager.js';
import { CcStatusView } from './cc-status.js';
import * as ipc from './ipc-bridge.js';

const tabManager = new TabManager();
let ccView = null;

async function init() {
  tabManager.init();
  tabManager.onChange = updateStatusbar;
  initKeybindings(tabManager);

  // CC Status 徽章 + @mention agent 数据源
  ccView = new CcStatusView({
    badgeRoot: document.getElementById('ccBadges'),
    getProjectDir: () => tabManager.getActiveCwd(),
  });
  ccView.onAgents = (agents) => {
    tabManager.agentProvider = () => agents;
  };
  ccView.start();

  // 启动恢复（无记录则建默认 main）
  try {
    const slots = await ipc.sessionRestore();
    await tabManager.restoreOrInit(slots);
  } catch (e) {
    console.warn('[session] restore failed, fallback to default', e);
    await tabManager.restoreOrInit(null);
  }

  // 生命周期 / 托盘事件
  ipc.onTrayNewTerminal(() => tabManager.createTab());
  ipc.onAppBeforeQuit(() => tabManager.persistNow());
  ipc.onHealthReport((reports) => showHealthWarning(reports));
}

function updateStatusbar(tm) {
  const countEl = document.getElementById('statusCount');
  const textEl = document.getElementById('statusText');
  if (countEl) countEl.textContent = `${tm.tabs.size} 终端`;
  const active = tm.activeId ? tm.tabs.get(tm.activeId) : null;
  if (textEl && active) {
    textEl.textContent =
      active.status === 'exited'
        ? `${active.label} · 已退出`
        : active.label;
  }
}

function showHealthWarning(reports) {
  const el = document.getElementById('healthWarn');
  if (!el) return;
  const warn = reports.find(
    (r) => r.action === 'warn' || r.action === 'stale' || r.action === 'kill',
  );
  if (!warn) {
    el.classList.remove('on');
    el.textContent = '';
    return;
  }
  const tip =
    warn.action === 'warn'
      ? `${shortLabel(warn)} RSS 偏高`
      : warn.action === 'kill'
        ? `${shortLabel(warn)} 进程僵尸`
        : `${shortLabel(warn)} 进程异常`;
  el.textContent = '⚠ ' + tip;
  el.classList.add('on');
}

function shortLabel(r) {
  return r.pid ? `#${r.pid}` : '终端';
}

function initKeybindings(tm) {
  const mod = navigator.platform.includes('Mac') ? 'metaKey' : 'ctrlKey';
  window.addEventListener('keydown', (e) => {
    if (!e[mod]) return;
    const key = e.key.toLowerCase();
    if (key === 't') {
      e.preventDefault();
      tm.createTab();
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
