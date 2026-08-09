// M1: 多供应商管理 + 手动切换 — ModelSwitchController 前端控制器单测
//
// 被测：src/provider-switch.js 的 ModelSwitchController（export { ModelSwitchController }）
// 硬约束：
//  - provider-switch.js 顶层 `import * as ipc from './ipc-bridge.js'`；
//    ipc-bridge.js 顶层解构 window.__TAURI__.core.{invoke, Channel} 与
//    window.__TAURI__.event.listen → 必须在 import 之前装好 globalThis.window.__TAURI__。
//  - 测试用动态 import，且 import 前完成 shim。
//  - 项目无 jsdom，手写最小 fake DOM（见 FakeElement / parseHTML / freshDocument）。
//  - 定时器 stub 为队列，只记录不真正调度 → 3200ms toast 定时器不会拖慢/挂起进程，
//    事件循环无悬挂 → node --test 进程自然退出。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';

// ════════════════════════════════════════════════════════════════════
// 全局 shim（必须在 import provider-switch.js 之前执行）
// ════════════════════════════════════════════════════════════════════

// 可变的跨测试状态：ipc-bridge 在模块顶层捕获 invoke/listen 引用，因此这两个
// 函数必须稳定（读取 state），测试只需在 makeEnv() 里重置 state 即得干净环境。
const state = {
  handlers: new Map(),       // cmd -> value | (args)=>value | { __reject: err }
  invokeCalls: [],           // [{ cmd, args }]
  eventListeners: new Map(), // channel -> Set<callback>
  confirmCalls: [],
  confirmResult: true,
  dispatched: [],
};

globalThis.window = globalThis.window || {};
globalThis.window.__TAURI__ = {
  core: {
    invoke: async (cmd, args) => {
      state.invokeCalls.push({ cmd, args: args ?? null });
      const h = state.handlers.get(cmd);
      if (h && h.__reject !== undefined) throw h.__reject;
      if (typeof h === 'function') {
        return h(args);
      }
      return h;
    },
    // Channel 只需 stub class：new 不抛错、有 onmessage 属性；本测试不真正走 PTY channel。
    Channel: class Channel {
      constructor() { this.onmessage = null; }
    },
  },
  event: {
    listen: (channel, cb) => {
      if (!state.eventListeners.has(channel)) state.eventListeners.set(channel, new Set());
      state.eventListeners.get(channel).add(cb);
      return () => { state.eventListeners.get(channel)?.delete(cb); };
    },
  },
};
globalThis.window.confirm = (msg) => { state.confirmCalls.push(msg); return state.confirmResult; };
globalThis.window.dispatchEvent = (ev) => { state.dispatched.push(ev); };
globalThis.CustomEvent = globalThis.CustomEvent || class CustomEvent {
  constructor(type, init = {}) { this.type = type; this.detail = init.detail; }
};

// 定时器 stub：入队但不真正调度 → 进程可自然退出；需要时可 flushTimers()。
const pendingTimers = [];
globalThis.setTimeout = (fn, ms, ...args) => { pendingTimers.push({ fn, ms, args }); return pendingTimers.length; };
globalThis.clearTimeout = () => {};
globalThis.requestAnimationFrame = (fn) => { fn(); return 0; };
globalThis.cancelAnimationFrame = () => {};
function flushTimers() {
  while (pendingTimers.length) {
    const t = pendingTimers.shift();
    t.fn(...t.args);
  }
}

// ════════════════════════════════════════════════════════════════════
// 最小 fake DOM
// ════════════════════════════════════════════════════════════════════

const VOID_TAGS = new Set(['br', 'hr', 'img', 'input', 'meta', 'link', 'area', 'base', 'col', 'embed', 'source', 'track', 'wbr']);

