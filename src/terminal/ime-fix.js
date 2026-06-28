// CJK 输入法修复 — 基于 onecode/agent-runtime/gateway/static/term.js 的验证方案。
//
// 核心原则（来自 onecode 实际调试）：
//   桌面端 xterm.js 内置的 CompositionHelper 能正确处理 CJK 输入。
//   不要在 compositionstart/compositionend/input 事件中干预 xterm 的处理流程，
//   否则会与 xterm 内部的 setTimeout(0) 读 textarea 产生竞争，
//   导致中文无法输入或重复发送。
//
// 修复 1: 粘贴后清空 textarea（fixDesktopPaste）
//   根因：xterm 读 clipboardData 发送粘贴内容，但不清空 textarea。
//         后续 compositionend 时 xterm 读 textarea 把旧粘贴文本
//         和新 composition 文本拼在一起发送 → 重复。
//   修复：paste 后 rAF 清空 textarea。
//
// 修复 2: composition 期间拦截非 229 keydown（fixImeStaleText）
//   根因：composition 期间任何非 keyCode 229 的按键（包括空格确认键）
//         触发 xterm 的 _finalizeComposition(false)，同步读 textarea
//         发送陈旧的预编辑文本（如拼音 "wo men" 含分隔符）。
//         然后 compositionend 触发 _finalizeComposition(true) 又发送
//         正确文本（如 "women"）→ 结果："wo menwomen"（重复 + 陈旧）。
//   修复：在 textarea 的 keydown capture 阶段拦截非 229 keydown，
//         用 stopImmediatePropagation 阻止 xterm 处理。
//         不调 preventDefault — IME 需要按键在浏览器层面正常处理
//         才能正确提交 composition（IME 在 OS 层面先于浏览器 keydown
//         处理按键，所以 stopImmediatePropagation 不影响 IME 行为）。
//
//   补充（Caps Lock 问题）：
//     macOS 上按 Caps Lock 切换输入法时，事件顺序为：
//     1. OS/IME 先处理 Caps Lock → 切换输入法 → 触发 compositionend
//     2. compositionend 重置 isComposing = false
//     3. Caps Lock keydown 到达浏览器 → isComposing 已 false → Fix 2 不拦截
//     4. xterm keydown handler 触发 _finalizeComposition(false) → 发送陈旧拼音
//     修复：增加 _compositionJustEnded 标志 — compositionend 后置 true，
//     下一个非 229 keydown 仍然拦截（这是导致 composition 结束的那个按键），
//     拦截后重置标志。这覆盖了 Caps Lock、Enter、空格等所有中断键。
//
// 修复 3: IME 直接字符插入捕获（Shift+符号键如 ！@#￥%）
//   根因：IME 激活时按 Shift+1，xterm 的 keydown handler 看到
//         isComposing=true → 跳过 + 不调 preventDefault → IME 产出
//         全角字符插入 textarea → input 事件 → xterm 也跳过 → 字符丢失。
//   修复：termEl 的 input capture 阶段检测 IME 直接插入的字符，
//         手动发送到 PTY 并清空 textarea。
//
// 修复 4: 输入去重（防止 xterm onData + Fix 3 双发）
//   根因：Fix 3 的 input capture 处理器会捕获所有非 composition/paste
//         的 input 事件。当 xterm 的 keydown 处理了字符但未调
//         preventDefault 时，字符同时在 textarea 里 AND 已通过 onData
//         发到 PTY → Fix 3 再发一遍 → 重复。单字符（如空格）因为
//         原 imeFilter 只对 data.length > 1 去重而完全绕过去重。
//   修复：(a) term 对象上记录 xterm onData 最近发送的数据和时间戳，
//         Fix 3 在发送前检查 xterm 是否刚发过同样的数据。
//         (b) imeFilter 去重覆盖单字符（100ms 窗口）。

