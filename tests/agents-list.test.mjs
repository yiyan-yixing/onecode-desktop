// Agents 列表点击语义验证 —— **点击 = 派发新员工 Slot**，⌥/Ctrl/Cmd+点击 = 同会话点名。
//
// 用 mock 的 TabManager / MentionController 注入，避免真实 DOM/IPC 依赖。
// agents-list.js 不直接 import ipc-bridge.js（通过 DI 注入），测试可直接导入。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { AgentsListController } from '../src/agents-list.js';

/** 支持的 mock toast 元素（_showToast 会写入 .al-toast-msg.textContent） */
function makeToastEl() {
  const classes = new Set();
  const msgEl = { textContent: '' };
  return {
    className: 'al-toast',
    innerHTML: '',
    _msgEl: msgEl,
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
      _classes: classes,
    },
    querySelector: (sel) => (sel === '.al-toast-msg' ? msgEl : null),
  };
}

/**
 * 这些测试在无 DOM 的 Node 环境跑，而 `_showToast` 需要 document.createElement。
 * 装一个最小 stub（node --test 每个文件独立进程，无需还原）。
 */
function installDomStub() {
  if (globalThis.document && typeof globalThis.document.createElement === 'function') return;
  globalThis.document = globalThis.document || {};
  globalThis.document.createElement = () => makeToastEl();
}
installDomStub();

/**
 * 最小 mock：收集 createTab / sendInput 调用，模拟 TabManager。
 * 不调用 _buildUI()（需要 document），直接设置 _container 为 mock 对象。
 */
function makeMocks(agents = [], { hasActiveTerminal = true, isError = false } = {}) {
  const sent = [];
  const created = [];

  const mention = {
    active: false,
    sendInput: (s) => sent.push({ path: 'mention', data: s }),
    hide: () => {},
  };
  const term = { focus() {} };

  const activeId = hasActiveTerminal ? 'term-1' : null;
  const tabs = new Map();
  if (hasActiveTerminal) {
    tabs.set('term-1', {
      id: 'term-1',
      term,
      mention,
      isError,
      projectId: 'proj-1',
      cwd: '/tmp/proj-1',
      backend: 'claude-code',
      providerId: 'deepseek',
      model: 'deepseek-chat',
    });
  }
  const tabManager = {
    activeId,
    tabs,
    createTab: async (opts) => {
      created.push(opts);
      return 'new-id';
    },
  };

  const ptyWrite = (id, data) => {
    sent.push({ path: 'ptyWrite', id, data });
    return Promise.resolve();
  };

  const container = {
    _toastEl: null,
    appendChild(child) { this._toastEl = child; },
    querySelector(sel) {
      if (sel === '.al-toast' && this._toastEl) return this._toastEl;
      return null;
    },
  };

  const ctrl = new AgentsListController();
  ctrl._container = container;
  ctrl._tabManager = tabManager;
  ctrl.setPtyWrite(ptyWrite);
  ctrl.setProvider(() => agents);

  return { ctrl, sent, created, container, tabManager, mention, term, ptyWrite };
}

const AGENTS = [
  { id: 'arch', name: 'Architect', description: 'designs', color: '#ff0000', icon: '', scope: 'project' },
  { id: 'qa', name: 'QA', description: 'tests', color: '#00ff00', icon: '', scope: 'global' },
  { id: 'dev', name: 'DevOps', description: 'ships', color: '#0000ff', icon: '🚀', scope: 'project' },
];

describe('@name 派发：点击 agent → 新开一个以该 agent 为身份的 Slot', () => {
  test('点击 → createTab({ agent }) 被调用，且**不**往当前终端打字', async () => {
    const { ctrl, created, sent } = makeMocks(AGENTS);

    await ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(created.length, 1);
    assert.equal(created[0].agent, 'arch');
    // 关键：派发不是打字
    assert.equal(sent.length, 0, '派发路径不应向 PTY 写入 @name 文本');
  });

  test('派发继承当前项目的 projectId / cwd / backend（终端按项目组织）', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('qa', mockItemEl());

    assert.equal(created[0].projectId, 'proj-1');
    assert.equal(created[0].cwd, '/tmp/proj-1');
    assert.equal(created[0].backend, 'claude-code');
  });

  test('派发继承当前项目的路由（供应商/模型），新员工与项目同源', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('qa', mockItemEl());

    assert.equal(created[0].providerId, 'deepseek');
    assert.equal(created[0].model, 'deepseek-chat');
  });

  test('标签用 agent 名称（有 icon 时带上），便于在标签栏分辨员工', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(created[0].label, 'Architect');

    ctrl._lastClickTime = 0;
    await ctrl._onAgentClick('dev', mockItemEl());
    assert.equal(created[1].label, '🚀 DevOps');
  });

  test('agents 数据里查不到时，标签回退到 @id（不抛异常）', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('ghost', mockItemEl());
    assert.equal(created[0].label, '@ghost');
    assert.equal(created[0].agent, 'ghost');
  });

  test('派发后 itemEl 有视觉反馈', async () => {
    const { ctrl } = makeMocks(AGENTS);
    const itemEl = mockItemEl();
    await ctrl._onAgentClick('arch', itemEl);
    assert.equal(itemEl._classList.has('al-mention-flash'), true);
  });

  test('createTab 抛错 → 提示 toast，不产生未捕获 rejection', async () => {
    const { ctrl, container } = makeMocks(AGENTS);
    ctrl._tabManager.createTab = async () => { throw new Error('boom'); };
    const origWarn = console.warn;
    console.warn = () => {};
    try {
      await ctrl._onAgentClick('arch', mockItemEl());
    } finally {
      console.warn = origWarn;
    }
    assert.equal(container.querySelector('.al-toast').classList.contains('on'), true);
  });
});

