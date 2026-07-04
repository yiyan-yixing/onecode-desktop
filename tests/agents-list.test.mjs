// F2: Agents 列表点击 → @mention 插入到活跃终端 验证
// 用 mock 的 TabManager / MentionController 注入，避免真实 DOM/IPC 依赖。
// agents-list.js 不直接 import ipc-bridge.js（通过 DI 注入），测试可直接导入。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { AgentsListController } from '../src/agents-list.js';

/**
 * 最小 mock：收集 sendInput 调用，模拟 TabManager。
 * 不调用 _buildUI()（需要 document），直接设置 _container 为 mock 对象。
 */
function makeMocks(agents = [], hasActiveTerminal = true) {
  const sent = [];

  // Mock mention controller with sendInput
  const mention = {
    sendInput: (s) => sent.push({ path: 'mention', data: s }),
  };

  // Mock terminal
  const term = { focus() {} };

  // Mock TabManager
  const activeId = hasActiveTerminal ? 'term-1' : null;
  const tabs = new Map();
  if (hasActiveTerminal) {
    tabs.set('term-1', {
      id: 'term-1',
      term,
      mention,
      isError: false,
    });
  }
  const tabManager = {
    activeId,
    tabs,
  };

  // Mock ptyWrite fallback
  const ptyWrite = (id, data) => {
    sent.push({ path: 'ptyWrite', id, data });
    return Promise.resolve();
  };

  // Mock container (minimal, no document.createElement needed)
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
  ctrl.setTabManager(hasActiveTerminal ? tabManager : { activeId: null, tabs: new Map() });
  ctrl.setPtyWrite(ptyWrite);
  ctrl.setProvider(() => agents);

  return { ctrl, sent, container, tabManager, mention, term, ptyWrite };
}

const AGENTS = [
  { id: 'arch', name: 'Architect', description: 'designs', color: '#ff0000', icon: '', scope: 'project' },
  { id: 'qa', name: 'QA', description: 'tests', color: '#00ff00', icon: '', scope: 'global' },
  { id: 'dev', name: 'DevOps', description: 'ships', color: '#0000ff', icon: '🚀', scope: 'project' },
];

describe('F2: AgentsListController 初始化', () => {
  test('setTabManager 注入 TabManager 引用', () => {
    const { ctrl } = makeMocks(AGENTS);
    assert.equal(ctrl._tabManager !== null, true);
  });

  test('setPtyWrite 注入 ptyWrite 回调', () => {
    const { ctrl } = makeMocks(AGENTS);
    assert.equal(typeof ctrl._ptyWrite, 'function');
  });

  test('_terminalStateChanged 初始为 true（确保首次渲染）', () => {
    const ctrl = new AgentsListController();
    assert.equal(ctrl._terminalStateChanged, true);
  });
});

describe('F2: 点击 agent → @mention 插入（主路径: mention.sendInput）', () => {
  test('点击 agent 项 → 通过 mention.sendInput 写入 @agent-id', () => {
    const { ctrl, sent } = makeMocks(AGENTS);

    ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'mention');
    assert.equal(sent[0].data, '@arch ');
  });

  test('点击不同 agent 写入不同的 @id', () => {
    const { ctrl, sent } = makeMocks(AGENTS);

    ctrl._onAgentClick('qa', mockItemEl());
    assert.equal(sent[0].data, '@qa ');
  });

  test('点击后终端获得焦点（term.focus 被调用）', () => {
    const { ctrl, term } = makeMocks(AGENTS);
    let focused = false;
    term.focus = () => { focused = true; };
    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(focused, true);
  });

  test('点击后 itemEl 添加 al-mention-flash 类', () => {
    const { ctrl } = makeMocks(AGENTS);
    const itemEl = mockItemEl();
    ctrl._onAgentClick('arch', itemEl);
    assert.equal(itemEl._classList.has('al-mention-flash'), true);
  });

  test('mention 弹窗活跃时 → 先 hide() 再 sendInput', () => {
    const { ctrl, sent, mention } = makeMocks(AGENTS);
    mention.active = true;
    let hideCalled = false;
    mention.hide = () => { hideCalled = true; };

    ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(hideCalled, true);
    assert.equal(sent.length, 1);
    assert.equal(sent[0].data, '@arch ');
  });

  test('mention 弹窗不活跃时 → 不调用 hide()', () => {
    const { ctrl, sent, mention } = makeMocks(AGENTS);
    mention.active = false;
    let hideCalled = false;
    mention.hide = () => { hideCalled = true; };

    ctrl._onAgentClick('arch', mockItemEl());

    assert.equal(hideCalled, false);
    assert.equal(sent.length, 1);
  });
});

