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
//
// 修复 3 改进: 移除 insertText 条件守卫，依赖 Fix 4 去重
//   根因：旧的 recentlyComposed 窗口守卫和 lastKeyDownCode 判断都不可靠。
//   修复：完全移除 insertText 条件守卫。依赖 Fix 4 的 _xtermSentData 去重。
//
// 修复 5: compositionend 后记录组合文本，Fix 3 剥离前缀防重复
//   根因：compositionend 后 textarea 残留已提交的组合文本。用户后续
//         按键触发 input 事件时，Fix 3 读到残留文本+新字符，整串发送 →
//         重复输出。例：输入"我们"后按空格 → textarea 有"我们 " →
//         Fix 3 发送"我们 " → 终端显示"我们我们 "。
//
//   错误方案尝试：
//     v2: 清空 textarea + 手动发送 → xterm 也发送 → 双重输出
//     v3: 清空 textarea 不发送 → xterm 的 _handleAnyTextareaChanges()
//         检测到 textarea 从有内容变空，认为用户删除了所有文字 →
//         发退格序列 → 中文被立即删除 → "无法输入中文"
//     v4: stopImmediatePropagation + 手动发送 → xterm 的 handler
//         注册在先（同元素同目标按注册顺序执行），我们的 handler 在后
//         → 无法阻止已经执行过的 xterm handler → 仍然双重输出
//
//   正确方案 (v5): 不清空 textarea，不在 compositionend 中手动发送。
//         xterm 的 CompositionHandler 通过 onData 正常发送组合结果。
//         我们在 compositionend 中只记录 _lastCompositionText。
//         Fix 3 在 compositionend 后的 input 事件中，检查 textarea
//         是否以 _lastCompositionText 开头，若是则只发送新增部分
//         （剥离已发送的组合文本前缀），避免重复。
//         若 textarea 内容与 _lastCompositionText 完全相同 → 跳过
//         （IME 回写，无新输入）。
//
//   Caps Lock 场景：IME 将拼音 "wo men" 提交到 textarea → xterm 通过
//   onData 发送 → tab-manager.js onData 路径的拼音过滤器去空格 → "women"。
//   _lastCompositionText 记录为 "wo men"，Fix 3 剥离前缀正常工作。
//
// 补充：_compositionJustEnded 安全重置
//   修复：compositionend 后 200ms setTimeout 自动重置标志。

