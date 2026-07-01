// IME 修复自动化验证 — 基于真实事件派发 + initImeFix 注册的监听器。
//
// 修复 5 v5: 不清空 textarea，不在 compositionend 中发送。
// 只记录 _lastCompositionText，Fix 3 在 input 事件中剥离前缀。

import { test, describe, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { initImeFix } from '../src/terminal/ime-fix.js';

function createImeEnv() {
  const originalRaf = globalThis.requestAnimationFrame;
  const rafQueue = [];
  globalThis.requestAnimationFrame = (cb) => { rafQueue.push(cb); };

  const taListeners = {};
  const ta = {
    value: '',
    addEventListener(type, fn, capture) {
      const key = capture ? `${type}:capture` : `${type}:bubble`;
      taListeners[key] = taListeners[key] || [];
      taListeners[key].push(fn);
    },
    removeEventListener() {},
  };

  const elListeners = {};
  const termEl = {
    addEventListener(type, fn, capture) {
      const key = capture ? `${type}:capture` : `${type}:bubble`;
      elListeners[key] = elListeners[key] || [];
      elListeners[key].push(fn);
    },
    removeEventListener() {},
  };

  const sentViaImeSendFn = [];
  const sentViaOnData = [];
  const _sendLog = [];

  // ★ 统一 PTY 写入去重（与 tab-manager.js ptyWriteDedup 一致，区分来源）
  let _lastPtyData = '';
  let _lastPtyTime = 0;
  let _lastPtySource = '';
  const ptyWriteDedup = (data, source) => {
    const now = Date.now();
    if (data === _lastPtyData && (now - _lastPtyTime) < 30 && source !== _lastPtySource) {
      return; // 不同来源 30ms 内重复 → 丢弃
    }
    _lastPtyData = data;
    _lastPtyTime = now;
    _lastPtySource = source;
    _sendLog.push({ source: 'pty', data });
  };

  const term = {
    textarea: ta,
    element: { querySelector: () => ta },
    _imeSendFn: (data) => {
      sentViaImeSendFn.push(data);
      ptyWriteDedup(data, 'ime');
    },
    _xtermSentData: '',
    _xtermSentTime: 0,
    _compositionEndedAt: 0,
    _lastOnDataData: '',
    _lastOnDataTime: 0,
    _mentionController: null,
    onData: (cb) => {
      term._onDataCallback = cb;
      return { dispose: () => {} };
    },
    _onDataCallback: null,
  };

  initImeFix(term, termEl);

  function trackCalls() {
    let called = false;
    const fn = () => { called = true; };
    fn.wasCalled = () => called;
    return fn;
  }

  function dispatchTaEvent(type, eventObj) {
    const captureKey = `${type}:capture`;
    const bubbleKey = `${type}:bubble`;
    const captureListeners = taListeners[captureKey] || [];
    const bubbleListeners = taListeners[bubbleKey] || [];
    const e = {
      type, keyCode: 0, isComposing: false, ctrlKey: false,
      stopImmediatePropagation: () => {},
      preventDefault: () => {},
      ...eventObj,
    };
    for (const fn of captureListeners) fn(e);
    for (const fn of bubbleListeners) fn(e);
  }

  function dispatchElEvent(type, eventObj) {
    const captureKey = `${type}:capture`;
    const bubbleKey = `${type}:bubble`;
    const captureListeners = elListeners[captureKey] || [];
    const bubbleListeners = elListeners[bubbleKey] || [];
    const e = { type, isComposing: false, inputType: '', stopImmediatePropagation: () => {}, ...eventObj };
    for (const fn of captureListeners) fn(e);
    for (const fn of bubbleListeners) fn(e);
  }

  function simulateXtermOnData(data) {
    term._xtermSentData = data;
    term._xtermSentTime = Date.now();
    if (/^[a-zA-Z0-9]+( +[a-zA-Z0-9]+)+$/.test(data)) {
      data = data.replace(/ +/g, '');
    }
    sentViaOnData.push(data);
    ptyWriteDedup(data, 'onData');
  }

  function flushRaf() {
    while (rafQueue.length > 0) rafQueue.shift()();
  }

  const sentToPty = () => _sendLog.map(e => e.data).filter(x => x !== '');

  return {
    term, ta, termEl, dispatchTaEvent, dispatchElEvent,
    simulateXtermOnData, flushRaf, trackCalls,
    sentViaOnData, sentViaImeSendFn, sentToPty,
    restoreRaf: () => { globalThis.requestAnimationFrame = originalRaf; },
  };
}

// ── Scenario A: Chinese input — xterm sends, no duplicate ──

describe('Scenario A: Chinese input', () => {
  test('IME-A1: Composition — xterm sends via onData, textarea preserved', () => {
    const env = createImeEnv();
    try {
      const stopProp = env.trackCalls();
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('keydown', { keyCode: 229, isComposing: true, stopImmediatePropagation: () => {} });
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: stopProp });
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      // Simulate xterm sending the composition result via onData
      env.simulateXtermOnData('我们');
      env.flushRaf();

      assert.ok(stopProp.wasCalled(), 'space keydown blocked during composition');
      // v5: compositionend does NOT send, does NOT clear textarea
      assert.deepEqual(env.sentViaImeSendFn, [], 'nothing sent via _imeSendFn');
      assert.deepEqual(env.sentViaOnData, ['我们'], 'xterm sends via onData');
      assert.deepEqual(env.sentToPty(), ['我们'], 'exactly once');
    } finally {
      env.restoreRaf();
    }
  });

  test('IME-A2: IME re-inserts same text — dedup', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');
      env.flushRaf();

      // IME re-inserts same text into textarea
      env.ta.value = '我们';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });

      // _lastCompositionText dedup catches it
      assert.deepEqual(env.sentViaImeSendFn, [], 'IME re-insertion deduped');
      assert.deepEqual(env.sentViaOnData, ['我们'], 'only original');
    } finally {
      env.restoreRaf();
    }
  });

  test('IME-A3: Space after composition — only space sent (prefix stripped)', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      // User presses space — xterm sends via onData
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: false, stopImmediatePropagation: () => {} });
      env.simulateXtermOnData(' ');

      // Input event: textarea has "我们 " (stale composition + space)
      env.ta.value = '我们 ';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });

      // Fix 5 v5: strips "我们" prefix, only sends " " → but Fix 4 catches it (xterm already sent)
      assert.deepEqual(env.sentToPty(), ['我们', ' '], 'composition + space, no duplicate');
    } finally {
      env.restoreRaf();
    }
  });
});