describe('F2: 兜底路径 — mention 不可用时走 ptyWrite', () => {
  test('mention 为 null → 通过 _ptyWrite 写入', () => {
    const sent = [];
    const term = { focus() {} };
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term, mention: null, isError: false });
    const tabManager = { activeId: 'term-1', tabs };
    const ptyWrite = (id, data) => {
      sent.push({ path: 'ptyWrite', id, data });
      return Promise.resolve();
    };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;
    ctrl._ptyWrite = ptyWrite;

    ctrl._onAgentClick('dev', mockItemEl());

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'ptyWrite');
    assert.equal(sent[0].data, '@dev ');
    assert.equal(sent[0].id, 'term-1');
  });

  test('mention.sendInput 不存在 → 通过 _ptyWrite 写入', () => {
    const sent = [];
    const term = { focus() {} };
    const mention = {}; // 无 sendInput
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term, mention, isError: false });
    const tabManager = { activeId: 'term-1', tabs };
    const ptyWrite = (id, data) => {
      sent.push({ path: 'ptyWrite', id, data });
      return Promise.resolve();
    };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;
    ctrl._ptyWrite = ptyWrite;

    ctrl._onAgentClick('qa', mockItemEl());

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'ptyWrite');
    assert.equal(sent[0].data, '@qa ');
  });

  test('mention 不可用且 _ptyWrite 也未注入 → 静默不写入且不闪烁', () => {
    const term = { focus() {} };
    const mention = {};
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term, mention, isError: false });
    const tabManager = { activeId: 'term-1', tabs };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;
    // _ptyWrite 未注入（默认 null）

    const itemEl = mockItemEl();
    // 不应抛出异常
    ctrl._onAgentClick('arch', itemEl);
    // QA P1-3 fix: 无写入时不闪烁，避免误导
    assert.equal(itemEl._classList.has('al-mention-flash'), false);
  });

  test('mention.sendInput 抛异常 → 回退到 _ptyWrite 写入', () => {
    const sent = [];
    const term = { focus() {} };
    const mention = {
      sendInput: () => { throw new Error('mention broken'); },
      active: false,
    };
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term, mention, isError: false });
    const tabManager = { activeId: 'term-1', tabs };
    const ptyWrite = (id, data) => {
      sent.push({ path: 'ptyWrite', id, data });
      return Promise.resolve();
    };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;
    ctrl._ptyWrite = ptyWrite;

    // 不应抛出异常，应回退到 ptyWrite
    const itemEl = mockItemEl();
    ctrl._onAgentClick('arch', itemEl);

    assert.equal(sent.length, 1);
    assert.equal(sent[0].path, 'ptyWrite');
    assert.equal(sent[0].data, '@arch ');
    // 回退写入后仍有视觉反馈
    assert.equal(itemEl._classList.has('al-mention-flash'), true);
  });

  test('mention.sendInput 抛异常且无 _ptyWrite → 不闪烁', () => {
    const term = { focus() {} };
    const mention = {
      sendInput: () => { throw new Error('mention broken'); },
      active: false,
    };
    const tabs = new Map();
    tabs.set('term-1', { id: 'term-1', term, mention, isError: false });
    const tabManager = { activeId: 'term-1', tabs };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;
    // _ptyWrite 未注入

    // Suppress console.warn in this test
    const origWarn = console.warn;
    console.warn = () => {};

    try {
      const itemEl = mockItemEl();
      ctrl._onAgentClick('arch', itemEl);
      assert.equal(itemEl._classList.has('al-mention-flash'), false);
    } finally {
      console.warn = origWarn;
    }
  });
});

describe('F2: 防抖 — 快速连续点击', () => {
  test('300ms 内第二次点击被忽略', () => {
    const { ctrl, sent } = makeMocks(AGENTS);

    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(sent.length, 1);

    // 立即第二次点击（< 300ms）
    ctrl._onAgentClick('qa', mockItemEl());
    assert.equal(sent.length, 1); // 第二次被忽略
  });

  test('300ms 后第二次点击正常处理', () => {
    const { ctrl, sent } = makeMocks(AGENTS);

    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(sent.length, 1);

    // 模拟时间过了 300ms（直接修改 _lastClickTime，绕过真实计时器）
    ctrl._lastClickTime = Date.now() - 400;
    ctrl._onAgentClick('qa', mockItemEl());
    assert.equal(sent.length, 2);
    assert.equal(sent[1].data, '@qa ');
  });
});