describe('@name 派发：缺少项目上下文时如实提示，不悄悄退回打字', () => {
  test('无活跃终端 → toast，且不 createTab、不写 PTY', async () => {
    const { ctrl, created, sent, container } = makeMocks(AGENTS, { hasActiveTerminal: false });

    await ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(created.length, 0);
    assert.equal(sent.length, 0);
    assert.equal(container.querySelector('.al-toast').classList.contains('on'), true);
  });

  test('活跃终端是 error tab → toast（没有可用项目上下文）', async () => {
    const { ctrl, created, container } = makeMocks(AGENTS, { isError: true });

    await ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(created.length, 0);
    assert.equal(container.querySelector('.al-toast').classList.contains('on'), true);
  });

  test('没有 TabManager → toast，不抛异常', async () => {
    const ctrl = new AgentsListController();
    ctrl._container = {
      _toastEl: null,
      appendChild(c) { this._toastEl = c; },
      querySelector(sel) { return sel === '.al-toast' ? this._toastEl : null; },
    };
    const origCreate = globalThis.document?.createElement;
    globalThis.document = globalThis.document || {};
    globalThis.document.createElement = () => makeToastEl();
    try {
      await ctrl._onAgentClick('arch', mockItemEl());
      assert.equal(ctrl._container.querySelector('.al-toast').classList.contains('on'), true);
    } finally {
      if (origCreate) globalThis.document.createElement = origCreate;
      else delete globalThis.document.createElement;
    }
  });
});

describe('@name 派发：防抖 — 300ms 内重复点击只派发一次', () => {
  test('第二次点击被忽略', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('arch', mockItemEl());
    await ctrl._onAgentClick('qa', mockItemEl());

    assert.equal(created.length, 1);
  });

  test('300ms 后第二次点击正常派发', async () => {
    const { ctrl, created } = makeMocks(AGENTS);

    await ctrl._onAgentClick('arch', mockItemEl());
    ctrl._lastClickTime = Date.now() - 400;
    await ctrl._onAgentClick('qa', mockItemEl());

    assert.equal(created.length, 2);
    assert.equal(created[1].agent, 'qa');
  });
});

describe('⌥/Ctrl/Cmd + 点击：同会话点名（保留 @mention 插入行为）', () => {
  const MOD = { insertMention: true };

  test('走 mention.sendInput 写入 @id', () => {
    const { ctrl, sent } = makeMocks(AGENTS);
    ctrl._onAgentClick('arch', mockItemEl(), MOD);

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'mention');
    assert.equal(sent[0].data, '@arch ');
  });

  test('不创建新 Slot', async () => {
    const { ctrl, created } = makeMocks(AGENTS);
    await ctrl._onAgentClick('arch', mockItemEl(), MOD);
    assert.equal(created.length, 0);
  });

  test('mention 弹窗活跃时先 hide() 再 sendInput', () => {
    const { ctrl, sent, mention } = makeMocks(AGENTS);
    mention.active = true;
    let hidden = false;
    mention.hide = () => { hidden = true; };

    ctrl._onAgentClick('arch', mockItemEl(), MOD);

    assert.equal(hidden, true);
    assert.equal(sent[0].data, '@arch ');
  });

  test('mention 为 null → 回退到 _ptyWrite', () => {
    const sent = [];
    const tabs = new Map();
    tabs.set('term-1', {
      id: 'term-1', term: { focus() {} }, mention: null, isError: false,
      projectId: 'p', cwd: '/tmp', backend: 'claude-code',
    });
    const ctrl = new AgentsListController();
    ctrl._tabManager = { activeId: 'term-1', tabs, createTab: async () => 'x' };
    ctrl._ptyWrite = (id, data) => { sent.push({ path: 'ptyWrite', id, data }); return Promise.resolve(); };

    ctrl._onAgentClick('dev', mockItemEl(), MOD);

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'ptyWrite');
    assert.equal(sent[0].data, '@dev ');
    assert.equal(sent[0].id, 'term-1');
  });

  test('mention.sendInput 抛异常 → 回退到 _ptyWrite（且仍有视觉反馈）', () => {
    const sent = [];
    const mention = { active: false, sendInput: () => { throw new Error('broken'); } };
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term: { focus() {} }, mention, isError: false });
    const ctrl = new AgentsListController();
    ctrl._tabManager = { activeId: 'term-1', tabs, createTab: async () => 'x' };
    ctrl._ptyWrite = (id, data) => { sent.push({ path: 'ptyWrite', id, data }); return Promise.resolve(); };

    const origWarn = console.warn;
    console.warn = () => {};
    let itemEl;
    try {
      itemEl = mockItemEl();
      ctrl._onAgentClick('arch', itemEl, MOD);
    } finally {
      console.warn = origWarn;
    }

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'ptyWrite');
    assert.equal(itemEl._classList.has('al-mention-flash'), true);
  });

  test('mention 与 _ptyWrite 都不可用 → 不写入、不闪烁（避免误导）', () => {
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term: { focus() {} }, mention: {}, isError: false });
    const ctrl = new AgentsListController();
    ctrl._tabManager = { activeId: 'term-1', tabs, createTab: async () => 'x' };
    // _ptyWrite 未注入

    const itemEl = mockItemEl();
    ctrl._onAgentClick('arch', itemEl, MOD);

    assert.equal(itemEl._classList.has('al-mention-flash'), false);
  });

  test('无活跃终端 → toast，不写入', () => {
    const { ctrl, sent, container } = makeMocks(AGENTS, { hasActiveTerminal: false });
    ctrl._onAgentClick('arch', mockItemEl(), MOD);

    assert.equal(sent.length, 0);
    assert.equal(container.querySelector('.al-toast').classList.contains('on'), true);
  });
});

