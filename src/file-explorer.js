// 文件浏览器控制器 — 侧边栏「文件」Tab，原生文件系统浏览 + 预览。
//
// 通过 Rust fs_list_dir / fs_read_file 命令访问本地文件系统。
// 预览复用 marked.min.js（Markdown）和 highlight.min.js（代码高亮）。

import * as ipc from './ipc-bridge.js';

// ── 文件图标配置（移植自 onecode.html）──────────────────────────

const BADGE_MAP = {
  js: 'JS', mjs: 'JS', cjs: 'JS',
  ts: 'TS', tsx: 'TX', jsx: 'JX',
  py: 'PY', rb: 'RB', go: 'GO', rs: 'RS',
  java: 'JV', kt: 'KT',
  c: 'C', cpp: 'C+', h: 'H', hpp: 'H+', cs: 'C#',
  php: 'PH', sh: 'SH', bash: 'SH', zsh: 'SH',
  yaml: 'YM', yml: 'YM', toml: 'TM',
  json: '{ }', xml: 'XM',
  html: 'HT', htm: 'HT', css: 'CS', scss: 'SC',
  sql: 'DB', vue: 'VU', svelte: 'SV', lua: 'LU',
  swift: 'SW', dart: 'DT', scala: 'SC',
  dockerfile: 'DK', makefile: 'MK', md: 'MD',
};

const COLOR_MAP = {
  js: '#fb923c', mjs: '#fb923c', cjs: '#fb923c',
  ts: '#60a5fa', tsx: '#60a5fa', jsx: '#fb923c',
  py: '#7dd3fc', rb: '#f87171', go: '#22d3ee', rs: '#fb923c',
  java: '#f87171', kt: '#c084fc',
  c: '#8b9fc0', cpp: '#8b9fc0', h: '#8b9fc0', hpp: '#8b9fc0',
  cs: '#34d399', php: '#c084fc', sh: '#34d399', bash: '#34d399', zsh: '#34d399',
  yaml: '#f87171', yml: '#f87171', toml: '#8b9fc0',
  json: '#34d399', xml: '#fb923c',
  html: '#f87171', htm: '#f87171', css: '#60a5fa', scss: '#c084fc',
  sql: '#fbbf24', vue: '#34d399', svelte: '#fb923c', lua: '#7dd3fc',
  swift: '#fb923c', dart: '#7dd3fc', scala: '#f87171',
  dockerfile: '#22d3ee', makefile: '#8b9fc0', md: '#c084fc',
  png: '#f87171', jpg: '#f87171', jpeg: '#f87171',
  gif: '#f87171', svg: '#fb923c', webp: '#f87171', ico: '#fb923c',
  lock: '#546380',
};

const EXT_LANG = {
  js: 'javascript', mjs: 'javascript', cjs: 'javascript',
  ts: 'typescript', tsx: 'typescript',
  py: 'python', rb: 'ruby', go: 'go', rs: 'rust',
  java: 'java', kt: 'kotlin',
  c: 'c', cpp: 'cpp', h: 'c', hpp: 'cpp',
  cs: 'csharp', php: 'php',
  sh: 'bash', bash: 'bash', zsh: 'bash',
  yaml: 'yaml', yml: 'yaml', toml: 'ini',
  json: 'json', xml: 'xml',
  html: 'html', htm: 'html', css: 'css', scss: 'scss',
  sql: 'sql', vue: 'html', svelte: 'html', lua: 'lua',
  swift: 'swift', dart: 'dart', scala: 'scala',
  dockerfile: 'dockerfile', makefile: 'makefile',
  md: 'markdown', sql: 'sql',
};

// ── 工具函数 ────────────────────────────────────────────────────

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function fmtSize(b) {
  if (!b) return '';
  if (b < 1024) return b + 'B';
  if (b < 1048576) return (b / 1024 | 0) + 'K';
  if (b < 1073741824) return (b / 1048576).toFixed(1) + 'M';
  return (b / 1073741824).toFixed(1) + 'G';
}