export function initImeFix(term, termEl) {
  const ta = term.textarea || (term.element && term.element.querySelector('textarea'));
  if (!ta) return;

  // ── 修复 4 基础设施：跟踪 xterm onData 最近发送 ──
  // tab-manager.js 的 onData 回调写入这两个字段，Fix 3 读取以避免双发
  term._xtermSentData = '';
  term._xtermSentTime = 0;

  // 暴露 composition 结束时间，供 tab-manager.js onData 路径做双发去重
  term._compositionEndedAt = 0;

  // ── 修复 1: 粘贴后清空 textarea + 通知 mention 禁止触发 ──
  ta.addEventListener('paste', () => {
    // 通知 mention controller 粘贴中（防止含 @ 文本触发弹窗闪烁）
    if (term._mentionController) term._mentionController.setPasting(true);
    requestAnimationFrame(() => {
      ta.value = '';
      if (term._mentionController) term._mentionController.setPasting(false);
    });
  });

  // ── 修复 2: composition 期间拦截非 229 keydown ──
  let isComposing = false;
  let _compositionEndedAt = 0; // 追踪最近一次 composition 结束时间

  // ★ Caps Lock 补充修复：composition 刚结束后的第一个非 229 keydown 也要拦截
  // macOS 上 Caps Lock 切换输入法时，compositionend 先于 Caps Lock keydown 到达，
  // 此时 isComposing 已经 false，Fix 2 不拦截 → xterm _finalizeComposition(false) 触发
  // → 发送陈旧拼音。此标志兜住这个间隙。
  let _compositionJustEnded = false;

  ta.addEventListener('compositionstart', () => {
    isComposing = true;
    _compositionJustEnded = false; // 新 composition 开始，清除标志
  }, true);

  ta.addEventListener('compositionend', () => {
    isComposing = false;
    _compositionEndedAt = Date.now();
    term._compositionEndedAt = _compositionEndedAt; // 同步到 term 对象
    // ★ 置标志：下一个非 229 keydown 仍然拦截
    // 这是导致 composition 结束的那个按键（如 Caps Lock、Enter、空格）
    _compositionJustEnded = true;
    // ⚠️ 不在这里清空 textarea！xterm 的 compositionend handler
    // 通过 setTimeout(0) 异步读取 textarea 获取最终文本。
    // 如果在 rAF 中提前清空，xterm 会读到空字符串 → 中文无法输入。
    // textarea 的清空由 xterm 自己负责。
  }, true);

  // 安全：失焦时重置 IME 状态，防止 isComposing 卡在 true
  ta.addEventListener('blur', () => {
    isComposing = false;
    _compositionJustEnded = false;
  }, true);

  ta.addEventListener('keydown', (e) => {
    // composition 期间 OR composition 刚结束后的第一个非 229 keydown：
    // 拦截以阻止 xterm 的 _finalizeComposition(false) 发送陈旧预编辑文本
    const shouldBlock = (isComposing || e.isComposing || _compositionJustEnded) &&
        e.keyCode !== 229 && e.keyCode !== 16 && e.keyCode !== 17 && e.keyCode !== 18;
    if (shouldBlock) {
      e.stopImmediatePropagation();
      // ★ 如果 composition 已结束但标志仍为 true，说明这是导致结束的按键
      // （如 Caps Lock），拦截这一次就够了，重置标志
      if (_compositionJustEnded && !isComposing && !e.isComposing) {
        _compositionJustEnded = false;
      }
      // 安全网：Ctrl+C 取消 composition 时重置 isComposing
      if (e.ctrlKey && e.keyCode === 67) {
        isComposing = false;
      }
    }
  }, true); // capture 阶段 — 在 xterm keydown handler 之前触发

  // ── 修复 3 + 修复 4: IME 直接字符插入捕获 + 去重 ──
  if (termEl) {
    termEl.addEventListener('input', (e) => {
      // composition 输入 — xterm 的 compositionend 处理器会处理
      if (e.isComposing || e.inputType === 'insertCompositionText') return;
      // 粘贴/拖放 — xterm 通过 clipboardData 处理
      if (e.inputType === 'insertFromPaste' || e.inputType === 'insertFromDrop') return;

      const value = ta.value;
      if (!value) return;

      // ★ 修复 4: 跳过 xterm onData 已发送的重复数据
      const now = Date.now();
      if (value === term._xtermSentData && (now - term._xtermSentTime) < 100) {
        ta.value = '';
        return;
      }

      // ★ 修复 3: 普通键盘 insertText — xterm 的 keydown handler 正常处理
      // 只有 IME 最近活跃时（composition 结束后 300ms 内）的 insertText
      // 才可能是 IME 直接插入（Shift+symbol），需要手动发送。
      // 非此窗口内的 insertText 说明 xterm 已通过 keydown 处理过了。
      const recentlyComposed = (now - _compositionEndedAt) < 300;
      if (e.inputType === 'insertText' && !recentlyComposed && !isComposing) {
        ta.value = '';
        return;
      }

      // IME 直接插入了字符（如 Shift+1 → ！），xterm 两条路径都跳过了
      if (typeof term._imeSendFn === 'function') {
        const filtered = term.imeFilter ? term.imeFilter(value) : value;
        if (filtered) term._imeSendFn(filtered);
      }
      ta.value = '';
    }, true);  // capture 阶段 — 在 textarea 任何处理器之前触发
  }

  // ── imeFilter: 去重复（覆盖单字符 + 多字符） ──
  // 仅用于 _imeSendFn 路径（Fix 3），不用于 onData 路径
  let lastSentData = '';
  let lastSentTime = 0;
  term.imeFilter = (data) => {
    const now = Date.now();
    // 去重复：100ms 内收到和上次完全相同的数据，丢弃
    if (data === lastSentData && (now - lastSentTime) < 100) {
      return '';
    }
    lastSentData = data;
    lastSentTime = now;
    return data;
  };
}