describe('AgentsListController 初始化与刷新', () => {
  test('setTabManager / setPtyWrite 注入生效', () => {
    const { ctrl } = makeMocks(AGENTS);
    assert.equal(ctrl._tabManager !== null, true);
    assert.equal(typeof ctrl._ptyWrite, 'function');
  });

  test('_terminalStateChanged 初始为 true（确保首次渲染）', () => {
    const ctrl = new AgentsListController();
    assert.equal(ctrl._terminalStateChanged, true);
  });

  test('markTerminalStateChanged 置 true', () => {
    const ctrl = new AgentsListController();
    ctrl._terminalStateChanged = false;
    ctrl.markTerminalStateChanged();
    assert.equal(ctrl._terminalStateChanged, true);
  });

  test('refresh()：终端状态变更时重渲染（即使 agents 未变）', () => {
    const agents = [{ id: 'dev', name: 'Dev', description: '', color: '#000', icon: '', scope: 'project' }];
    let renderCount = 0;
    const ctrl = new AgentsListController();
    ctrl._provider = () => agents;
    ctrl._agents = agents;
    ctrl._terminalStateChanged = true;
    ctrl._listEl = { innerHTML: '', appendChild() {} };
    ctrl._renderList = () => { renderCount++; };

    ctrl.refresh();
    assert.equal(renderCount, 1);
    assert.equal(ctrl._terminalStateChanged, false);
  });

  test('refresh()：数据与状态都未变时跳过渲染', () => {
    const agents = [{ id: 'dev', name: 'Dev', description: '', color: '#000', icon: '', scope: 'project' }];
    let renderCount = 0;
    const ctrl = new AgentsListController();
    ctrl._provider = () => agents;
    ctrl._agents = agents;
    ctrl._terminalStateChanged = false;
    ctrl._listEl = {};
    ctrl._renderList = () => { renderCount++; };

    ctrl.refresh();
    assert.equal(renderCount, 0);
  });
});

describe('_showToast — toast 生命周期', () => {
  test('首次调用创建 toast 并显示，文案写入 .al-toast-msg', () => {
    const container = {
      _toastEl: null,
      appendChild(c) { this._toastEl = c; },
      querySelector(sel) { return sel === '.al-toast' ? this._toastEl : null; },
    };
    const mockToast = makeToastEl();
    const origCreate = globalThis.document?.createElement;
    globalThis.document = globalThis.document || {};
    globalThis.document.createElement = () => mockToast;

    try {
      const ctrl = new AgentsListController();
      ctrl._container = container;
      ctrl._showToast('测试文案');
      assert.equal(container._toastEl, mockToast);
      assert.equal(mockToast.classList.contains('on'), true);
      assert.equal(mockToast._msgEl.textContent, '测试文案');
    } finally {
      if (origCreate) globalThis.document.createElement = origCreate;
      else delete globalThis.document.createElement;
    }
  });

  test('已有 toast 时不重复创建', () => {
    let createCount = 0;
    const existing = makeToastEl();
    const container = {
      _toastEl: existing,
      appendChild() { createCount++; },
      querySelector(sel) { return sel === '.al-toast' ? existing : null; },
    };

    const ctrl = new AgentsListController();
    ctrl._container = container;
    ctrl._showToast('再来一次');

    assert.equal(createCount, 0);
    assert.equal(existing._msgEl.textContent, '再来一次');
  });
});

/** Mock item element with classList tracking (Set-based, robust) */
function mockItemEl() {
  const classes = new Set();
  const el = { _classList: classes };
  el.classList = {
    add(cls) { classes.add(cls); },
    remove(cls) { classes.delete(cls); },
    contains(cls) { return classes.has(cls); },
  };
  return el;
}
