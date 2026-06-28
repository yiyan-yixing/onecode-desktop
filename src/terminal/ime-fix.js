// CJK 输入法修复（两项）。
//
// Fix 1: paste 后 textarea 残留导致 compositionend 重复发送
// 原理：xterm 在 paste 时从 clipboardData 读取并通过 term.onData() 发送，但会把
// 粘贴文本残留在 helper textarea.value。下一次 IME compositionend 触发时，
// xterm 的 compositionHelper 读取 textarea.value，导致旧粘贴内容被重复发送。
// 修复：paste 后用 requestAnimationFrame 清空 textarea.value（xterm 已先处理）。
//
// Fix 2: 非 IME 按键（Caps Lock/中/英切换键等）导致组合输入重复发送
// 原理：当 IME 组合输入进行中（_isComposing=true），如果收到非 IME 按键
// （keyCode != 229 且 != 16/17/18），xterm 的 CompositionHelper.keydown()
// 调用 _finalizeComposition(false) 立即发送组合文本。随后 compositionend 事件
// 触发 _finalizeComposition(true)，其 setTimeout 回调再次读取 textarea.value
// 并重复发送同样的数据。这是 xterm.js 的已知 bug（_finalizeComposition(false)
// 未设置 _dataAlreadySent，导致后续 _finalizeComposition(true) 无法去重）。
// 修复：跟踪组合状态和"过早终结"标志，在 compositionend 中检测到过早终结时
// 同步清空 textarea.value，使 setTimeout 回调读到空字符串而跳过发送。
//
// Desktop 版本：xterm 内置 compositionHelper 在桌面端能正确处理 CJK 基本流程，
// 故**不**添加 compositionstart/compositionend/keydown 的完全拦截。

export function initImeFix(term) {
  const ta = term.textarea || (term.element && term.element.querySelector('textarea'));
  if (!ta) return;

  // ── Fix 1: paste 后清空 textarea 残留 ──
  ta.addEventListener('paste', () => {
    requestAnimationFrame(() => {
      // 等 xterm 自身 paste handler 跑完后再清空，避免影响本次粘贴发送
      ta.value = '';
    });
  });

  // ── Fix 2: 非 IME 按键导致组合输入重复发送 ──
  let isComposing = false;
  let compositionPrematurelyFinalized = false;

  // 跟踪组合状态（capture phase，在 xterm 之后执行）
  ta.addEventListener('compositionstart', () => {
    isComposing = true;
    compositionPrematurelyFinalized = false;
  }, true);

  ta.addEventListener('compositionend', () => {
    isComposing = false;
    if (compositionPrematurelyFinalized) {
      // xterm 的 _finalizeComposition(false) 已经在 keydown 中发送了数据。
      // 现在 compositionend 触发，xterm 的 _finalizeComposition(true) 会安排
      // setTimeout 重新读取 textarea.value 并发送——这就是重复的来源。
      // 同步清空 textarea.value，使 setTimeout 回调读到空字符串，跳过发送。
      ta.value = '';
      compositionPrematurelyFinalized = false;
    }
  }, true);

  // 检测组合期间的"过早终结"按键
  // keyCode 229 = IME 字符，16/17/18 = 修饰键——这些不会触发 _finalizeComposition(false)
  ta.addEventListener('keydown', (e) => {
    if (isComposing && e.keyCode !== 229 && e.keyCode !== 16 && e.keyCode !== 17 && e.keyCode !== 18) {
      compositionPrematurelyFinalized = true;
    }
  }, true);
}