export function initImeFix(term, termEl) {
  const ta = term.textarea || (term.element && term.element.querySelector('textarea'));
  if (!ta) return;

  // ── 修复 4 基础设施：跟踪 xterm onData 最近发送 ──
  term._xtermSentData = '';
  term._xtermSentTime = 0;

  // 暴露 composition 结束时间，供 tab-manager.js onData 路径做双发去重
  term._compositionEndedAt = 0;

  // ── 修复 1: 粘贴后清空 textarea + 通知 mention 禁止触发 ──
  ta.addEventListener('paste', () => {
    if (term._mentionController) term._mentionController.setPasting(true);
    requestAnimationFrame(() => {
      ta.value = '';
      if (term._mentionController) term._mentionController.setPasting(false);
    });
  });

  // ── 修复 2: composition 期间拦截非 229 keydown ──
  let isComposing = false;
  let _compositionEndedAt = 0;

  let _compositionJustEnded = false;

  // ★ 修复 5: 记录 compositionend 时的组合文本，供 Fix 3 剥离前缀
  let _lastCompositionText = '';

  ta.addEventListener('compositionstart', () => {
    isComposing = true;
    _compositionJustEnded = false;
    _lastCompositionText = '';
  }, true);

  ta.addEventListener('compositionend', () => {
    isComposing = false;
    _compositionEndedAt = Date.now();
    term._compositionEndedAt = _compositionEndedAt;
    _compositionJustEnded = true;

    setTimeout(() => { _compositionJustEnded = false; }, 200);

    // ★ 修复 5 v5: 只记录组合文本，不清空 textarea，不手动发送
    //
    // 不清空 textarea：xterm 的 _handleAnyTextareaChanges() 会检测
    // textarea 内容变化，如果从有内容变空，xterm 认为用户删除了文字，
    // 发送退格序列 → 中文被删掉。
    //
    // 不手动发送：xterm 的 CompositionHandler 在 compositionend handler
    // 中通过 onData 发送组合结果（它注册在先，先于我们执行），
    // 手动发送会导致双重输出。
    //
    // 只记录 _lastCompositionText：Fix 3 在后续 input 事件中检查
    // textarea 是否以组合文本开头，剥离前缀只发新增部分。
    // ★ 修复 5 v5 改进：去掉时间窗口过期机制，改为事件驱动消费——
    // _lastCompositionText 在以下时机清除：被 Fix 3 消费、新 composition 开始、失焦。
    // 根因：200ms 窗口过期后，textarea 中陈旧的组合文本不再被剥离，
    // 用户按空格/数字时 Fix 3 读取 ta.value 含陈旧前缀 → 重复输出。
    const value = ta.value;
    if (value) {
      _lastCompositionText = value;
    }
  }, true);

  // 安全：失焦时重置 IME 状态
  ta.addEventListener('blur', () => {
    isComposing = false;
    _compositionJustEnded = false;
    _lastCompositionText = '';
  }, true);

  ta.addEventListener('keydown', (e) => {
    const shouldBlock = (isComposing || e.isComposing || _compositionJustEnded) &&
        e.keyCode !== 229 && e.keyCode !== 16 && e.keyCode !== 17 && e.keyCode !== 18;
    if (shouldBlock) {
      e.stopImmediatePropagation();
      if (_compositionJustEnded && !isComposing && !e.isComposing) {
        _compositionJustEnded = false;
      }
      if (e.ctrlKey && e.keyCode === 67) {
        isComposing = false;
      }
    }
  }, true);

  // ── 修复 3 + 修复 4 + 修复 5: IME 直接字符插入捕获 + 去重 + 剥离前缀 ──
  //
  // ★ 核心问题: WKWebView 中 keydown 的 preventDefault() 无法阻止字符插入 textarea，
  // xterm 的 _inputEvent handler (textarea capture) 会再次 triggerDataEvent → 重复输出。
  // 解决方案: 不依赖在 termEl 上 stopImmediatePropagation 阻止 xterm（因为 textarea
  // 和 termEl 是不同元素，stop 在 termEl 上无法阻止 textarea 上的 handler），
  // 而是在 tab-manager.js 的 ptyWriteDedup 中统一去重——onData 和 _imeSendFn
  // 两条路径汇合到同一个去重函数，30ms 窗口内相同数据只写一次 PTY。
  if (termEl) {
    termEl.addEventListener('input', (e) => {
      if (e.isComposing || e.inputType === 'insertCompositionText') return;
      if (e.inputType === 'insertFromPaste' || e.inputType === 'insertFromDrop') return;

      const value = ta.value;
      if (!value) return;

      // ★ 修复 5: compositionend 后，textarea 可能仍包含组合文本 + 新字符
      // 例：输入"我们"后按空格 → textarea 有"我们 " →
      //     xterm 已通过 onData 发送"我们"，Fix 3 只应发送" "
      //
      // 策略（v5 改进：去掉 200ms 时间窗口，改为事件驱动消费）：
      // 1. textarea === _lastCompositionText → IME 回写，无新输入 → 跳过
      // 2. textarea 以 _lastCompositionText 开头 → 剥离前缀，只发新增部分
      // 3. _lastCompositionText 为空（已消费 / compositionstart / blur）→ 正常处理
      // 消费后清除 _lastCompositionText，防止后续 input 事件重复剥离。
      if (_lastCompositionText) {
        if (value === _lastCompositionText) {
          // IME 回写，与组合文本完全相同 → 跳过并清空
          _lastCompositionText = '';
          ta.value = '';
          return;
        }
        if (value.startsWith(_lastCompositionText)) {
          // 剥离组合文本前缀，只处理新增部分
          const newData = value.slice(_lastCompositionText.length);
          _lastCompositionText = '';
          ta.value = '';
          if (!newData) return;
          // 新增部分通过 Fix 4 去重检查后发送
          const now = Date.now();
          if (newData === term._xtermSentData && (now - term._xtermSentTime) < 100) {
            return; // xterm 已发送
          }
          if (typeof term._imeSendFn === 'function') {
            const filtered = term.imeFilter ? term.imeFilter(newData) : newData;
            if (filtered) term._imeSendFn(filtered);
          }
          return;
        }
      }

      // ★ 修复 4: 跳过 xterm onData 已发送的重复数据
      const now = Date.now();
      if (value === term._xtermSentData && (now - term._xtermSentTime) < 100) {
        ta.value = '';
        return;
      }

      // ★ 修复 3: IME 直接插入字符
      if (typeof term._imeSendFn === 'function') {
        const filtered = term.imeFilter ? term.imeFilter(value) : value;
        if (filtered) term._imeSendFn(filtered);
      }
      ta.value = '';
    }, true);
  }

  // ── imeFilter: 去重复 ──
  let lastSentData = '';
  let lastSentTime = 0;
  term.imeFilter = (data) => {
    const now = Date.now();
    if (data === lastSentData && (now - lastSentTime) < 100) {
      return '';
    }
    lastSentData = data;
    lastSentTime = now;
    return data;
  };
}