describe('F2: 无活跃终端时显示提示', () => {
  test('无 TabManager → 调用 _showNoTerminalToast', () => {
    const ctrl = new AgentsListController();
    assert.equal(ctrl._tabManager, null);

    let toastShown = false;
    ctrl._showNoTerminalToast = () => { toastShown = true; };
    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(toastShown, true);
  });

  test('有活跃终端 → 不显示 toast', () => {
    const { ctrl } = makeMocks(AGENTS);
    let toastShown = false;
    ctrl._showNoTerminalToast = () => { toastShown = true; };
    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(toastShown, false);
  });

  test('活跃终端为 error tab → 显示 toast', () => {
    const sent = [];
    const term = { focus() {} };
    const mention = { sendInput: (s) => sent.push(s) };
    const tabs = new Map();
    tabs.set('err-1', { id: 'err-1', term, mention, isError: true });
    const tabManager = { activeId: 'err-1', tabs };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;

    let toastShown = false;
    ctrl._showNoTerminalToast = () => { toastShown = true; };
    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(toastShown, true);
    assert.equal(sent.length, 0); // 不应写入 PTY
  });

  test('activeId 为 null → 显示 toast', () => {
    const tabManager = { activeId: null, tabs: new Map() };

    const ctrl = new AgentsListController();
    ctrl._tabManager = tabManager;

    let toastShown = false;
    ctrl._showNoTerminalToast = () => { toastShown = true; };
    ctrl._onAgentClick('arch', mockItemEl());
    assert.equal(toastShown, true);
  });
});

describe('F2: markTerminalStateChanged — P1-1 fix', () => {
  test('markTerminalStateChanged 设置 _terminalStateChanged 为 true', () => {
    const ctrl = new AgentsListController();
    ctrl._terminalStateChanged = false;
    ctrl.markTerminalStateChanged();
    assert.equal(ctrl._terminalStateChanged, true);
  });

  test('refresh() 在 _terminalStateChanged=true 时重新渲染（即使 agents 数据未变）', () => {
    const agents = [{ id: 'dev', name: 'Dev', description: '', color: '#000', icon: '', scope: 'project' }];
    let renderCount = 0;
    const ctrl = new AgentsListController();
    ctrl._provider = () => agents;
    ctrl._agents = agents; // 数据未变
    ctrl._terminalStateChanged = true;
    ctrl._listEl = { innerHTML: '', appendChild() {} };
    ctrl._renderList = () => { renderCount++; };

    ctrl.refresh();
    assert.equal(renderCount, 1);
    assert.equal(ctrl._terminalStateChanged, false); // 重置
  });

  test('refresh() 在 _terminalStateChanged=false 且 agents 数据未变时跳过渲染', () => {
    const agents = [{ id: 'dev', name: 'Dev', description: '', color: '#000', icon: '', scope: 'project' }];
    let renderCount = 0;
    const ctrl = new AgentsListController();
    ctrl._provider = () => agents;
    ctrl._agents = agents; // 数据未变
    ctrl._terminalStateChanged = false;
    ctrl._listEl = {};
    ctrl._renderList = () => { renderCount++; };

    ctrl.refresh();
    assert.equal(renderCount, 0);
  });
});

describe('F2: _showNoTerminalToast — toast 生命周期', () => {
  test('首次调用 → 创建 toast 元素并显示', () => {
    const container = { _toastEl: null, appendChild(child) { this._toastEl = child; }, querySelector() { return null; } };
    const ctrl = new AgentsListController();
    ctrl._container = container;

    // Stub document.createElement for toast creation
    const mockToast = {
      className: 'al-toast',
      innerHTML: '',
      classList: {
        _classes: new Set(),
        add(cls) { this._classes.add(cls); },
        remove(cls) { this._classes.delete(cls); },
        contains(cls) { return this._classes.has(cls); },
      },
    };

    const origCreate = globalThis.document?.createElement;
    globalThis.document = globalThis.document || {};
    globalThis.document.createElement = () => mockToast;

    try {
      ctrl._showNoTerminalToast();
      assert.equal(container._toastEl, mockToast);
      assert.equal(mockToast.classList._classes.has('on'), true);
    } finally {
      if (origCreate) {
        globalThis.document.createElement = origCreate;
      } else {
        delete globalThis.document.createElement;
      }
    }
  });

  test('连续调用 → toast 只创建一次', () => {
    let createCount = 0;
    const mockToast = {
      className: 'al-toast',
      innerHTML: '',
      classList: {
        _classes: new Set(),
        add(cls) { this._classes.add(cls); },
        remove(cls) { this._classes.delete(cls); },
        contains(cls) { return this._classes.has(cls); },
      },
    };

    const container = {
      _toastEl: mockToast, // 已有 toast
      appendChild(child) { createCount++; this._toastEl = child; },
      querySelector(sel) {
        if (sel === '.al-toast' && this._toastEl) return this._toastEl;
        return null;
      },
    };

    const ctrl = new AgentsListController();
    ctrl._container = container;

    ctrl._showNoTerminalToast();
    assert.equal(createCount, 0); // 已有 toast，不重复创建
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