// ── Scenario B: Direct space ──

describe('Scenario B: Direct space', () => {
  test('IME-B1: Normal space — Fix 4 dedup', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: false, stopImmediatePropagation: () => {} });
      env.simulateXtermOnData(' ');
      env.ta.value = ' ';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.deepEqual(env.sentViaOnData, [' '], 'space via onData');
      assert.deepEqual(env.sentViaImeSendFn, [], 'Fix 4 dedup');
    } finally {
      env.restoreRaf();
    }
  });
});

// ── Scenario C: Enter during composition ──

describe('Scenario C: Non-IME key during composition', () => {
  test('IME-C1: Enter commits text', () => {
    const env = createImeEnv();
    try {
      const sp = env.trackCalls();
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('keydown', { keyCode: 229, isComposing: true, stopImmediatePropagation: () => {} });
      env.dispatchTaEvent('keydown', { keyCode: 13, isComposing: true, stopImmediatePropagation: sp });
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');
      assert.ok(sp.wasCalled(), 'Enter blocked');
      assert.deepEqual(env.sentViaOnData, ['我们'], 'text sent via onData');
    } finally {
      env.restoreRaf();
    }
  });
});

// ── Scenario D: imeFilter ──

describe('Scenario D: imeFilter dedup', () => {
  test('IME-D1: dedup', async () => {
    const env = createImeEnv();
    try {
      assert.equal(env.term.imeFilter('a'), 'a');
      assert.equal(env.term.imeFilter('a'), '');
      await new Promise(r => setTimeout(r, 110));
      assert.equal(env.term.imeFilter('a'), 'a');
    } finally { env.restoreRaf(); }
  });
  test('IME-D2: different data', () => {
    const env = createImeEnv();
    try {
      assert.equal(env.term.imeFilter('a'), 'a');
      assert.equal(env.term.imeFilter('b'), 'b');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario E: Ctrl+C ──

describe('Scenario E: Ctrl+C', () => {
  test('IME-E1: resets isComposing', () => {
    const env = createImeEnv();
    try {
      const sp = env.trackCalls();
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('keydown', { keyCode: 67, ctrlKey: true, isComposing: true, stopImmediatePropagation: () => {} });
      env.dispatchTaEvent('keydown', { keyCode: 65, isComposing: false, stopImmediatePropagation: sp });
      assert.ok(!sp.wasCalled(), 'Ctrl+C reset isComposing');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario F: Regression ──

describe('Scenario F: Regression', () => {
  test('IME-F1: Paste — rAF clears textarea', () => {
    const env = createImeEnv();
    try {
      env.ta.value = 'text';
      env.dispatchTaEvent('paste', {});
      assert.equal(env.ta.value, 'text', 'before rAF');
      env.flushRaf();
      assert.equal(env.ta.value, '', 'after rAF');
    } finally { env.restoreRaf(); }
  });

  test('IME-F2: Shift+symbol after composition', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '你';
      env.dispatchTaEvent('compositionend', {});
      // _lastCompositionText = '你'
      // value = '！' → doesn't start with '你' → Fix 3 sends
      env.ta.value = '！';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.ok(env.sentViaImeSendFn.includes('！'), '！ sent');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario G: Chinese normal flow ──

describe('Scenario G: Chinese input normal flow', () => {
  test('IME-G1: "我们"', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      for (const kc of [87, 79, 77, 69, 78]) {
        env.dispatchTaEvent('keydown', { keyCode: kc, isComposing: true, stopImmediatePropagation: () => {} });
      }
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      assert.deepEqual(env.sentViaOnData, ['我们'], 'via onData');
      assert.deepEqual(env.sentViaImeSendFn, [], 'no _imeSendFn');
      assert.deepEqual(env.sentToPty(), ['我们'], 'once');
    } finally { env.restoreRaf(); }
  });

  test('IME-G2: "你好世界"', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      for (const kc of [78, 73, 72, 65, 79]) {
        env.dispatchTaEvent('keydown', { keyCode: kc, isComposing: true, stopImmediatePropagation: () => {} });
      }
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '你好';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('你好');

      env.dispatchTaEvent('compositionstart', {});
      for (const kc of [83, 72, 73, 74, 73, 69]) {
        env.dispatchTaEvent('keydown', { keyCode: kc, isComposing: true, stopImmediatePropagation: () => {} });
      }
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '世界';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('世界');

      assert.deepEqual(env.sentToPty(), ['你好', '世界'], 'both words');
    } finally { env.restoreRaf(); }
  });

  test('IME-G3: "hello你好"', () => {
    const env = createImeEnv();
    try {
      for (const ch of 'hello') {
        env.dispatchTaEvent('keydown', { keyCode: ch.charCodeAt(0), isComposing: false, stopImmediatePropagation: () => {} });
        env.simulateXtermOnData(ch);
      }
      env.dispatchTaEvent('compositionstart', {});
      for (const kc of [78, 73, 72, 65, 79]) {
        env.dispatchTaEvent('keydown', { keyCode: kc, isComposing: true, stopImmediatePropagation: () => {} });
      }
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '你好';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('你好');

      assert.deepEqual(env.sentToPty(), ['h', 'e', 'l', 'l', 'o', '你好'], 'mixed');
    } finally { env.restoreRaf(); }
  });

  test('IME-G4: Chinese + Enter', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      for (const kc of [87, 79, 77, 69, 78]) {
        env.dispatchTaEvent('keydown', { keyCode: kc, isComposing: true, stopImmediatePropagation: () => {} });
      }
      env.dispatchTaEvent('keydown', { keyCode: 32, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      env.dispatchTaEvent('keydown', { keyCode: 13, isComposing: false, stopImmediatePropagation: () => {} });
      env.simulateXtermOnData('\r');

      assert.deepEqual(env.sentToPty(), ['我们', '\r'], 'Chinese + Enter');
    } finally { env.restoreRaf(); }
  });

  test('IME-G5: Cancelled', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('keydown', { keyCode: 27, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = '';
      env.dispatchTaEvent('compositionend', {});

      assert.deepEqual(env.sentToPty(), [], 'nothing');
    } finally { env.restoreRaf(); }
  });

  test('IME-G6: Shift+symbol after composition', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '你';
      env.dispatchTaEvent('compositionend', {});
      // _lastCompositionText='你', value='！' → not startswith → Fix 3 sends
      env.ta.value = '！';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.ok(env.sentViaImeSendFn.includes('！'), '！ sent');
    } finally { env.restoreRaf(); }
  });
});

// ── Edge cases ──

describe('Edge cases', () => {
  test('Blur resets isComposing', () => {
    const env = createImeEnv();
    try {
      const sp = env.trackCalls();
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('blur', {});
      env.dispatchTaEvent('keydown', { keyCode: 65, isComposing: false, stopImmediatePropagation: sp });
      assert.ok(!sp.wasCalled());
    } finally { env.restoreRaf(); }
  });

  test('Paste with mentionController', () => {
    const env = createImeEnv();
    try {
      let st = [];
      env.term._mentionController = { setPasting: (v) => { st.push(v); } };
      env.dispatchTaEvent('paste', {});
      assert.deepEqual(st, [true]);
      env.flushRaf();
      assert.deepEqual(st, [true, false]);
    } finally { env.restoreRaf(); }
  });

  test('compositionend — textarea NOT cleared (v5)', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      // v5: textarea NOT cleared — xterm manages it
      assert.equal(env.ta.value, '我们', 'textarea preserved');
      assert.deepEqual(env.sentViaImeSendFn, [], 'nothing sent');
    } finally { env.restoreRaf(); }
  });

  test('Modifier keys not blocked', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      const s1 = env.trackCalls();
      env.dispatchTaEvent('keydown', { keyCode: 16, isComposing: true, stopImmediatePropagation: s1 });
      assert.ok(!s1.wasCalled(), 'Shift');
      const s2 = env.trackCalls();
      env.dispatchTaEvent('keydown', { keyCode: 17, isComposing: true, stopImmediatePropagation: s2 });
      assert.ok(!s2.wasCalled(), 'Ctrl');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario H: Caps Lock ──

describe('Scenario H: Caps Lock', () => {
  test('IME-H1: _compositionJustEnded blocks next keydown', () => {
    const env = createImeEnv();
    try {
      const sp = env.trackCalls();
      env.dispatchTaEvent('compositionstart', {});
      env.dispatchTaEvent('keydown', { keyCode: 229, isComposing: true, stopImmediatePropagation: () => {} });
      env.ta.value = 'wo men';
      env.dispatchTaEvent('compositionend', {});
      env.dispatchTaEvent('keydown', { keyCode: 20, isComposing: false, stopImmediatePropagation: sp });
      assert.ok(sp.wasCalled(), 'Caps Lock blocked');
    } finally { env.restoreRaf(); }
  });

  test('IME-H2: Pinyin spaces filtered by onData', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = 'wo men';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('wo men'); // onData filter → "women"
      assert.deepEqual(env.sentViaOnData, ['women'], 'pinyin spaces filtered');
    } finally { env.restoreRaf(); }
  });

  test('IME-H3: Pinyin re-insertion dedup', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = 'wo men';
      env.dispatchTaEvent('compositionend', {});
      // IME re-inserts pinyin → _lastCompositionText='wo men' → dedup
      env.ta.value = 'wo men';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.deepEqual(env.sentViaImeSendFn, [], 're-insertion deduped');
    } finally { env.restoreRaf(); }
  });

  test('IME-H4: Pinyin + new char — prefix stripped', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = 'wo men';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('women');

      // User types after Caps Lock switch: textarea has "wo men "
      env.ta.value = 'wo men ';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      // Prefix "wo men" stripped → only " " sent via _imeSendFn
      // (Space keydown was blocked by Fix 2 _compositionJustEnded → xterm didn't send it)
      assert.deepEqual(env.sentToPty(), ['women', ' '], 'composition + space, no duplicate');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario I: Fix 5 v5 ──

describe('Scenario I: Fix 5 v5 — record, prefix strip', () => {
  test('IME-I1: Textarea preserved after compositionend', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      assert.equal(env.ta.value, '我们', 'textarea preserved');
    } finally { env.restoreRaf(); }
  });

  test('IME-I2: Full match dedup', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.ta.value = '我们';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.deepEqual(env.sentViaImeSendFn, [], 'full match deduped');
    } finally { env.restoreRaf(); }
  });

  test('IME-I3: Prefix strip — only new part sent', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      // User types "a" → textarea has "我们a"
      env.ta.value = '我们a';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      // "我们" prefix stripped → "a" sent via _imeSendFn
      // "a" is new input after composition — should be sent
      assert.deepEqual(env.sentToPty(), ['我们', 'a'], 'composition + new char, no duplicate');
    } finally { env.restoreRaf(); }
  });

  test('IME-I4: _lastCompositionText persists until consumed (no time expiry)', async () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');
      await new Promise(r => setTimeout(r, 210));
      // Even after 210ms, prefix stripping still works — no time-based expiry
      env.ta.value = '我们 ';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      // "我们" prefix stripped → only " " sent via _imeSendFn
      // xterm also sent " " → Fix 4 dedup may catch it
      // Key assertion: NO duplicate of "我们"
      const ptySent = env.sentToPty();
      const hasDuplicate = ptySent.filter(d => d.includes('我们')).length > 1;
      assert.ok(!hasDuplicate, 'no duplicate composition text even after 210ms delay');
    } finally { env.restoreRaf(); }
  });

  test('IME-I5: _lastCompositionText cleared after consumed', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      // First input: prefix stripped, _lastCompositionText consumed
      env.ta.value = '我们 ';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });

      // Second input: no prefix stripping (_lastCompositionText already consumed)
      env.ta.value = 'a';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      // "a" sent normally, no stale prefix
      assert.ok(env.sentViaImeSendFn.includes('a'), 'second char sent normally after prefix consumed');
      // No duplicate of "我们" in second send
      assert.ok(!env.sentViaImeSendFn.some(d => d.includes('我们') && d !== '我们'),
        'no stale composition text in second input');
    } finally { env.restoreRaf(); }
  });

  test('IME-I6: _lastCompositionText cleared by compositionstart', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '我们';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('我们');

      // New composition starts → _lastCompositionText cleared
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = '好';
      env.dispatchTaEvent('compositionend', {});
      env.simulateXtermOnData('好');
      assert.deepEqual(env.sentToPty(), ['我们', '好'], 'new composition independent of old');
    } finally { env.restoreRaf(); }
  });
});