/** 把一段 HTML 字符串解析成一棵 FakeElement 树，返回根（#root，children 即顶层元素）。 */
function parseHTML(html) {
  const root = new FakeElement('#root');
  const stack = [root];
  // 依次匹配：开始/自闭合标签、闭合标签、注释、文本
  const re = /<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[a-zA-Z][a-zA-Z0-9-]*(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*(\/?)>|<\/([a-zA-Z][a-zA-Z0-9-]*)\s*>|<!--[\s\S]*?-->|([^<]+)/g;
  let m;
  while ((m = re.exec(html)) !== null) {
    if (m[0].startsWith('<!--')) continue;
    if (m[1]) {
      const tag = m[1];
      const attrStr = m[2] || '';
      const selfClosing = m[3] === '/';
      const el = new FakeElement(tag);
      const attrRe = /([a-zA-Z][a-zA-Z0-9-]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
      let am;
      while ((am = attrRe.exec(attrStr)) !== null) {
        const name = am[1];
        const val = am[2] ?? am[3] ?? am[4] ?? '';
        el._setAttr(name, val);
      }
      const top = stack[stack.length - 1];
      top.children.push(el);
      el.parent = top;
      if (!selfClosing && !VOID_TAGS.has(tag.toLowerCase())) stack.push(el);
    } else if (m[4]) {
      // 闭合标签：从栈顶弹出直到匹配的开启标签
      const closeTag = m[4].toLowerCase();
      while (stack.length > 1 && stack[stack.length - 1].tagName.toLowerCase() !== closeTag) stack.pop();
      if (stack.length > 1) stack.pop();
    } else if (m[5] != null) {
      const top = stack[stack.length - 1];
      top._text = (top._text === undefined ? '' : top._text) + m[5];
    }
  }
  return root;
}

/** 单段复合选择器匹配：支持 tag、.class(.class..)、#id、[attr]、[attr="val"]。 */
function matchCompound(el, comp) {
  let i = 0;
  if (comp[i] && comp[i] !== '.' && comp[i] !== '#' && comp[i] !== '[') {
    const mt = /^[a-zA-Z0-9-]+/.exec(comp);
    if (mt[0].toUpperCase() !== el.tagName) return false;
    i = mt[0].length;
  }
  while (i < comp.length) {
    const c = comp[i];
    if (c === '.') {
      const mc = /^[a-zA-Z0-9_-]+/.exec(comp.slice(i + 1));
      if (!mc || !el.classList.contains(mc[0])) return false;
      i += 1 + mc[0].length;
    } else if (c === '#') {
      const mi = /^[a-zA-Z0-9_-]+/.exec(comp.slice(i + 1));
      if (!mi || el.id !== mi[0]) return false;
      i += 1 + mi[0].length;
    } else if (c === '[') {
      const end = comp.indexOf(']', i);
      if (end < 0) return false;
      const inner = comp.slice(i + 1, end);
      const eq = inner.indexOf('=');
      if (eq === -1) {
        if (!el._hasAttr(inner)) return false;
      } else {
        const name = inner.slice(0, eq).trim();
        let val = inner.slice(eq + 1).trim();
        if ((val[0] === '"' && val[val.length - 1] === '"') || (val[0] === "'" && val[val.length - 1] === "'")) {
          val = val.slice(1, -1);
        }
        if (String(el._getAttr(name)) !== val) return false;
      }
      i = end + 1;
    } else {
      return false;
    }
  }
  return true;
}

/** 选择器匹配（支持空格后代组合）。最右一段必须匹配元素自身，左段沿祖先链匹配（允许跨代）。 */
function matchSelector(el, sel) {
  const parts = sel.trim().split(/\s+/);
  return matchChain(el, parts, parts.length - 1);
}
function matchChain(el, parts, i) {
  if (i < 0) return true;
  if (!el) return false;
  if (!matchCompound(el, parts[i])) return false;
  if (i === 0) return true;
  let anc = el.parent;
  while (anc) {
    if (matchChain(anc, parts, i - 1)) return true;
    anc = anc.parent;
  }
  return false;
}

class FakeElement {
  constructor(tagName) {
    this.tagName = String(tagName).toUpperCase();
    this.children = [];
    this.parent = null;
    this._listeners = new Map();
    this.attributes = new Map();
    this.dataset = {};
    this.style = {};
    this.value = '';
    this.placeholder = '';
    this.type = '';
    this.title = '';
    this.id = '';
    this.disabled = false;
    this._text = undefined;
    this._classes = new Set();
  }

  get className() { return [...this._classes].join(' '); }
  set className(v) { this._classes = new Set(String(v ?? '').split(/\s+/).filter(Boolean)); }

  get classList() {
    const s = this._classes;
    return {
      add: (...c) => { for (const x of c) if (x) s.add(x); },
      remove: (...c) => { for (const x of c) if (x) s.delete(x); },
      contains: (c) => s.has(c),
      has: (c) => s.has(c),
      toggle: (c, force) => {
        if (force === undefined) { if (s.has(c)) { s.delete(c); return false; } s.add(c); return true; }
        if (force) s.add(c); else s.delete(c);
        return s.has(c);
      },
    };
  }

  _setAttr(name, val) {
    const v = String(val);
    this.attributes.set(name, v);
    if (name === 'id') this.id = v;
    else if (name === 'class') this.className = v;
    else if (name === 'title') this.title = v;
    else if (name === 'value') this.value = v;
    else if (name === 'placeholder') this.placeholder = v;
    else if (name === 'type') this.type = v;
    else if (name === 'disabled') this.disabled = val !== null && val !== false && val !== 'false';
    else if (name.startsWith('data-')) this.dataset[name.slice(5)] = v;
  }
  _getAttr(name) { return this.attributes.get(name); }
  _hasAttr(name) { return this.attributes.has(name); }

  get textContent() {
    let out = '';
    if (this._text !== undefined) out += this._text;
    for (const c of this.children) out += c.textContent;
    return out;
  }
  set textContent(v) {
    this._text = String(v ?? '');
    this.children = [];
  }

  get innerHTML() {
    let out = '';
    if (this._text !== undefined) out += this._text;
    for (const c of this.children) out += c.outerHTML;
    return out;
  }
  set innerHTML(v) {
    this._text = undefined;
    this.children = [];
    const root = parseHTML(String(v));
    for (const c of root.children) { c.parent = this; this.children.push(c); }
  }

  get outerHTML() {
    const attrs = [];
    for (const [k, v] of this.attributes) attrs.push(`${k}="${v.replace(/"/g, '&quot;')}"`);
    const a = attrs.length ? ' ' + attrs.join(' ') : '';
    const tag = this.tagName.toLowerCase();
    const inner = this._text !== undefined
      ? this._text
      : this.children.map((c) => c.outerHTML).join('');
    return `<${tag}${a}>${inner}</${tag}>`;
  }

  addEventListener(type, fn) {
    if (!this._listeners.has(type)) this._listeners.set(type, new Set());
    this._listeners.get(type).add(fn);
  }
  removeEventListener(type, fn) {
    this._listeners.get(type)?.delete(fn);
  }
  /** 触发事件：先本元素监听器，再向上冒泡（受 event._stopped 控制）。 */
  _emit(type, event) {
    const listeners = this._listeners.get(type);
    if (listeners) {
      for (const fn of [...listeners]) {
        fn(event);
      }
    }
    if (this.parent && !event._stopped) this.parent._emit(type, event);
  }

  appendChild(child) {
    if (child.parent) child.remove();
    child.parent = this;
    this.children.push(child);
    return child;
  }
  remove() {
    if (this.parent) {
      const i = this.parent.children.indexOf(this);
      if (i >= 0) this.parent.children.splice(i, 1);
      this.parent = null;
    }
  }

  closest(sel) {
    let el = this;
    while (el) {
      if (matchSelector(el, sel)) return el;
      el = el.parent;
    }
    return null;
  }

  querySelector(sel) {
    return this.querySelectorAll(sel)[0] || null;
  }
  querySelectorAll(sel) {
    const out = [];
    const walk = (el) => {
      for (const c of el.children) {
        if (matchSelector(c, sel)) out.push(c);
        walk(c);
      }
    };
    walk(this);
    return out;
  }

  focus() {}
  select() {}
  scrollIntoView() {}
}

function freshDocument() {
  const body = new FakeElement('body');
  const byId = new Map();
  return {
    body,
    register(el) { if (el.id) byId.set(el.id, el); },
    getElementById(id) { return byId.get(id) || null; },
    createElement(tag) { return new FakeElement(tag); },
    querySelector(sel) { return body.querySelector(sel); },
    querySelectorAll(sel) { return body.querySelectorAll(sel); },
  };
}

/** 构造最小事件对象。 */
function makeEvent(target, props = {}) {
  return Object.assign({
    target,
    _stopped: false,
    defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this._stopped = true; },
  }, props);
}

// 测试数据：3 家供应商 A/B/C（name 与 id 相同，便于断言精确文案）
function makeCatalog(activeId) {
  return {
    providers: [
      { id: 'A', name: 'A', model: 'm-a', base_url: 'https://a' },
      { id: 'B', name: 'B', model: 'm-b', base_url: 'https://b' },
      { id: 'C', name: 'C', model: 'm-c', base_url: 'https://c' },
    ],
    active_provider_id: activeId,
    failover_queue: ['B', 'C'],
    auto_failover_enabled: true,
    failover_params: { consecutive_failures: 4, timeout_seconds: 60 },
  };
}

function makeTabs(entries, restartTab) {
  return {
    tabs: new Map(entries),
    activeId: null,
    restartTab: restartTab || (async () => {}),
  };
}

// ════════════════════════════════════════════════════════════════════
// 环境工厂（每测试干净状态 + 全新 fake document + 新 controller）
// ════════════════════════════════════════════════════════════════════

function makeEnv() {
  state.handlers = new Map();
  state.invokeCalls = [];
  state.eventListeners = new Map();
  state.confirmCalls = [];
  state.confirmResult = true;
  state.dispatched = [];

  const doc = freshDocument();
  globalThis.document = doc;

  const chip = new FakeElement('div');
  chip._setAttr('id', 'ribbonProviderChip');
  doc.register(chip);
  doc.body.appendChild(chip);

  const ctrl = new ModelSwitchController();

  return {
    doc,
    chip,
    ctrl,
    get calls() { return state.invokeCalls; },
    callsFor(cmd) { return state.invokeCalls.filter((c) => c.cmd === cmd).map((c) => c.args); },
    setHandler(cmd, v) { state.handlers.set(cmd, v); },
    setReject(cmd, err) { state.handlers.set(cmd, { __reject: err }); },
    emit(ch, payload) {
      for (const cb of [...(state.eventListeners.get(ch) || [])]) cb({ payload });
    },
    setConfirm(v) { state.confirmResult = v; },
    confirmCalls() { return state.confirmCalls; },
    toasts() {
      return doc.body.querySelectorAll('.ps-toast').map((el) => ({ text: el.textContent, cls: el.className }));
    },
  };
}

/** 排空微任务（本测试所有 Promise 都是已 resolve/reject 的确定性 Promise）。 */
async function flush(n = 30) {
  for (let i = 0; i < n; i++) await Promise.resolve();
}

// 先装好 shim 再导入被测模块（动态 import）。
const { ModelSwitchController } = await import('../src/provider-switch.js');

// ════════════════════════════════════════════════════════════════════
// 1. 多供应商列表
// ════════════════════════════════════════════════════════════════════

describe('PS 多供应商列表', () => {
  test('PS-01 正常：渲染 3 行，当前项 active + 「当前」，selIdx 默认落在当前项', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.tm = makeTabs([]);
    ctrl.catalog = makeCatalog('A');
    await ctrl.openSwitcher();
    const overlay = ctrl._overlay;
    assert.ok(overlay, 'overlay 打开');

    const items = overlay.querySelectorAll('.ps-item[data-id]');
    assert.equal(items.length, 3, '渲染 3 行');

    const rowA = overlay.querySelector('.ps-item[data-id="A"]');
    assert.ok(rowA, '存在 A 行');
    assert.equal(rowA.classList.contains('active'), true, 'A 行 active');
    assert.ok(rowA.textContent.includes('当前'), 'A 行带「当前」文本');
    assert.equal(rowA.classList.contains('sel'), true, 'selIdx 默认落在 A');
  });

  test('PS-02 边界：active_provider_id 为 null 或不存在 → 无 active 行且不报错', async () => {
    for (const activeId of [null, 'ZZZ']) {
      const env = makeEnv();
      const ctrl = env.ctrl;
      ctrl.tm = makeTabs([]);
      ctrl.catalog = makeCatalog(activeId);
      await ctrl.openSwitcher();
      const items = ctrl._overlay.querySelectorAll('.ps-item[data-id]');
      assert.equal(items.length, 3);
      const activeRows = items.filter((el) => el.classList.contains('active'));
      assert.equal(activeRows.length, 0, `active_provider_id=${activeId} 无 active 行`);
    }
  });

  test('PS-03 边界：providers 为空 → 空态文案「尚无供应商」+ 管理按钮仍在', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.tm = makeTabs([]);
    ctrl.catalog = { providers: [], active_provider_id: null };
    await ctrl.openSwitcher();
    const overlay = ctrl._overlay;
    assert.ok(overlay.textContent.includes('尚无供应商'), '空态文案');
    assert.ok(overlay.querySelector('.ps-manage'), '管理供应商按钮仍在');
  });

  test('PS-05 正常：点击当前项行 → 只关闭 overlay，不调 providers_switch', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.tm = makeTabs([]);
    ctrl.catalog = makeCatalog('A');
    await ctrl.openSwitcher();
    const rowA = ctrl._overlay.querySelector('.ps-item[data-id="A"]');
    rowA._emit('click', makeEvent(rowA));
    assert.equal(ctrl._overlay, null, 'overlay 已关闭');
    assert.equal(env.callsFor('providers_switch').length, 0, '未调 providers_switch');
  });
});