/** 根据 file_type 和文件名生成图标 HTML */
function renderIcon(entry) {
  if (entry.is_dir) {
    return '<span class="fe-dir-icon"><svg viewBox="0 0 16 16" fill="none">' +
      '<path d="M1.5 3.5h4l1.5 1.5h8.5v8.5H1.5z" fill="var(--em)" opacity=".12" stroke="var(--em)" stroke-width=".8" stroke-linejoin="round"/>' +
      '<path d="M1.5 6h13v7.5H1.5z" fill="var(--em)" opacity=".06"/></svg></span>';
  }
  const ext = entry.name.split('.').pop().toLowerCase();
  const base = entry.name.toLowerCase();
  // 特殊文件名匹配
  const nameExts = {
    '.gitignore': 'sh', '.env': 'sh', '.npmrc': 'sh',
    '.babelrc': 'sh', '.eslintrc': 'sh', '.prettierrc': 'sh', '.editorconfig': 'sh',
    'Makefile': 'mk', 'Dockerfile': 'dk', 'Cargo.toml': 'tm', 'Cargo.lock': 'lock',
  };
  const resolvedExt = nameExts[base] || ext;
  const label = BADGE_MAP[resolvedExt] || resolvedExt.substring(0, 2).toUpperCase();
  const color = COLOR_MAP[resolvedExt] || '#71717a';
  return `<span class="fe-badge" style="background:${color}18;color:${color};border:1px solid ${color}30">${esc(label)}</span>`;
}

/** HTML 安全过滤 — expanded to cover single-quoted, unquoted event handlers,
 *  SVG/math tags, and javascript: URIs (mitigation, not full DOMPurify replacement). */
function sanitizeHtml(html) {
  return html
    // Remove dangerous tags
    .replace(/<script[\s\S]*?<\/script>/gi, '')
    .replace(/<svg[\s\S]*?<\/svg>/gi, '')
    .replace(/<math[\s\S]*?<\/math>/gi, '')
    .replace(/<iframe[\s\S]*?<\/iframe>/gi, '')
    .replace(/<object[\s\S]*?<\/object>/gi, '')
    .replace(/<embed[^>]*>/gi, '')
    .replace(/<form[\s\S]*?<\/form>/gi, '')
    // Remove all on* event handlers (double-quoted, single-quoted, or unquoted)
    .replace(/\s+on\w+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]*)/gi, '')
    // Remove javascript: / vbscript: / data:text/html URIs in href/src/action
    .replace(/(href|src|action|formaction)\s*=\s*(?:"(?:javascript|vbscript|data:text\/html)[^"]*"|'(?:javascript|vbscript|data:text\/html)[^']*')/gi, '');
}

/** hljs 高亮代码 */
function hlCode(text, lang) {
  if (typeof window.hljs !== 'undefined') {
    try {
      if (lang && window.hljs.getLanguage(lang)) {
        return window.hljs.highlight(text, { language: lang }).value;
      }
      return window.hljs.highlightAuto(text).value;
    } catch (_) {}
  }
  return esc(text);
}

// ── 控制器 ──────────────────────────────────────────────────────

export class FileExplorerController {
  constructor() {
    this._currentPath = null;
    this._entries = [];
    this._isLoading = false;
    this._tm = null;
    this._container = null;
    this._toolbarEl = null;
    this._treeEl = null;
    this._previewEl = null;
    this._previewBody = null;
    this._home = null;
    this._refreshTimer = null;
  }

  init(container, tm) {
    this._container = container;
    this._tm = tm;

    // 构建DOM
    this._buildUI();

    // 获取 home 目录
    ipc.getHomeDir().then((h) => { this._home = h; }).catch(() => {});
  }

