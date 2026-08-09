// Monaco 编辑器最小原型（M3 探索）—— 纯前端，可回退。
//
// 【原型标注】这是探索性原型，验证 Monaco Editor（MIT）与现有 xterm 终端
// 体验的融合，不追求完整文件树。改动独立成模块，未触碰核心终端/会话逻辑。
//
// 技术选型：
//   - 直接使用 monaco-editor 的 AMD/min 构建（node_modules → src/static/vs，
//     由 scripts/copy-static.sh 同步），不走 @monaco-editor/react（避免引入 React）。
//   - 无打包器环境（frontendDist = src/，静态托管），因此用 AMD loader
//     （vs/loader.js）按需加载 vs/editor/editor.main.js。
//   - worker 走 same-origin 的 vs/base/worker/workerMain.js（CSP 友好，
//     不需要 blob:/data: worker URL）。
//
// 入口（见 main.js / palette.js）：
//   - 命令面板 (⌘K) → 「编辑器原型」
//   - 快捷键 ⌘E
//   - 空状态「编辑器原型」按钮

const SAMPLE_TITLE = 'monaco-prototype.demo.js';
const SAMPLE_LANG = 'javascript';
const SAMPLE_CONTENT = `/**
 * OneCode Desktop — Monaco 编辑器最小原型
 * ============================================
 * 这是内置示例文件（原型阶段，纯前端，不读真实文件），
 * 用于验证 Monaco Editor 与 xterm 终端体验的融合。
 *
 * 入口：
 *   • 命令面板 (⌘K) → 「编辑器原型」
 *   • 快捷键 ⌘E
 *   • 空状态「编辑器原型」按钮
 *
 * 后续完整集成文件树时的步骤（详见交付说明）：
 *   1. 复用右侧文件浏览器 fs_list_dir / fs_read_file
 *   2. 点击文件 → monacoPreview.open({ title, content, language })
 *   3. Tauri 侧补 fs::write 命令即可支持编辑保存
 */

// ── AI 员工调度台（演示片段）──────────────────────────────
const employees = [
  { name: '@dev',       role: '开发者',  status: 'running' },
  { name: '@qa',        role: '测试',    status: 'running' },
  { name: '@cfo',       role: '财务',    status: 'idle'    },
  { name: '@committer', role: '提交官',  status: 'idle'    },
];

function dispatch(agentName, task) {
  const agent = employees.find((a) => a.name === agentName);
  if (!agent) throw new Error(\`未知员工: \${agentName}\`);
  if (agent.status !== 'running') {
    console.warn(\`\${agentName} 未就绪，任务排队中\`);
    return { queued: true, task };
  }
  console.log(\`\${agentName} 领取任务: \${task}\`);
  return { queued: false, task, startedAt: Date.now() };
}

// 调度一次批量任务
const tasks = [
  ['@dev', '实现 Monaco 编辑器原型'],
  ['@qa',  '回归验证现有测试'],
  ['@cfo', '更新每日现金流'],
];

tasks.forEach(([name, task]) => dispatch(name, task));
`;

// 语言下拉（原型演示 Monaco 多语言高亮）
const LANGS = [
  { id: 'javascript', label: 'JavaScript' },
  { id: 'typescript', label: 'TypeScript' },
  { id: 'json',       label: 'JSON' },
  { id: 'css',        label: 'CSS' },
  { id: 'html',       label: 'HTML' },
  { id: 'markdown',   label: 'Markdown' },
  { id: 'rust',       label: 'Rust' },
  { id: 'python',     label: 'Python' },
  { id: 'yaml',       label: 'YAML' },
];

const LOADER_SRC = 'static/vs/loader.js';
const VS_PATH = 'static/vs';

export class MonacoPreviewController {
  constructor() {
    this._ready = null;          // Promise<monaco>
    this._overlay = null;
    this._editor = null;
    this._titleEl = null;
    this._langSelect = null;
    this._bodyEl = null;
    this._opened = false;
    this._styleEl = null;
    this._keyHandler = null;
    this._resizeHandler = null;
  }

  /** 注入样式 + 构建浮层 DOM。幂等。 */
  init() {
    this._injectStyles();
    this._buildOverlay();
  }

  // ── 生命周期 ────────────────────────────────────────────

