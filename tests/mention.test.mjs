// @mention 控制器验证 —— 测真实模块 src/terminal/mention.js（零构建改动）。
// 用 mock 的 term / termEl / popEl 注入，避免真实 DOM/xterm 依赖。
// Node 24 + package.json type:module → 可直接 import 真实 ESM 源文件。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { MentionController } from '../src/terminal/mention.js';

// 最小 mock：收集 sendInput、记录 popEl 状态。
function makeMocks(agents = []) {
  const sent = [];
  const term = { onData() {}, focus() {} };
  const termEl = { addEventListener() {} };
  const classList = new Set();
  let items = [];
  const popEl = {
    _html: '',
    classList: {
      add: (c) => classList.add(c),
      remove: (c) => classList.delete(c),
      contains: (c) => classList.has(c),
      toggle: (c, on) => { on ? classList.add(c) : classList.delete(c); },
    },
    set innerHTML(v) { this._html = v; },
    get innerHTML() { return this._html; },
    querySelectorAll: () => items,
    _setItems(arr) { items = arr; },
  };
  const ctrl = new MentionController({
    term, termEl, popEl,
    sendInput: (s) => sent.push(s),
    getAgents: () => agents,
  });
  return { ctrl, sent, popEl, classList };
}

const AGENTS = [
  { id: 'arch', name: 'Architect', description: 'designs', color: '#ff0000', icon: '' },
  { id: 'qa', name: 'QA', description: 'tests', color: '', icon: '' },
  { id: 'dev', name: 'DevOps', description: 'ships', color: '#00ff00', icon: '🚀' },
  { id: 'a.b_c-d', name: 'DotAgent', description: 'has separators', color: '#111', icon: '' },
];

describe('_onInput 状态机', () => {
  test('@ 触发 active', () => {
    const { ctrl } = makeMocks(AGENTS);
    assert.equal(ctrl.active, false);
    ctrl._onInput('@');
    assert.equal(ctrl.active, true);
    assert.equal(ctrl.prefix, '');
  });
  test('@ 后累积词字符', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@ar');
    assert.equal(ctrl.prefix, 'ar');
    assert.equal(ctrl.active, true);
  });
  test('非词字符收尾 → hide', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@abc def');
    assert.equal(ctrl.active, false); // 空格触发 hide
    assert.equal(ctrl.prefix, '');
  });
  test('PREFIX_CHARS 容纳 . _ - 与字母数字（用含分隔符的 agent 验证累积）', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@a.b_c-d');
    assert.equal(ctrl.prefix, 'a.b_c-d');
    assert.equal(ctrl.active, true);
  });
  test('未 active 时 @ 之外的输入不激活', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('ls -la');
    assert.equal(ctrl.active, false);
  });
  test('逐字符处理多字符 data（onData 一次可发多字符）', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@');
    ctrl._onInput('q');
    assert.equal(ctrl.prefix, 'q');
  });
});

describe('matches 过滤（id/name startswith，大小写不敏感）', () => {
  test('前缀匹配 id', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@ar');
    assert.deepEqual(ctrl.matches.map((a) => a.id), ['arch']);
  });
  test('前缀匹配 name（DevOps → dev 前缀不命中 name，dev 命中 name）', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@dev');
    // id 'dev' 命中；name 'DevOps' startswith 'dev' (lowercased) 也命中 → 同一项
    assert.deepEqual(ctrl.matches.map((a) => a.id), ['dev']);
  });
  test('大小写不敏感', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@AR');
    assert.deepEqual(ctrl.matches.map((a) => a.id), ['arch']);
  });
  test('无匹配 → 空（直接置 prefix 测 getter；经 _onInput 会因无匹配触发 hide）', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl.prefix = 'zzz';
    assert.equal(ctrl.matches.length, 0);
  });
  test('空 getAgents → 安全返回空', () => {
    const { ctrl } = makeMocks(null);
    ctrl._onInput('@a');
    assert.equal(ctrl.matches.length, 0);
  });
});

describe('_select — backspace 清前缀 + 插入 @id', () => {
  test('prefix="ar" → 3 个 backspace + "@arch "', () => {
    const { ctrl, sent } = makeMocks(AGENTS);
    ctrl._onInput('@ar'); // prefix='ar'
    ctrl._select({ dataset: { agent: 'arch' } });
    assert.deepEqual(sent, ['\x7f\x7f\x7f', '@arch ']);
  });
  test('prefix=""（仅 @）→ 1 个 backspace + "@id "', () => {
    const { ctrl, sent } = makeMocks(AGENTS);
    ctrl._onInput('@'); // prefix=''
    ctrl._select({ dataset: { agent: 'qa' } });
    assert.deepEqual(sent, ['\x7f', '@qa ']);
  });
  test('选中后 active 关闭、prefix 清空', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl._onInput('@ar');
    ctrl._select({ dataset: { agent: 'arch' } });
    assert.equal(ctrl.active, false);
    assert.equal(ctrl.prefix, '');
  });
  test('dataset.agent 缺失 → 仅 hide，不发输入', () => {
    const { ctrl, sent } = makeMocks(AGENTS);
    ctrl._onInput('@ar');
    ctrl._select({ dataset: {} });
    assert.deepEqual(sent, []);
  });
});

