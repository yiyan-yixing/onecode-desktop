// 应用入口：初始化 TabManager + 全局快捷键 + 状态栏。

import { TabManager } from './terminal/tab-manager.js';

const tabManager = new TabManager();

function init() {
  tabManager.init();
  tabManager.onChange = updateStatusbar;
  initKeybindings(tabManager);
}

function updateStatusbar(tm) {
  const countEl = document.getElementById('statusCount');
  const textEl = document.getElementById('statusText');
  if (countEl) countEl.textContent = `${tm.tabs.size} 终端`;
  const active = tm.activeId ? tm.tabs.get(tm.activeId) : null;
  if (textEl && active) {
    textEl.textContent = active.status === 'exited'
      ? `${active.label} · 已退出`
      : active.label;
  }
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