  _buildUI() {
    const c = this._container;

    // ── 工具栏（面包屑 + 刷新） ──
    const toolbar = document.createElement('div');
    toolbar.className = 'fe-toolbar';

    const bcr = document.createElement('div');
    bcr.className = 'fe-breadcrumb';
    bcr.innerHTML = '<span class="fe-bcr-seg fe-bcr-home">~</span>';
    toolbar.appendChild(bcr);

    const refreshBtn = document.createElement('button');
    refreshBtn.className = 'fe-refresh-btn';
    refreshBtn.title = '刷新';
    refreshBtn.innerHTML = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" style="width:14px;height:14px"><path d="M2 8a6 6 0 0 1 11-3M14 8a6 6 0 0 1-11 3"/><path d="M13 2v3h-3M3 14v-3h3"/></svg>';
    refreshBtn.addEventListener('click', () => this.refresh());
    toolbar.appendChild(refreshBtn);

    c.appendChild(toolbar);
    this._toolbarEl = toolbar;

    // ── 文件列表 ──
    const tree = document.createElement('div');
    tree.className = 'fe-tree';
    tree.innerHTML = '<div class="fe-empty">选择一个目录开始浏览</div>';
    c.appendChild(tree);
    this._treeEl = tree;

    // ── 预览面板（覆盖在文件列表上） ──
    const preview = document.createElement('div');
    preview.className = 'fe-preview';

    const previewHeader = document.createElement('div');
    previewHeader.className = 'fe-preview-header';
    previewHeader.innerHTML = '<span class="fe-preview-title"></span><button class="fe-preview-close" title="关闭预览">✕</button>';
    previewHeader.querySelector('.fe-preview-close').addEventListener('click', () => this.closePreview());
    preview.appendChild(previewHeader);

    const previewBody = document.createElement('div');
    previewBody.className = 'fe-preview-body';
    preview.appendChild(previewBody);

    c.appendChild(preview);
    this._previewEl = preview;
    this._previewBody = previewBody;

    // Escape 关闭预览
    this._keyHandler = (e) => {
      if (e.key === 'Escape' && this._previewEl.classList.contains('on')) {
        e.preventDefault();
        this.closePreview();
      }
    };
    document.addEventListener('keydown', this._keyHandler);
  }

  // ── 导航 ──────────────────────────────────────────────────

  /** 导航到指定路径 */
  async navigate(path) {
    if (this._isLoading) return;
    this._isLoading = true;
    this.closePreview();

    this._currentPath = path;

    // 先渲染面包屑（即时反馈）
    this._renderBreadcrumb();

    this._treeEl.innerHTML = '<div class="fe-loading">加载中…</div>';

    try {
      const result = await ipc.fsListDir(path);
      this._currentPath = result.path; // 使用 Rust 返回的 canonical path
      this._entries = result.entries;
      this._renderBreadcrumb(); // 用 canonical path 重新渲染
      this._renderTree();
    } catch (e) {
      console.warn('[file-explorer] navigate failed:', e);
      this._treeEl.innerHTML = `<div class="fe-error">无法读取目录: ${esc(e)}</div>`;
    } finally {
      this._isLoading = false;
    }
  }

  /** 刷新当前目录 */
  refresh() {
    if (this._currentPath) {
      this.navigate(this._currentPath);
    }
  }

  /** 同步终端 cwd（终端切换时调用） */
  syncCwd(cwd) {
    if (!cwd) return;
    // 如果当前没有路径或 cwd 变化了，自动导航
    if (!this._currentPath || this._currentPath !== cwd) {
      this.navigate(cwd);
    }
  }

  // ── 面包屑 ──────────────────────────────────────────────

