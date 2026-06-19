// CJK 输入法 / paste 修复（提取自 gateway/static/term.js fixDesktopPaste）。
//
// 原理：xterm 在 paste 时从 clipboardData 读取并通过 term.onData() 发送，但会把
// 粘贴文本残留在 helper textarea.value。下一次 IME compositionend 触发时，
// xterm 的 compositionHelper 读取 textarea.value，导致旧粘贴内容被重复发送。
// 修复：paste 后用 requestAnimationFrame 清空 textarea.value（xterm 已先处理）。
//
// Desktop 版本：xterm 内置 compositionHelper 在桌面端能正确处理 CJK，
// 故**不**添加 compositionstart/compositionend/keydown 拦截。

export function initImeFix(term) {
  const ta = term.textarea || (term.element && term.element.querySelector('textarea'));
  if (!ta) return;
  ta.addEventListener('paste', () => {
    requestAnimationFrame(() => {
      // 等 xterm 自身 paste handler 跑完后再清空，避免影响本次粘贴发送
      ta.value = '';
    });
  });
}