// ════════════════════════════════════════════════════════════════════
// 2. 切换事件流
// ════════════════════════════════════════════════════════════════════

describe('PS 切换事件流', () => {
  test('PS-06 正常：performSwitch + provider-switched → 芯片先「切换中」→ 重启 running → 复位 → 芯片=B → toast', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    let catalog = makeCatalog('A');
    env.setHandler('providers_list', () => catalog);
    env.setHandler('providers_switch', (args) => { catalog = { ...catalog, active_provider_id: args.providerId }; return { ok: true }; });
    env.setHandler('load_config', { model: '' });

    const restarted = [];
    ctrl.tm = makeTabs([['t1', { status: 'running', isError: false, term: { focus() {} } }]], async (id) => { restarted.push(id); });
    ctrl.tm.activeId = 't1';
    ctrl.init(ctrl.tm);
    await flush();

    assert.equal(env.chip.textContent, 'A ▾', '初始芯片为当前供应商');

    await ctrl.performSwitch('B');
    assert.equal(env.callsFor('providers_switch').length, 1);
    assert.deepEqual(env.callsFor('providers_switch')[0], { providerId: 'B' });
    assert.equal(ctrl._switching, true, '切换进行中');
    assert.equal(env.chip.textContent, '⟳ 切换中…', '芯片先显示切换中');

    env.emit('provider-switched', { from: 'A', to: 'B', reason: 'manual' });
    await flush();

    assert.deepEqual(restarted, ['t1'], '对 running tab 重启');
    assert.deepEqual(env.callsFor('pty_refresh_env'), [{ id: 't1' }], '先 pty_refresh_env');
    assert.equal(ctrl._switching, false, '_switching 复位');
    assert.equal(env.chip.textContent, 'B ▾', '芯片回到当前供应商');
    assert.ok(env.toasts().some((t) => t.text === '已切换到 B'), 'toast「已切换到 B」');
  });

  test('PS-09 正常：emit providers-changed → refresh 重拉 providers_list', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    let catalog = makeCatalog('A');
    env.setHandler('providers_list', () => catalog);
    ctrl.tm = makeTabs([]);
    ctrl.init(ctrl.tm);
    await flush();

    const before = env.callsFor('providers_list').length;
    catalog = { ...catalog, active_provider_id: 'B' };
    env.emit('providers-changed');
    await flush();

    assert.equal(env.callsFor('providers_list').length, before + 1, '重拉 providers_list');
    assert.equal(ctrl.catalog.active_provider_id, 'B', 'catalog 已更新');
  });

  test('PS-10 正常：事件回调顺序 refresh → restartSessions → _switching=false → renderChip', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    let catalog = makeCatalog('A');
    env.setHandler('providers_list', () => catalog);
    env.setHandler('providers_switch', (args) => { catalog = { ...catalog, active_provider_id: args.providerId }; return { ok: true }; });
    ctrl.tm = makeTabs([['t1', { status: 'running', isError: false, term: { focus() {} } }]], async () => {});
    ctrl.init(ctrl.tm);
    await flush();

    const order = [];
    const origRefresh = ctrl.refresh.bind(ctrl);
    ctrl.refresh = async () => { order.push('refresh'); return origRefresh(); };
    const origRestart = ctrl.restartSessions.bind(ctrl);
    ctrl.restartSessions = async () => { order.push('restartSessions'); return origRestart(); };
    const origRender = ctrl.renderChip.bind(ctrl);
    ctrl.renderChip = () => { order.push(`renderChip(${ctrl._switching})`); return origRender(); };

    ctrl._switching = true; // 模拟 performSwitch 已置位，事件到来时的真实状态
    env.emit('provider-switched', { from: 'A', to: 'B', reason: 'manual' });
    await flush();

    // refresh 内部会先以 _switching=true 渲染一次芯片，最后 finally 再以 false 渲染一次
    assert.deepEqual(order, ['refresh', 'renderChip(true)', 'restartSessions', 'renderChip(false)']);
    assert.equal(ctrl._switching, false, '结束后 _switching 复位');
  });
});

