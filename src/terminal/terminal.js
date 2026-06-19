// xterm.js 初始化 + 主题（提取自 gateway/static/term.js，主题色改用 Cowork 暖色调色板）。
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
    // 深色终端块（Cowork --void 系），暖色 UI 包裹深色终端
    theme: {
      background: '#111119',
      foreground: '#D4DFF0',
      cursor: '#7DD3FC',
      cursorAccent: '#111119',
      selectionBackground: 'rgba(125,211,252,.22)',
      selectionForeground: '#eaf0ff',
      black: '#16161F',
      red: '#ff6b6b',
      green: '#10B981',       // emerald
      yellow: '#f7c948',
      blue: '#4d9cff',
      magenta: '#A78BFA',     // lavender
      cyan: '#22d3ee',
      white: '#D4DFF0',
      brightBlack: '#343446',
      brightRed: '#ff8787',
      brightGreen: '#52e8b8',
      brightYellow: '#ffd55a',
      brightBlue: '#79b4ff',
      brightMagenta: '#d8b4fe',
      brightCyan: '#67e8f9',
      brightWhite: '#eef2ff',
    },
    allowProposedApi: true,
    allowTransparency: true,
    convertEol: false,
  });

  const fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  if (WebLinksAddon) term.loadAddon(new WebLinksAddon.WebLinksAddon());
  term.open(containerEl);

  // WebGL 渲染（Safari 跳过，scroll 渲染 bug）
  const isSafari = /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
  if (!isSafari && WebglAddon) {
    try {
      const webgl = new WebglAddon.WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch (_) {
      // 回退到 canvas 渲染
    }
  }
  fitAddon.fit();

  injectTermStyles();

  // 可见性/尺寸变化时 auto-fit（Tab 切换、窗口 resize）
  if (typeof ResizeObserver !== 'undefined') {
    let timer = null;
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        if (containerEl.offsetWidth > 0 && containerEl.offsetHeight > 0) {
          try {
            fitAddon.fit();
          } catch (_) {}
        }
      }, 100);
    });
    ro.observe(containerEl);
  }

  return { term, fitAddon };
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