// ── Scenario J: Fix 3 + Fix 4 ──

describe('Scenario J: Fix 3 + Fix 4', () => {
  test('IME-J1: English Fix 4 dedup', () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('keydown', { keyCode: 65, isComposing: false, stopImmediatePropagation: () => {} });
      env.simulateXtermOnData('a');
      env.ta.value = 'a';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.deepEqual(env.sentViaOnData, ['a']);
      assert.deepEqual(env.sentViaImeSendFn, []);
    } finally { env.restoreRaf(); }
  });

  test('IME-J2: IME Shift+symbol', () => {
    const env = createImeEnv();
    try {
      env.ta.value = '！';
      env.dispatchElEvent('input', { inputType: 'insertText', isComposing: false });
      assert.ok(env.sentViaImeSendFn.includes('！'));
    } finally { env.restoreRaf(); }
  });

  test('IME-J3: _compositionJustEnded 200ms auto-reset', async () => {
    const env = createImeEnv();
    try {
      env.dispatchTaEvent('compositionstart', {});
      env.ta.value = 'x';
      env.dispatchTaEvent('compositionend', {});
      await new Promise(r => setTimeout(r, 210));
      const sp = env.trackCalls();
      env.dispatchTaEvent('keydown', { keyCode: 65, isComposing: false, stopImmediatePropagation: sp });
      assert.ok(!sp.wasCalled(), 'auto-reset');
    } finally { env.restoreRaf(); }
  });
});