// ════════════════════════════════════════════════════════════════════
// 3. 活跃会话确认 G1
// ════════════════════════════════════════════════════════════════════

describe('PS 活跃会话确认 G1', () => {
  test('PS-11 正常：2 个 running → 弹确认，文案含「2 个活跃会话」，不调 providers_switch', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([
      ['t1', { status: 'running', isError: false }],
      ['t2', { status: 'running', isError: false }],
    ]);
    await ctrl.selectProvider('B');
    assert.ok(ctrl._overlay.querySelector('.ps-confirm'), '确认框出现');
    assert.ok(ctrl._overlay.textContent.includes('2 个活跃会话'), '文案含 2 个活跃会话');
    assert.equal(env.callsFor('providers_switch').length, 0, '未调 providers_switch');
  });

  test('PS-12 边界：1 restarting + 1 isError → 仍弹确认（isError 排除），文案「1 个活跃会话」', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([
      ['t1', { status: 'restarting', isError: false }],
      ['t2', { status: 'running', isError: true }],
    ]);
    await ctrl.selectProvider('B');
    assert.ok(ctrl._overlay.querySelector('.ps-confirm'), '确认框出现');
    assert.ok(ctrl._overlay.textContent.includes('1 个活跃会话'), '文案含 1 个活跃会话');
    assert.equal(env.callsFor('providers_switch').length, 0, '未调 providers_switch');
  });

  test('PS-14 正常：确认框点「取消」→ 关闭，不调 providers_switch，_switching 仍 false', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([['t1', { status: 'running', isError: false }]]);
    await ctrl.selectProvider('B');
    const cancelBtn = ctrl._overlay.querySelector('.ps-cancel');
    cancelBtn._emit('click', makeEvent(cancelBtn));
    assert.equal(ctrl._overlay, null, '确认框关闭');
    assert.equal(env.callsFor('providers_switch').length, 0, '未调 providers_switch');
    assert.equal(ctrl._switching, false, '_switching 仍 false');
  });

  test('PS-15 正常：确认框点「切换」→ 关闭，调 providers_switch(B) 一次', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([['t1', { status: 'running', isError: false }]]);
    env.setHandler('providers_switch', { ok: true });
    await ctrl.selectProvider('B');
    const goBtn = ctrl._overlay.querySelector('.ps-go');
    goBtn._emit('click', makeEvent(goBtn));
    await flush();
    assert.equal(ctrl._overlay, null, '确认框关闭');
    assert.equal(env.callsFor('providers_switch').length, 1);
    assert.deepEqual(env.callsFor('providers_switch')[0], { providerId: 'B' });
  });

  test('PS-16 边界：无活跃 tab → 不弹确认，直接调 providers_switch(B)', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([['t1', { status: 'stopped', isError: false }]]);
    env.setHandler('providers_switch', { ok: true });
    await ctrl.selectProvider('B');
    assert.equal(ctrl._overlay, null, '不弹确认框');
    assert.equal(env.callsFor('providers_switch').length, 1);
    assert.deepEqual(env.callsFor('providers_switch')[0], { providerId: 'B' });
  });
});

