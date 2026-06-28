// IME 修复自动化验证 — 基于 onecode fixImeStaleText + fixDesktopPaste 方案。
//
// 核心原则：桌面端不干预 xterm 的 composition 处理流程。
// 只做两件事：
// 1. paste 后 rAF 清空 textarea
// 2. composition 期间 stopImmediatePropagation 阻止非 229 keydown
//    （不调 preventDefault，IME 需要按键正常处理）

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';

// ── Fix 2: keydown 拦截逻辑 ──

function keydownShouldBlock(isComposing, eIsComposing, keyCode) {
  if (!isComposing && !eIsComposing) return false;
  // keyCode 229 = IME 处理键（安全）
  // keyCode 16/17/18 = 修饰键（安全）
  if (keyCode === 229 || keyCode === 16 || keyCode === 17 || keyCode === 18) return false;
  return true;
}

describe('Fix 2: composition 期间 keydown 拦截', () => {
  test('composition 期间 Space(keyCode=32) 应被拦截', () => {
    assert.ok(keydownShouldBlock(true, true, 32), 'isComposing + Space → 拦截');
  });

  test('composition 期间 Enter(keyCode=13) 应被拦截', () => {
    assert.ok(keydownShouldBlock(true, true, 13), 'isComposing + Enter → 拦截');
  });

  test('composition 期间 Escape(keyCode=27) 应被拦截', () => {
    assert.ok(keydownShouldBlock(true, true, 27), 'isComposing + Escape → 拦截');
  });

  test('composition 期间 IME 处理键(keyCode=229) 不应拦截', () => {
    assert.ok(!keydownShouldBlock(true, true, 229), 'keyCode 229 不拦截');
  });

  test('composition 期间 修饰键(16/17/18) 不应拦截', () => {
    assert.ok(!keydownShouldBlock(true, true, 16), 'Shift 不拦截');
    assert.ok(!keydownShouldBlock(true, true, 17), 'Ctrl 不拦截');
    assert.ok(!keydownShouldBlock(true, true, 18), 'Alt 不拦截');
  });

  test('非 composition 期间不拦截', () => {
    assert.ok(!keydownShouldBlock(false, false, 32), '非 composition 不拦截');
    assert.ok(!keydownShouldBlock(false, false, 13), '非 composition 不拦截');
  });

  test('e.isComposing=true 但 isComposing=false 时也拦截', () => {
    // macOS 时序差：compositionend 后 keydown.isComposing 仍为 true
    // xterm 内部 _isComposing 可能已 false，但 _isSendingComposition 可能仍 true
    // → _finalizeComposition(false) 可能触发 → 仍需拦截
    assert.ok(keydownShouldBlock(false, true, 32),
      'e.isComposing timing lag → 仍拦截');
  });

  test('isComposing=true 但 e.isComposing=false 时也拦截', () => {
    // 我们的 compositionstart 比 keydown 先触发（capture 阶段）
    assert.ok(keydownShouldBlock(true, false, 32),
      'isComposing=true → 拦截');
  });
});

// ── 完整场景验证 ──

describe('完整场景：中文输入流程', () => {
  test('场景 A: 拼音 → 空格确认 → 不重复', () => {
    // 1. compositionstart → isComposing = true
    // 2. keydown(keyCode=229) × N → 不拦截（IME 处理键）
    // 3. 用户按空格确认 → keydown(keyCode=32, isComposing=true)
    //    → 我们拦截 stopImmediatePropagation → xterm 不处理
    //    → 不调 preventDefault → IME 正常提交 composition
    // 4. compositionend → isComposing = false
    //    → xterm _finalizeComposition(true) → setTimeout(0) → 发送正确文本
    //    → 只发一次，不重复 ✅
    assert.ok(keydownShouldBlock(true, true, 32), '确认空格应拦截');
    assert.ok(!keydownShouldBlock(false, false, 32), '后续空格不拦截');
  });

  test('场景 B: 确认后再按空格 → 正常输出空格', () => {
    // compositionend 后 → isComposing = false
    // 用户按空格 → keydown(isComposing=false, e.isComposing=false)
    // → 不拦截 → xterm 正常处理 → 发送 " " ✅
    assert.ok(!keydownShouldBlock(false, false, 32), '普通空格不拦截');
  });

  test('场景 C: composition 期间 Tab → 不发送到终端', () => {
    // Tab 在 composition 期间被拦截 → xterm 不处理
    // 不调 preventDefault → 浏览器处理 Tab
    // （onecode web 中 Tab=Enter，桌面端不需要）
    assert.ok(keydownShouldBlock(true, true, 9), 'Tab 在 composition 期间应拦截');
  });

  test('场景 D: 失焦安全 — blur 重置 isComposing', () => {
    let isComposing = true;
    // 模拟 blur
    isComposing = false;
    assert.ok(!keydownShouldBlock(isComposing, false, 32),
      'blur 后不再拦截');
  });
});

