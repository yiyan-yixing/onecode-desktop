// xterm.js 初始化 + Aurora 主题（极光深度色系）。
// xterm 走 UMD 全局（script 标签加载），通过 window.Terminal / window.FitAddon 等访问。

export function createTerminal(containerEl) {
  const Terminal = window.Terminal;
  const FitAddon = window.FitAddon;
  const WebLinksAddon = window.WebLinksAddon;
  const WebglAddon = window.WebglAddon;

  if (!Terminal) {
    throw new Error('xterm.js 未加载（static/xterm.min.js 缺失，请先 npm run copy-static）');
  }

  const term = new Terminal({
    scrollback: 5000,
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
    fastScrollModifier: 'alt',
    fastScrollSensitivity: 5,
  });

  const fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  if (WebLinksAddon) term.loadAddon(new WebLinksAddon.WebLinksAddon());
  term.open(containerEl);

  // ⚠️ 在 Tauri WKWebView 中，WebGL 渲染器会导致闪烁和滚动异常。
  // 统一使用 canvas 渲染器，稳定可靠。
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
        if (containerEl.offsetWidth > 0 && containerEl.offsetHeight > 0) {
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
            // 保存 scroll 位置，fit 后恢复（防止 reflow 跳动）
            const wasAtBottom = term.buffer.active.viewportY + term.rows >= term.buffer.active.length;
            const savedY = term.buffer.active.viewportY;
            try {
              fitAddon.fit();
            } catch (_) {}
            if (!wasAtBottom) {
              try { term.scrollLines(savedY - term.buffer.active.viewportY, false); } catch (_) {}
            }
          } else {
            // 行列没变但像素尺寸变了（极少见），记录新像素值
            lastW = w;
            lastH = h;
          }
        }
      }, 150);
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
    '.xterm-viewport{overscroll-behavior:none!important;scrollbar-width:none!important}' +
    '.xterm-viewport::-webkit-scrollbar{display:none!important}' +
    '.xterm-helpers{position:absolute!important;opacity:0}' +
    '.xterm{-webkit-font-smoothing:antialiased!important;-moz-osx-font-smoothing:grayscale!important;' +
    'text-rendering:optimizeLegibility!important}' +
    '.xterm-rows{font-variant-ligatures:none!important;letter-spacing:0!important}';
  document.head.appendChild(s);
}