  async open(opts = {}) {
    const title = opts.title || SAMPLE_TITLE;
    const content = opts.content ?? SAMPLE_CONTENT;
    const language = opts.language || SAMPLE_LANG;

    if (!this._overlay) this._buildOverlay();
    this._overlay.classList.add('on');
    if (this._titleEl) this._titleEl.textContent = title;

    let monaco;
    try {
      monaco = await this._ensureMonaco();
    } catch (e) {
      console.warn('[monaco-preview] 加载失败:', e);
      if (this._bodyEl) {
        this._bodyEl.textContent = '';
        const errDiv = document.createElement('div');
        errDiv.textContent = `Monaco 加载失败: ${String((e && e.message) || e)}`;
        errDiv.style.cssText = 'color:#f87171;padding:24px;font-size:12px;font-family:var(--font-mono)';
        this._bodyEl.appendChild(errDiv);
      }
      return;
    }

    if (!this._editor) {
      this._editor = monaco.editor.create(this._bodyEl, {
        value: content,
        language,
        theme: 'vs-dark',          // 与终端深色视觉一致
        automaticLayout: true,     // 容器 resize 自动 layout
        fontSize: 13,
        minimap: { enabled: true },
        scrollBeyondLastLine: false,
        wordWrap: 'on',
        padding: { top: 12 },
      });
    } else {
      this._editor.setValue(content);
      this._setLanguage(language);
    }

    this._opened = true;
    if (this._langSelect) this._langSelect.value = language;
    requestAnimationFrame(() => { try { this._editor.layout(); } catch (_) {} });
    try { this._editor.focus(); } catch (_) {}
  }

  /** 关闭浮层，焦点还给终端。 */
  close() {
    this._opened = false;
    if (this._overlay) this._overlay.classList.remove('on');
    // 让终端重新获得键盘焦点（恢复调度台体验）
    try {
      const ta = document.querySelector('.term-instance.active .xterm textarea');
      if (ta) ta.focus();
    } catch (_) {}
  }

  /** ⌘E 快捷键入口。 */
  toggle() {
    if (this._opened) this.close();
    else this.open();
  }

  /** 卸载浮层与全局监听（原型退出时调用；当前无调用方，预留） */
  destroy() {
    if (this._keyHandler) document.removeEventListener('keydown', this._keyHandler);
    if (this._resizeHandler) window.removeEventListener('resize', this._resizeHandler);
    if (this._editor) {
      try { this._editor.getModel()?.dispose(); } catch (_) {} // 隐式创建的 model 不会随 editor.dispose 释放
      try { this._editor.dispose(); } catch (_) {}
      this._editor = null;
    }
    if (this._overlay) { try { this._overlay.remove(); } catch (_) {} this._overlay = null; }
    if (this._styleEl) { try { this._styleEl.remove(); } catch (_) {} this._styleEl = null; }
    this._opened = false;
  }

  // ── 内部：Monaco 按需加载（AMD）────────────────────────

  /** 确保 monaco 已加载（AMD loader + editor.main.js），返回 monaco 全局。 */
  _ensureMonaco() {
    if (this._ready) return this._ready;
    this._ready = new Promise((resolve, reject) => {
      const existing = document.querySelector(`script[src="${LOADER_SRC}"]`);
      const loaderPromise = existing
        ? Promise.resolve()
        : new Promise((res, rej) => {
            const s = document.createElement('script');
            s.src = LOADER_SRC;
            s.onload = res;
            s.onerror = () => { try { s.remove(); } catch (_) {} rej(new Error(`${LOADER_SRC} 加载失败`)); };
            document.head.appendChild(s);
          });

      loaderPromise
        .then(() => {
          if (typeof window.require !== 'function') {
            throw new Error('monaco AMD loader 未就绪');
          }
          window.require.config({ paths: { vs: VS_PATH } });
          // worker 引导：所有语言 worker 都走同一份 same-origin bootstrap，
          // 避免 blob:/data: worker（CSP 默认只放行 'self'）。
          if (!self.MonacoEnvironment) {
            self.MonacoEnvironment = {
              getWorkerUrl: () => `${VS_PATH}/base/worker/workerMain.js`,
            };
          }
          window.require(['vs/editor/editor.main'], () => resolve(window.monaco));
        })
        .catch((err) => { this._ready = null; reject(err); });
    });
    return this._ready;
  }

  // ── 内部：UI ────────────────────────────────────────────