  _renderBreadcrumb() {
    const bcr = this._toolbarEl.querySelector('.fe-breadcrumb');
    if (!bcr || !this._currentPath) return;

    const homePath = this._home || '/';
    // 判断当前路径是否在 home 下
    const underHome = this._home && this._currentPath.startsWith(this._home);

    // ~ home 按钮
    let html = `<span class="fe-bcr-seg fe-bcr-home" data-path="${esc(homePath)}">~</span>`;

    if (underHome) {
      // 路径在 home 下：取 home 之后的路径段
      const relPath = this._currentPath.slice(this._home.length); // 如 "/yiyan-yixing/onecode-desktop"
      const segments = relPath.split('/').filter(Boolean); // ["yiyan-yixing", "onecode-desktop"]
      let accPath = this._home; // 从 home 开始累积
      segments.forEach((seg, i) => {
        accPath += '/' + seg;
        const isLast = i === segments.length - 1;
        html += `<span class="fe-bcr-sep">/</span>`;
        html += `<span class="fe-bcr-seg${isLast ? ' fe-bcr-cur' : ''}" data-path="${esc(accPath)}">${esc(seg)}</span>`;
      });
    } else {
      // 路径不在 home 下（如 /tmp）：按原始路径段显示
      const segments = this._currentPath.split('/').filter(Boolean);
      let accPath = '';
      segments.forEach((seg, i) => {
        accPath += '/' + seg;
        const isLast = i === segments.length - 1;
        html += `<span class="fe-bcr-sep">/</span>`;
        html += `<span class="fe-bcr-seg${isLast ? ' fe-bcr-cur' : ''}" data-path="${esc(accPath)}">${esc(seg)}</span>`;
      });
    }

    bcr.innerHTML = html;

    // 点击面包屑段导航
    bcr.querySelectorAll('.fe-bcr-seg:not(.fe-bcr-cur)').forEach((el) => {
      el.addEventListener('click', () => {
        this.navigate(el.dataset.path);
      });
    });
  }

  // ── 文件列表 ──────────────────────────────────────────────

  _renderTree() {
    const tree = this._treeEl;
    tree.innerHTML = '';

    // 如果不在根目录，添加 ".." 返回上级
    // 但不越过 home 目录（Rust validate_path 限制在 home 内）
    if (this._currentPath !== '/' && this._currentPath !== this._home) {
      let parentPath = this._currentPath.replace(/\/[^/]+$/, '') || '/';
      // 如果上级目录超出 home，则限制到 home
      if (this._home && !parentPath.startsWith(this._home)) {
        parentPath = this._home;
      }
      const upItem = document.createElement('div');
      upItem.className = 'fe-item fe-item-up';
      upItem.innerHTML = '<span class="fe-icon"></span><span class="fe-name">..</span>';
      upItem.addEventListener('click', () => this.navigate(parentPath));
      tree.appendChild(upItem);
    }

    if (this._entries.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'fe-empty';
      empty.textContent = '空目录';
      tree.appendChild(empty);
      return;
    }

    this._entries.forEach((entry) => {
      const item = document.createElement('div');
      item.className = 'fe-item';
      item.dataset.path = entry.path;
      item.dataset.name = entry.name;
      item.dataset.isDir = entry.is_dir ? '1' : '0';
      item.dataset.fileType = entry.file_type;

      item.innerHTML =
        `<span class="fe-icon">${renderIcon(entry)}</span>` +
        `<span class="fe-name">${esc(entry.name)}</span>` +
        (!entry.is_dir ? `<span class="fe-size">${fmtSize(entry.size)}</span>` : '');

      item.addEventListener('click', () => {
        if (entry.is_dir) {
          this.navigate(entry.path);
        } else {
          this.openFile(entry.name, entry.path, entry.file_type);
        }
      });

      tree.appendChild(item);
    });
  }

  // ── 文件预览 ──────────────────────────────────────────────

  async openFile(name, path, fileType) {
    const preview = this._previewEl;
    const body = this._previewBody;
    const title = preview.querySelector('.fe-preview-title');

    title.textContent = name;
    body.innerHTML = '<div class="fe-loading">加载中…</div>';
    preview.classList.add('on');

    try {
      const content = await ipc.fsReadFile(path);
      this._renderPreview(content);
    } catch (e) {
      console.warn('[file-explorer] read file failed:', e);
      body.innerHTML = `<div class="fe-error">无法读取文件: ${esc(e)}</div>`;
    }
  }

