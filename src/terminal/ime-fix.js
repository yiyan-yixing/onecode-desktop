// CJK 输入法修复（两项）。
//
// Fix 1: paste 后 textarea 残留导致 compositionend 重复发送
// 原理：xterm 在 paste 时从 clipboardData 读取并通过 term.onData() 发送，但会把
// 粘贴文本残留在 helper textarea.value。下一次 IME compositionend 触发时，
// xterm 的 compositionHelper 读取 textarea.value，导致旧粘贴内容被重复发送。
// 修复：paste 后用 requestAnimationFrame 清空 textarea.value（xterm 已先处理）。
//
// Fix 2: 中文 IME 组合输入被中断时发送含空格的原始拼音 + 重复发送
// 原理：macOS 中文输入法在按 Caps Lock（中/英切换键）中断组合输入时：
//   (a) xterm 的 _finalizeComposition(false) 在 keydown 中读取 textarea，
//       此时 textarea 包含原始拼音文本（如 "wo men"），音节间有空格分隔符。
//   (b) compositionend → _finalizeComposition(true) → setTimeout 再次发送。
//   (c) input 事件触发 _inputEvent 也发送数据。
//   结果：终端收到 "wo men"（含多余空格）且可能重复。
// 修复：在 onData 层面（imeFilter）做两件事——
//   1. 去空格：在 IME 组合期间或结束后 100ms 内，如果发送的数据是纯 ASCII
//      字母+空格（原始拼音特征），去掉空格（"wo men" → "women"）
//   2. 去重复：100ms 内如果收到和上次完全相同的多字符数据，丢弃（只发一次）
//   优势：不干预 xterm 事件处理流程，不受事件触发顺序影响，
//         兼容 keydown 先于/后于 compositionend 的两种时序。

export function initImeFix(term) {
  const ta = term.textarea || (term.element && term.element.querySelector('textarea'));
  if (!ta) return;

  // ── Fix 1: paste 后清空 textarea 残留 ──
  ta.addEventListener('paste', () => {
    requestAnimationFrame(() => {
      ta.value = '';
    });
  });

  // ── Fix 2: IME composition 数据层过滤 ──
  let isComposing = false;
  let imeEndWindow = 0;
  let lastSentData = '';
  let lastSentTime = 0;

  ta.addEventListener('compositionstart', () => {
    isComposing = true;
  }, true);

  ta.addEventListener('compositionend', () => {
    isComposing = false;
    imeEndWindow = Date.now() + 100;
  }, true);

  term.imeFilter = (data) => {
    const now = Date.now();

    // 去空格：IME 组合期间或结束后 100ms 内，
    // 如果数据是纯 ASCII 字母+空格（原始拼音特征），去掉空格
    // 例："wo men" → "women"
    // 安全性：仅在 IME 窗口内对纯字母+空格生效，不影响正常英文输入
    if ((isComposing || now < imeEndWindow) && /^[a-z ]+$/i.test(data) && data.includes(' ')) {
      data = data.replace(/ /g, '');
    }

    // 去重复：100ms 内收到和上次完全相同的多字符数据，丢弃
    // 防止 _finalizeComposition(false) + setTimeout/_inputEvent 重复发送
    if (data.length > 1 && data === lastSentData && (now - lastSentTime) < 100) {
      return '';
    }

    lastSentData = data;
    lastSentTime = now;
    return data;
  };
}
