// xterm.js 初始化 + Aurora 主题（极光深度色系）。
// xterm 走 UMD 全局（script 标签加载），通过 window.Terminal / window.FitAddon 等访问。

export function createTerminal(containerEl) {
  const Terminal = window.Terminal;
  const FitAddon = window.FitAddon;
  const WebLinksAddon = window.WebLinksAddon;

  if (!Terminal) {
    throw new Error('xterm.js 未加载（static/xterm.min.js 缺失，请先 npm run copy-static）');
  }

  const term = new Terminal({
    scrollback: 3000,
    fontSize: 13,
    lineHeight: 1.0,
    fontFamily: "'JetBrains Mono','SF Mono','Fira Code',Consolas,monospace",
    // Aurora 深度色系 — 终端始终深色，与极光背景融合
    theme: {
      background: '#0A0B0F',       // aurora-void
      foreground: '#C8D6F0',       // text-frost
      cursor: '#7DD3FC',           // id-sky
      cursorAccent: '#0A0B0F',
      selectionBackground: 'rgba(125,211,252,.20)',
      selectionForeground: '#E8F0FF',
      black: '#0E1017',            // aurora-abyss
      red: '#ff6b6b',
      green: '#10B981',            // id-emerald
      yellow: '#f7c948',           // id-amber
      blue: '#4d9cff',
      magenta: '#A78BFA',          // id-violet
      cyan: '#22d3ee',            // id-cyan
      white: '#C8D6F0',
      brightBlack: '#2D3348',      // aurora-overcast
      brightRed: '#ff8787',
      brightGreen: '#52e8b8',
      brightYellow: '#ffd55a',
      brightBlue: '#79b4ff',
      brightMagenta: '#d8b4fe',
      brightCyan: '#67e8f9',
      brightWhite: '#E8F0FF',
    },
    allowProposedApi: true,
    // ⚠️ 禁用 allowTransparency——强制不透明背景可让 canvas 渲染器
    //    跳过 alpha 合成，大幅提升 WKWebView 下的重绘性能，消除闪烁。
    allowTransparency: false,
    convertEol: false,
    // 新输出时滚动到底部（claude TUI 全屏重绘时跟随）
    scrollOnUserInput: true,
    // ⚠️ 滚动灵敏度由自定义加速器（attachCustomWheelEventHandler）接管，
    //    此处禁用 xterm 内置灵敏度，避免双重系统冲突。
    scrollSensitivity: 1,
    fastScrollModifier: undefined,
    fastScrollSensitivity: 1,
  });

  const fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);

  // 覆写 fit()：fit 后自动减一行，防止最后一行被状态栏亚像素遮挡
  // 所有 fitAddon.fit() 调用（含 tab-manager）都会走此路径，无需逐处修改
  const _origFit = fitAddon.fit.bind(fitAddon);
  fitAddon.fit = function () {
    _origFit();
    const p = fitAddon.proposeDimensions();
    if (p && p.rows > 1 && (term.rows !== p.rows - 1 || term.cols !== p.cols)) {
      term.resize(p.cols, p.rows - 1);
    }
  };

  if (WebLinksAddon) term.loadAddon(new WebLinksAddon.WebLinksAddon());
  term.open(containerEl);

  // ── 自定义滚轮加速器（xterm 5.5 官方 API）──
  //
  // 使用 attachCustomWheelEventHandler 在 xterm 内部处理 wheel 之前拦截。
  // 返回 false → xterm 不处理此事件；返回 true → xterm 正常处理。
  // 通过 term.scrollLines() 行级 API 驱动滚动，而非直接操作 viewport.scrollTop（像素级），
  // 消除像素/行级粒度冲突导致的抖动、边界速度跳变、_ignoreNextScrollEvent 竞态等问题。
  const SCROLL_BOOST = 5;   // 基础倍速
  const FAST_BOOST = 15;    // Alt 加速倍速

  // 获取行高（resize 时更新）
  let _rowHeight = 0;
  const updateRowHeight = () => {
    try {
      _rowHeight = term._core._renderService.dimensions.css.cell.height;
    } catch (_) {}
  };
  term.onResize(() => updateRowHeight());
  updateRowHeight();

  term.attachCustomWheelEventHandler((e) => {
    // vim/tmux 鼠标追踪模式：滚轮应发给 PTY，交给 xterm 处理
    if (term.coreMouseService && term.coreMouseService.areMouseEventsActive) return true;
    // 水平滚动或 Shift 滚动：交给 xterm 处理
    if (e.deltaY === 0 || e.shiftKey) return true;
    // 无 scrollback 内容：交给 xterm 处理
    if (term.buffer.active.length <= term.rows) return true;

    const boost = e.altKey ? FAST_BOOST : SCROLL_BOOST;
    const linesToScroll = Math.round((e.deltaY * boost) / Math.max(1, _rowHeight));
    if (linesToScroll !== 0) {
      term.scrollLines(linesToScroll, true); // suppressScrollEvent: 避免 onScroll 二次触发
    }
    return false; // 始终阻止 xterm 默认处理
  });

  // Canvas 渲染器（WKWebView 下 WebGL 闪烁，统一用 canvas）
  fitAddon.fit();

  injectTermStyles();

  // 可见性/尺寸变化时 auto-fit（Tab 切换、窗口 resize）
  // 防反馈循环：双重守卫——像素尺寸未变则完全跳过，行列未变则跳过 fit
  let resizeObserver = null;
  if (typeof ResizeObserver !== 'undefined') {
    let timer = null;
    let lastW = 0;
    let lastH = 0;
    let lastCols = 0;
    let lastRows = 0;
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        // 不可见终端（display:none / opacity:0）尺寸为 0 — 跳过
        if (containerEl.offsetWidth === 0 || containerEl.offsetHeight === 0) return;
        // 第一层守卫：像素尺寸完全没变 → 连 proposeDimensions 都跳过，避免强制回流
        const w = containerEl.offsetWidth;
        const h = containerEl.offsetHeight;
        if (w === lastW && h === lastH) return;
        // 第二层守卫：行列数没变 → 不调 fit
        const proposed = fitAddon.proposeDimensions();
        if (proposed && (proposed.cols !== lastCols || proposed.rows !== lastRows)) {
          lastCols = proposed.cols;
          lastRows = proposed.rows;
          lastW = w;
          lastH = h;
          try {
            fitAddon.fit();
          } catch (_) {}
        } else {
          // 行列没变但像素尺寸变了（极少见），记录新像素值
          lastW = w;
          lastH = h;
        }
      }, 100);
    });
    ro.observe(containerEl);
    resizeObserver = ro;
    // 初始化记录
    lastW = containerEl.offsetWidth;
    lastH = containerEl.offsetHeight;
    const initial = fitAddon.proposeDimensions();
    if (initial) {
      lastCols = initial.cols;
      lastRows = initial.rows;
    }
  }

  return { term, fitAddon, resizeObserver };
}

function injectTermStyles() {
  if (document.getElementById('onecode-term-styles')) return;
  const s = document.createElement('style');
  s.id = 'onecode-term-styles';
  s.textContent =
    /* will-change + translateZ 强制 WKWebView 将 viewport 提升为独立 GPU 合成层，
       启用硬件异步滚动 — 滚动时不需要等待主线程 paint 完成 */
    '.xterm-viewport{overscroll-behavior:none!important;scrollbar-width:none!important;' +
    '-webkit-overflow-scrolling:touch!important;' +
    'will-change:scroll-position!important;transform:translateZ(0)!important}' +
    '.xterm-viewport::-webkit-scrollbar{display:none!important}' +
    '.xterm-helpers{position:absolute!important;opacity:0}' +
    '.xterm{-webkit-font-smoothing:antialiased!important;-moz-osx-font-smoothing:grayscale!important;' +
    'text-rendering:optimizeLegibility!important}' +
    '.xterm-rows{font-variant-ligatures:none!important;letter-spacing:0!important}';
  document.head.appendChild(s);
}