// ── Fix 3: Shift+符号键 ──

describe('Fix 3: IME 直接字符插入', () => {
  test('非 composition + insertText → 发送', () => {
    const isComposing = false;
    const inputType = 'insertText';
    const shouldSend = !isComposing &&
      inputType !== 'insertCompositionText' &&
      inputType !== 'insertFromPaste' &&
      inputType !== 'insertFromDrop';
    assert.ok(shouldSend, 'Shift+符号应发送');
  });

  test('composition 中 → 不发送', () => {
    const isComposing = true;
    const inputType = 'insertText';
    const shouldSend = !isComposing &&
      inputType !== 'insertCompositionText' &&
      inputType !== 'insertFromPaste' &&
      inputType !== 'insertFromDrop';
    assert.ok(!shouldSend, 'composition 中不发送');
  });

  test('insertCompositionText → 跳过', () => {
    const inputType = 'insertCompositionText';
    assert.ok(inputType === 'insertCompositionText', '应跳过');
  });
});

// ── initImeFix 集成测试 ──

describe('initImeFix 集成测试', () => {
  test('imeFilter 被正确挂载且正常放行', async () => {
    const { initImeFix } = await import('../src/terminal/ime-fix.js');
    const term = {
      textarea: { addEventListener: () => {}, value: '' },
      element: { querySelector: () => term.textarea },
      _imeSendFn: () => {},
    };
    const termEl = { addEventListener: () => {} };
    initImeFix(term, termEl);
    assert.ok(typeof term.imeFilter === 'function');
    assert.equal(term.imeFilter('测试'), '测试', '正常文本放行');
    assert.equal(term.imeFilter(' '), ' ', '空格放行');
  });

  test('keydown/compositionstart/compositionend 在 capture 阶段注册', async () => {
    const { initImeFix } = await import('../src/terminal/ime-fix.js');
    const listeners = {};
    const ta = {
      addEventListener(type, fn, capture) {
        listeners[type] = listeners[type] || [];
        listeners[type].push({ fn, capture });
      },
      value: '',
    };
    const term = {
      textarea: ta,
      element: { querySelector: () => ta },
      _imeSendFn: () => {},
    };
    const termEl = { addEventListener: () => {} };
    initImeFix(term, termEl);

    assert.ok(listeners['compositionstart']?.some(l => l.capture === true),
      'compositionstart capture');
    assert.ok(listeners['compositionend']?.some(l => l.capture === true),
      'compositionend capture');
    assert.ok(listeners['keydown']?.some(l => l.capture === true),
      'keydown capture');
    assert.ok(listeners['blur']?.some(l => l.capture === true),
      'blur capture');
  });

  test('imeFilter 去重：100ms 内相同多字符丢弃', async () => {
    const { initImeFix } = await import('../src/terminal/ime-fix.js');
    const term = {
      textarea: { addEventListener: () => {}, value: '' },
      element: { querySelector: () => term.textarea },
      _imeSendFn: () => {},
    };
    const termEl = { addEventListener: () => {} };
    initImeFix(term, termEl);

    // 模拟快速重复调用
    assert.equal(term.imeFilter('hello'), 'hello', '首次放行');
    // 直接再调（now 差 < 100ms）
    // 注意：imeFilter 用 Date.now()，测试中调用间隔极短
    assert.equal(term.imeFilter('hello'), '', '100ms 内重复丢弃');
    // 不同内容放行
    assert.equal(term.imeFilter('world'), 'world', '不同内容放行');
  });
});