  _injectStyles() {
    if (this._styleEl || document.getElementById('mp-style')) return;
    const style = document.createElement('style');
    style.id = 'mp-style';
    style.textContent = `
.mp-overlay {
  position: absolute;
  inset: 0;
  z-index: 150;                 /* 高于 fe-preview(100) */
  display: none;
  flex-direction: column;
  background: var(--abyss);
  animation: fe-preview-in 200ms var(--ease-out);
}
.mp-overlay.on { display: flex; }
.mp-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  background: var(--void);
  border-bottom: 1px solid rgba(255,255,255,.08);
  flex: 0 0 auto;
}
.mp-badge {
  font-size: 10px;
  line-height: 1;
  color: var(--em);
  border: 1px solid var(--em);
  border-radius: 10px;
  padding: 3px 7px;
  letter-spacing: .5px;
}
.mp-title {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--tx-warm2);
  flex: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mp-header select, .mp-header button {
  font-size: 11px;
  font-family: var(--font-mono);
  background: var(--cream);
  color: var(--tx-warm);
  border: 1px solid var(--sand);
  border-radius: 4px;
  padding: 3px 8px;
  cursor: pointer;
}
.mp-header button:hover { border-color: var(--em); color: var(--em); }
.mp-body { flex: 1; min-height: 0; }
`;
    document.head.appendChild(style);
    this._styleEl = style;
  }

  _buildOverlay() {
    if (this._overlay) return;

    const overlay = document.createElement('div');
    overlay.className = 'mp-overlay';
    overlay.id = 'mpOverlay';

    const header = document.createElement('div');
    header.className = 'mp-header';

    const badge = document.createElement('span');
    badge.className = 'mp-badge';
    badge.textContent = '原型';
    header.appendChild(badge);

    const title = document.createElement('span');
    title.className = 'mp-title';
    title.textContent = SAMPLE_TITLE;
    header.appendChild(title);

    const lang = document.createElement('select');
    lang.className = 'mp-lang';
    lang.title = '切换语言（原型演示）';
    for (const l of LANGS) {
      const opt = document.createElement('option');
      opt.value = l.id;
      opt.textContent = l.label;
      lang.appendChild(opt);
    }
    lang.value = SAMPLE_LANG;
    lang.addEventListener('change', () => this._setLanguage(lang.value));
    header.appendChild(lang);

    const resetBtn = document.createElement('button');
    resetBtn.textContent = '重置示例';
    resetBtn.title = '恢复内置示例内容';
    resetBtn.addEventListener('click', () => {
      this._setContent(SAMPLE_CONTENT, SAMPLE_LANG);
      if (this._langSelect) this._langSelect.value = SAMPLE_LANG;
    });
    header.appendChild(resetBtn);

    const closeBtn = document.createElement('button');
    closeBtn.textContent = '关闭 (⎋)';
    closeBtn.title = '关闭编辑器原型';
    closeBtn.addEventListener('click', () => this.close());
    header.appendChild(closeBtn);

    const body = document.createElement('div');
    body.className = 'mp-body';
    body.id = 'mpBody';

    overlay.appendChild(header);
    overlay.appendChild(body);

    const contentEl = document.querySelector('.content');
    if (contentEl) contentEl.appendChild(overlay);
    else console.warn('[monaco-preview] .content 未找到，编辑器浮层未挂载');

    this._overlay = overlay;
    this._titleEl = title;
    this._langSelect = lang;
    this._bodyEl = body;

    // Esc 关闭（仅在浮层打开时拦截，避免干扰终端）
    this._keyHandler = (e) => {
      if (e.key === 'Escape' && this._opened) this.close();
    };
    document.addEventListener('keydown', this._keyHandler);

    // 窗口尺寸变化 → 让 Monaco 重算布局
    this._resizeHandler = () => { try { this._editor && this._editor.layout(); } catch (_) {} };
    window.addEventListener('resize', this._resizeHandler);
  }

  _setLanguage(lang) {
    if (!this._editor) return;
    try {
      const m = window.monaco;
      if (m && m.editor && m.editor.setModelLanguage) {
        m.editor.setModelLanguage(this._editor.getModel(), lang);
      }
    } catch (e) {
      console.warn('[monaco-preview] setLanguage failed:', e);
    }
  }

  _setContent(content, lang) {
    if (!this._editor) return;
    try {
      this._editor.setValue(content);
      if (lang) this._setLanguage(lang);
    } catch (e) {
      console.warn('[monaco-preview] setContent failed:', e);
    }
  }
}

// 供测试断言 / 未来文件树集成复用
export { SAMPLE_TITLE, SAMPLE_LANG, SAMPLE_CONTENT, LANGS };