describe('_itemHtml — 转义与颜色归一', () => {
  test('HTML 特殊字符被转义', () => {
    const { ctrl } = makeMocks(AGENTS);
    const html = ctrl._itemHtml(
      { id: 'a<b>', name: 'x&y"z', description: "'" , color: '#000', icon: '' }, 0,
    );
    assert.ok(html.includes('&lt;'));   // < → &lt;
    assert.ok(html.includes('&amp;'));  // & → &amp;
    assert.ok(html.includes('&quot;')); // " → &quot;
    assert.ok(html.includes('&#39;'));  // ' → &#39;
    assert.ok(!html.includes('<b>'));   // 原始 < 已转义
  });
  test('color 非 # → 归一为 #A78BFA', () => {
    const { ctrl } = makeMocks(AGENTS);
    const html = ctrl._itemHtml({ id: 'a', name: 'A', description: '', color: 'red', icon: '' }, 0);
    assert.ok(html.includes('#A78BFA'));
  });
  test('有 icon → 用 icon；无 icon → 取首字母', () => {
    const { ctrl } = makeMocks(AGENTS);
    const withIcon = ctrl._itemHtml({ id: 'a', name: 'A', description: '', color: '#000', icon: '🚀' }, 0);
    assert.ok(withIcon.includes('🚀'));
    const letter = ctrl._itemHtml({ id: 'a', name: 'alpha', description: '', color: '#000', icon: '' }, 0);
    assert.ok(letter.includes('A')); // name 首字母大写
  });
});

describe('_onKeyDown — 捕获阶段导航', () => {
  function keyEvt(key) {
    const e = { key, preventDefault() {}, stopPropagation() {} };
    return e;
  }
  function navMocks(n) {
    const m = makeMocks(AGENTS);
    m.ctrl.active = true;
    m.classList.add('on'); // 弹窗可见
    const items = Array.from({ length: n }, (_, i) => ({
      dataset: { agent: `agent${i}` },
      classList: { toggle() {} },
    }));
    m.popEl._setItems(items);
    return m;
  }
  test('ArrowDown 递增并夹到末项', () => {
    const { ctrl } = navMocks(3);
    assert.equal(ctrl.idx, -1);
    ctrl._onKeyDown(keyEvt('ArrowDown'));
    assert.equal(ctrl.idx, 0);
    ctrl._onKeyDown(keyEvt('ArrowDown'));
    ctrl._onKeyDown(keyEvt('ArrowDown'));
    ctrl._onKeyDown(keyEvt('ArrowDown')); // 越界夹断
    assert.equal(ctrl.idx, 2);
  });
  test('ArrowUp 递减并夹到 0', () => {
    const { ctrl } = navMocks(3);
    ctrl.idx = 1;
    ctrl._onKeyDown(keyEvt('ArrowUp'));
    assert.equal(ctrl.idx, 0);
    ctrl._onKeyDown(keyEvt('ArrowUp')); // 不为负
    assert.equal(ctrl.idx, 0);
  });
  test('Enter 在 idx=-1 时选中首项', () => {
    const { ctrl, sent } = navMocks(3);
    ctrl.prefix = ''; // 直接置状态（不经过 _onInput，避免与 navMocks 预置 active 冲突）
    ctrl.idx = -1;
    ctrl._onKeyDown(keyEvt('Enter'));
    // 选中首项 agent0：1 backspace(@) + @agent0
    assert.deepEqual(sent, ['\x7f', '@agent0 ']);
  });
  test('Escape → hide', () => {
    const { ctrl } = navMocks(3);
    ctrl._onKeyDown(keyEvt('Escape'));
    assert.equal(ctrl.active, false);
  });
  test('弹窗不可见时导航键不拦截', () => {
    const { ctrl } = makeMocks(AGENTS);
    ctrl.active = true;
    // classList 无 'on'
    const before = ctrl.idx;
    ctrl._onKeyDown(keyEvt('ArrowDown'));
    assert.equal(ctrl.idx, before); // 未改动
  });
});