// ════════════════════════════════════════════════════════════════════
// 4. F2/F3 入口
// ════════════════════════════════════════════════════════════════════

describe('PS F2/F3 入口', () => {
  test('PS-17 正常：chip 点击 → openSwitcher 弹出「切换模型」dialog', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    env.setHandler('providers_list', makeCatalog('A'));
    ctrl.tm = makeTabs([]);
    ctrl.init(ctrl.tm);
    await flush();

    env.chip._emit('click', makeEvent(env.chip));
    await flush();
    assert.ok(ctrl._overlay, 'switcher 弹出');
    assert.ok(ctrl._overlay.textContent.includes('切换模型'), '标题为切换模型');
  });

  test('PS-20 正常：switcher 点「管理供应商…」→ 弹「模型供应商」+ 渲染行 + 状态点 + failover 区', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    await ctrl.openSwitcher();
    const manageBtn = ctrl._overlay.querySelector('.ps-manage');
    manageBtn._emit('click', makeEvent(manageBtn));
    await flush();

    const overlay = ctrl._overlay;
    assert.ok(overlay, '管理弹窗打开');
    assert.ok(overlay.textContent.includes('模型供应商'), '标题为模型供应商');
    assert.equal(overlay.querySelectorAll('.ps-provider-row').length, 3, '渲染 3 个供应商行');
    assert.ok(overlay.querySelector('.ps-status-dot.using'), '有使用中状态点');
    assert.ok(overlay.querySelector('.ps-status-dot.ok'), '有可用状态点');
    assert.ok(overlay.textContent.includes('自动切换'), 'failover 区含自动切换');
    assert.ok(overlay.textContent.includes('备用顺序'), 'failover 区含备用顺序');
  });
});