  _renderPreview(content) {
    const body = this._previewBody;

    switch (content.file_type) {
      case 'md': {
        if (content.text) {
          if (typeof window.marked !== 'undefined') {
            try {
              const html = sanitizeHtml(window.marked.parse(content.text));
              body.innerHTML = `<div class="fe-md">${html}</div>`;
              // 高亮 markdown 内的代码块
              body.querySelectorAll('pre code').forEach((block) => {
                if (typeof window.hljs !== 'undefined') {
                  window.hljs.highlightElement(block);
                }
              });
              break;
            } catch (_) {}
          }
          // fallback: 纯文本
          body.innerHTML = `<pre class="fe-code-pre">${esc(content.text)}</pre>`;
        } else {
          body.innerHTML = '<div class="fe-bin">文件为空</div>';
        }
        break;
      }

      case 'code': {
        if (content.text) {
          const ext = content.name.split('.').pop().toLowerCase();
          const lang = EXT_LANG[ext] || '';
          const highlighted = hlCode(content.text, lang);
          body.innerHTML =
            `<div class="fe-code-view">` +
            `<button class="fe-copy-btn" title="复制">Copy</button>` +
            `<pre class="fe-code-pre"><code class="hljs">${highlighted}</code></pre>` +
            `</div>`;
          const copyBtn = body.querySelector('.fe-copy-btn');
          if (copyBtn) {
            copyBtn.addEventListener('click', () => {
              navigator.clipboard.writeText(content.text).then(() => {
                copyBtn.textContent = '已复制!';
                setTimeout(() => { copyBtn.textContent = 'Copy'; }, 1200);
              });
            });
          }
        } else {
          body.innerHTML = '<div class="fe-bin">文件为空</div>';
        }
        break;
      }

      case 'img': {
        if (content.data_base64) {
          const ext = content.name.split('.').pop().toLowerCase();
          const mimeMap = {
            png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
            gif: 'image/gif', svg: 'image/svg+xml', webp: 'image/webp',
            ico: 'image/x-icon', bmp: 'image/bmp', avif: 'image/avif',
          };
          const mime = mimeMap[ext] || 'image/png';
          body.innerHTML = `<div class="fe-img-view"><img src="data:${mime};base64,${content.data_base64}" alt="${esc(content.name)}"></div>`;
        } else {
          body.innerHTML = `<div class="fe-bin"><p>${esc(content.name)}</p><p>图片过大 (${fmtSize(content.size)})，无法预览</p></div>`;
        }
        break;
      }

      case 'pdf': {
        if (content.data_base64) {
          // sandbox="allow-same-origin" prevents embedded JS execution
          body.innerHTML = `<iframe class="fe-pdf-view" sandbox="allow-same-origin" src="data:application/pdf;base64,${content.data_base64}"></iframe>`;
        } else {
          body.innerHTML = `<div class="fe-bin"><p>${esc(content.name)}</p><p>PDF 过大 (${fmtSize(content.size)})，无法预览</p></div>`;
        }
        break;
      }

      default: {
        // bin 类型
        body.innerHTML =
          `<div class="fe-bin">` +
          `<div class="fe-bin-icon"><svg width="24" height="24" viewBox="0 0 20 20" fill="none" stroke="var(--tx-warm4)" stroke-width="1.2"><rect x="3" y="3" width="14" height="14" rx="2"/><path d="M7 8v4M10 7v6M13 9v2"/></svg></div>` +
          `<p>${esc(content.name)}</p>` +
          `<p>${fmtSize(content.size)}</p>` +
          `<p>二进制文件</p>` +
          `</div>`;
        break;
      }
    }
  }

  /** 关闭预览面板 */
  closePreview() {
    if (this._previewEl) {
      this._previewEl.classList.remove('on');
      this._previewBody.innerHTML = '';
    }
  }

  /** Tab 可见时开始自动刷新，Tab 隐藏时停止 */
  setVisible(visible) {
    if (visible) {
      // 首次可见时同步 cwd
      if (!this._currentPath && this._tm) {
        const cwd = this._tm.getActiveCwd();
        if (cwd) this.navigate(cwd);
      }
      // 启动自动刷新（每 15 秒）
      // P1-20: Skip IPC refresh when file panel is collapsed
      this._stopRefresh();
      this._refreshTimer = setInterval(() => {
        const panel = document.getElementById('filePanel');
        if (panel && panel.classList.contains('collapsed')) return;
        if (this._currentPath) this.refresh();
      }, 15000);
    } else {
      this._stopRefresh();
    }
  }

  _stopRefresh() {
    if (this._refreshTimer) {
      clearInterval(this._refreshTimer);
      this._refreshTimer = null;
    }
  }
}