// ════════════════════════════════════════════════════════════════════
// 5. 写回 IPC 契约
// ════════════════════════════════════════════════════════════════════

describe('PS 写回 IPC 契约', () => {
  test('PS-22 正常：performSwitch(B) 精确调 invoke(providers_switch,{providerId:B})', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_switch', { ok: true });
    await ctrl.performSwitch('B');
    assert.equal(env.callsFor('providers_switch').length, 1);
    assert.deepEqual(env.callsFor('providers_switch')[0], { providerId: 'B' });
  });

  test('PS-23 正常：新增表单填四要素 → providers_add({provider:{name,base_url,api_key,model}})', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_presets', []);
    env.setHandler('providers_add', { ok: true });
    env.setHandler('providers_list', ctrl.catalog);

    await ctrl.openManagement();
    ctrl._overlay.querySelector('.ps-add')._emit('click', makeEvent(ctrl._overlay.querySelector('.ps-add')));
    await flush();

    const form = ctrl._overlay;
    form.querySelector('.ps-name').value = 'DeepSeek';
    form.querySelector('.ps-url').value = 'https://api.deepseek.com';
    form.querySelector('.ps-key').value = 'sk-abc';
    form.querySelector('.ps-model').value = 'deepseek-v4';
    form.querySelector('.ps-save')._emit('click', makeEvent(form.querySelector('.ps-save')));
    await flush();

    assert.equal(env.callsFor('providers_add').length, 1);
    assert.deepEqual(env.callsFor('providers_add')[0], {
      provider: { name: 'DeepSeek', base_url: 'https://api.deepseek.com', api_key: 'sk-abc', model: 'deepseek-v4' },
    });
  });

  test('PS-24 正常：编辑 X 改 name、key 留空 → providers_update({id:X,updates:{name:NEW}})，不含 api_key', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = {
      providers: [{ id: 'X', name: 'Old', base_url: 'https://x', api_key: 'secret', model: 'mx' }],
      active_provider_id: 'X',
    };
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_presets', []);
    env.setHandler('providers_update', { ok: true });
    env.setHandler('providers_list', ctrl.catalog);

    await ctrl._openEditForm('X');
    const form = ctrl._overlay;
    form.querySelector('.ps-name').value = 'NEW';
    form.querySelector('.ps-save')._emit('click', makeEvent(form.querySelector('.ps-save')));
    await flush();

    const args = env.callsFor('providers_update')[0];
    assert.deepEqual(args, { id: 'X', updates: { name: 'NEW' } });
    assert.equal('api_key' in args.updates, false, 'updates 不含 api_key');
  });

  test('PS-25 正常：删除 B（confirm=true）→ providers_delete({id:B})，成功后 refresh', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_delete', { ok: true });
    let listCalls = 0;
    env.setHandler('providers_list', () => { listCalls++; return ctrl.catalog; });

    await ctrl.openManagement();
    const overlay = ctrl._overlay;
    const rowB = overlay.querySelectorAll('.ps-provider-row').find((r) => r.dataset.id === 'B');
    const delBtn = rowB.querySelector('.ps-del');
    delBtn._emit('click', makeEvent(delBtn));
    await flush();

    assert.ok(env.confirmCalls()[0].includes('B'), 'confirm 文案含供应商名');
    assert.equal(env.callsFor('providers_delete').length, 1);
    assert.deepEqual(env.callsFor('providers_delete')[0], { id: 'B' });
    assert.ok(listCalls >= 1, '删除后 refresh 重拉 providers_list');
  });
});

// ════════════════════════════════════════════════════════════════════
// 6. 重启会话
// ════════════════════════════════════════════════════════════════════

describe('PS 重启会话', () => {
  test('PS-26 正常：tabs={running,restarting,isError,stopped} → 仅处理 t1/t2，先 pty_refresh_env 后 restartTab', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    const restarted = [];
    ctrl.tm = makeTabs([
      ['t1', { status: 'running', isError: false }],
      ['t2', { status: 'restarting', isError: false }],
      ['t3', { status: 'running', isError: true }],
      ['t4', { status: 'stopped', isError: false }],
    ], async (id) => { restarted.push(id); });
    env.setHandler('pty_refresh_env', undefined);

    await ctrl.restartSessions();

    assert.deepEqual(env.callsFor('pty_refresh_env'), [{ id: 't1' }, { id: 't2' }], '仅 t1/t2 刷新 env');
    assert.deepEqual(restarted, ['t1', 't2'], '仅 t1/t2 重启，t3/t4 跳过');
  });

  test('PS-27 异常：pty_refresh_env(t1) reject → 仍继续 t1.restartTab + t2 完整处理，无未捕获异常', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    const restarted = [];
    ctrl.tm = makeTabs([
      ['t1', { status: 'running', isError: false }],
      ['t2', { status: 'running', isError: false }],
    ], async (id) => { restarted.push(id); });
    env.setHandler('pty_refresh_env', (args) => {
      if (args.id === 't1') return Promise.reject(new Error('env fail'));
      return undefined;
    });

    const origWarn = console.warn;
    console.warn = () => {};
    try {
      await ctrl.restartSessions();
    } finally {
      console.warn = origWarn;
    }

    assert.deepEqual(restarted, ['t1', 't2'], 't1 失败后仍重启 t1 与 t2');
    assert.deepEqual(env.callsFor('pty_refresh_env').map((a) => a.id), ['t1', 't2']);
  });

  test('PS-28 异常：restartTab(t1) reject → 仍继续 t2，无未捕获异常', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    const restarted = [];
    ctrl.tm = makeTabs([
      ['t1', { status: 'running', isError: false }],
      ['t2', { status: 'running', isError: false }],
    ], async (id) => {
      restarted.push(id);
      if (id === 't1') throw new Error('restart fail');
    });
    env.setHandler('pty_refresh_env', undefined);

    const origWarn = console.warn;
    console.warn = () => {};
    try {
      await ctrl.restartSessions();
    } finally {
      console.warn = origWarn;
    }

    assert.deepEqual(restarted, ['t1', 't2'], 't1 重启失败仍继续 t2');
  });
});

// ════════════════════════════════════════════════════════════════════
// 7. 边界/异常
// ════════════════════════════════════════════════════════════════════

describe('PS 边界/异常', () => {
  test('PS-31 异常：providers_list reject → catalog=null；芯片回退 load_config.model', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.tm = makeTabs([]);
    env.setReject('providers_list', new Error('backend down'));
    env.setHandler('load_config', { model: 'gpt-4o' });

    const origWarn = console.warn;
    console.warn = () => {};
    try {
      await ctrl.refresh();
    } finally {
      console.warn = origWarn;
    }

    assert.equal(ctrl.catalog, null, 'catalog 置 null');
    await flush();
    assert.equal(env.chip.textContent, 'gpt-4o ▾', '芯片回退显示 model');
  });

  test('PS-33 边界：键盘 ↓↓→Enter 选第3项、↑→Enter 选第2项、Esc 关闭不切换', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_switch', { ok: true });

    // 子场景1：active A，↓↓→Enter → 选第3项 C
    ctrl.catalog = makeCatalog('A');
    await ctrl.openSwitcher();
    let overlay = ctrl._overlay;
    assert.equal(overlay.querySelector('.ps-item[data-id="A"]').classList.contains('sel'), true, '默认 sel 在 A');
    overlay._emit('keydown', makeEvent(overlay, { key: 'ArrowDown' }));
    assert.equal(overlay.querySelector('.ps-item[data-id="B"]').classList.contains('sel'), true, '↓ 后 sel 在 B');
    overlay._emit('keydown', makeEvent(overlay, { key: 'ArrowDown' }));
    assert.equal(overlay.querySelector('.ps-item[data-id="C"]').classList.contains('sel'), true, '↓↓ 后 sel 在 C(第3项)');
    overlay._emit('keydown', makeEvent(overlay, { key: 'Enter' }));
    await flush();
    assert.equal(ctrl._overlay, null, 'Enter 后关闭');
    assert.deepEqual(env.callsFor('providers_switch').at(-1), { providerId: 'C' }, '选中第3项 C');
    ctrl._switching = false;

    // 子场景2：active C，↑→Enter → 选第2项 B
    ctrl.catalog = makeCatalog('C');
    await ctrl.openSwitcher();
    overlay = ctrl._overlay;
    assert.equal(overlay.querySelector('.ps-item[data-id="C"]').classList.contains('sel'), true, '默认 sel 在 C');
    overlay._emit('keydown', makeEvent(overlay, { key: 'ArrowUp' }));
    assert.equal(overlay.querySelector('.ps-item[data-id="B"]').classList.contains('sel'), true, '↑ 后 sel 在 B(第2项)');
    overlay._emit('keydown', makeEvent(overlay, { key: 'Enter' }));
    await flush();
    assert.deepEqual(env.callsFor('providers_switch').at(-1), { providerId: 'B' }, '选中第2项 B');
    ctrl._switching = false;

    // 子场景3：Esc → 关闭不切换
    const callsBefore = env.callsFor('providers_switch').length;
    ctrl.catalog = makeCatalog('A');
    await ctrl.openSwitcher();
    overlay = ctrl._overlay;
    overlay._emit('keydown', makeEvent(overlay, { key: 'Escape' }));
    assert.equal(ctrl._overlay, null, 'Esc 关闭');
    assert.equal(env.callsFor('providers_switch').length, callsBefore, 'Esc 不切换');
  });

  test('PS-34 异常：providers_switch reject → _switching 复位 + toast err，不调 restartSessions', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([['t1', { status: 'running', isError: false }]]);
    let restartCalled = false;
    const origRestart = ctrl.restartSessions.bind(ctrl);
    ctrl.restartSessions = async () => { restartCalled = true; return origRestart(); };
    env.setReject('providers_switch', new Error('switch failed'));

    const origWarn = console.warn;
    console.warn = () => {};
    try {
      await ctrl.performSwitch('B');
    } finally {
      console.warn = origWarn;
    }

    assert.equal(ctrl._switching, false, '_switching 复位');
    assert.equal(restartCalled, false, 'restartSessions 未被调');
    assert.ok(env.toasts().some((t) => t.cls.includes('ps-toast-err') && t.text.includes('switch failed')), 'toast err');
  });

  test('PS-35 异常：_switching=true 时再 performSwitch(C) → 直接 return，providers_switch 仅第一次被调', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    let resolveSwitch;
    env.setHandler('providers_switch', () => new Promise((res) => { resolveSwitch = res; }));

    const p1 = ctrl.performSwitch('B');
    assert.equal(ctrl._switching, true, '第一次切换进行中');
    await ctrl.performSwitch('C');
    assert.equal(env.callsFor('providers_switch').length, 1, '仅第一次被调');
    resolveSwitch({ ok: true });
    await p1;
    // 事件（provider-switched）未触发时 _switching 保持 true；仅事件回调会复位
    assert.equal(ctrl._switching, true, '无事件时 _switching 保持 true');
  });

  test('PS-38 边界：删除当前/唯一供应商（confirm=true）→ 仍发 providers_delete，后端 reject → toast err', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = { providers: [{ id: 'A', name: 'A', model: 'm', base_url: 'u' }], active_provider_id: 'A' };
    ctrl.tm = makeTabs([]);
    env.setReject('providers_delete', new Error('不可删除'));
    env.setHandler('providers_list', ctrl.catalog);

    await ctrl.openManagement();
    const overlay = ctrl._overlay;
    const rowA = overlay.querySelector('.ps-provider-row');
    rowA.querySelector('.ps-del')._emit('click', makeEvent(rowA.querySelector('.ps-del')));
    await flush();

    assert.equal(env.callsFor('providers_delete').length, 1, '前端仍发 providers_delete');
    assert.deepEqual(env.callsFor('providers_delete')[0], { id: 'A' });
    assert.ok(env.toasts().some((t) => t.cls.includes('ps-toast-err') && t.text.includes('不可删除')), 'toast err');
  });

  test('PS-40 边界：confirm 先 false 后 true → 第一次不发 delete，第二次发 delete', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_delete', { ok: true });
    env.setHandler('providers_list', ctrl.catalog);

    env.setConfirm(false);
    await ctrl.openManagement();
    let overlay = ctrl._overlay;
    let rowB = overlay.querySelectorAll('.ps-provider-row').find((r) => r.dataset.id === 'B');
    rowB.querySelector('.ps-del')._emit('click', makeEvent(rowB.querySelector('.ps-del')));
    await flush();
    assert.equal(env.callsFor('providers_delete').length, 0, 'confirm=false 不发 delete');
    assert.ok(ctrl._overlay, 'confirm=false 弹窗未关闭');

    env.setConfirm(true);
    overlay = ctrl._overlay;
    rowB = overlay.querySelectorAll('.ps-provider-row').find((r) => r.dataset.id === 'B');
    rowB.querySelector('.ps-del')._emit('click', makeEvent(rowB.querySelector('.ps-del')));
    await flush();
    assert.equal(env.callsFor('providers_delete').length, 1, 'confirm=true 发 delete');
    assert.deepEqual(env.callsFor('providers_delete')[0], { id: 'B' });
  });

  test('PS-42 边界：新增表单缺 name/base_url/model → toast「名称 / Base URL / 型号不能为空」且不发 providers_add', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = makeCatalog('A');
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_presets', []);

    await ctrl._openEditForm(null);
    const form = ctrl._overlay;
    form.querySelector('.ps-key').value = 'sk-only';
    form.querySelector('.ps-save')._emit('click', makeEvent(form.querySelector('.ps-save')));
    await flush();

    assert.equal(env.callsFor('providers_add').length, 0, '不发 providers_add');
    assert.ok(
      env.toasts().some((t) => t.cls.includes('ps-toast-err') && t.text.includes('名称 / Base URL / 型号不能为空')),
      'toast 名称 / Base URL / 型号不能为空',
    );
  });

  test('PS-45 边界：编辑 key 输入框 type=password、value=空、placeholder「不修改请留空」；改 name 保存 updates 不含 api_key', async () => {
    const env = makeEnv();
    const ctrl = env.ctrl;
    ctrl.catalog = {
      providers: [{ id: 'X', name: 'Old', base_url: 'https://x', api_key: 'secret', model: 'mx' }],
      active_provider_id: 'X',
    };
    ctrl.tm = makeTabs([]);
    env.setHandler('providers_presets', []);
    env.setHandler('providers_update', { ok: true });
    env.setHandler('providers_list', ctrl.catalog);

    await ctrl._openEditForm('X');
    const form = ctrl._overlay;
    const keyEl = form.querySelector('.ps-key');
    assert.equal(keyEl.type, 'password', 'key 输入框 type=password');
    assert.equal(keyEl.value, '', 'key 输入框 value 为空');
    assert.equal(keyEl.placeholder, '不修改请留空', 'key 输入框 placeholder');

    form.querySelector('.ps-name').value = 'NEW';
    form.querySelector('.ps-save')._emit('click', makeEvent(form.querySelector('.ps-save')));
    await flush();

    const args = env.callsFor('providers_update')[0];
    assert.deepEqual(args, { id: 'X', updates: { name: 'NEW' } });
    assert.equal('api_key' in args.updates, false, 'updates 不含 api_key');
  });
});
