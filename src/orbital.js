// 光球轨道 — 左侧面板，含「项目」/「文件」两个 Tab。
//
// 「项目」Tab 显示已保存的项目卡片列表 + 终端 orb。
// 「文件」Tab 显示文件浏览器（FileExplorerController）。

import * as ipc from './ipc-bridge.js';

const IDENTITY_COLORS = [
  { color: 'var(--id-emerald)', glow: 'var(--glow-emerald)', hex: '#10B981' },
  { color: 'var(--id-violet)',  glow: 'var(--glow-violet)',  hex: '#A78BFA' },
  { color: 'var(--id-cyan)',    glow: 'var(--glow-cyan)',    hex: '#22D3EE' },
  { color: 'var(--id-rose)',    glow: 'var(--glow-rose)',     hex: '#F472B6' },
  { color: 'var(--id-amber)',   glow: 'var(--glow-amber)',    hex: '#F7C948' },
  { color: 'var(--id-sky)',     glow: 'var(--glow-sky)',      hex: '#7DD3FC' },
  { color: 'var(--id-peach)',   glow: 'var(--glow-peach)',    hex: '#FB923C' },
  { color: 'var(--id-lime)',    glow: 'var(--glow-lime)',     hex: '#84CC16' },
  { color: 'var(--id-fuchsia)', glow: 'var(--glow-fuchsia)',  hex: '#D946EF' },
  { color: 'var(--id-teal)',    glow: 'var(--glow-teal)',     hex: '#2DD4BF' },
];

let colorIndex = 0;

function nextIdentity() {
  const id = IDENTITY_COLORS[colorIndex % IDENTITY_COLORS.length];
  colorIndex++;
  return id;
}

export class OrbitalController {
  constructor() {
    this.tm = null;
    this.el = null;
    this.agentProvider = null;
    this._ctxMenu = null;
    this._activeTab = 'projects';
    this._projects = [];
    this._projectListView = null;
    this._filesView = null;
    this._fileExplorer = null;
    this._projectsLoaded = false;
    this._backends = []; // cached list from list_backends
  }

  init(tm) {
    this.tm = tm;
    this.el = document.getElementById('orbital');

    this._renderTabBar();
    this._renderTabContent();

    // Context menu
    this.el.addEventListener('contextmenu', (e) => {
      const orb = e.target.closest('.orb');
      const projCard = e.target.closest('.project-card');
      if (orb) {
        e.preventDefault();
        this._showCtxMenu(e, orb.dataset.id);
      } else if (projCard) {
        e.preventDefault();
        const proj = this._projectByCard(projCard);
        if (proj) this._showProjectCtxMenu(e, proj);
      }
    });

    // Pre-load backends list
    this._loadBackends();
  }

  async _loadBackends() {
    try {
      this._backends = await ipc.listBackends();
      // Populate backend install hints cache in TabManager
      if (this.tm && this._backends.length) {
        const hints = new Map(this._backends.map(b => [b.id, b.install_hint]));
        this.tm.setBackendHints(hints);
      }
    } catch (e) {
      console.warn('[orbital] list_backends failed', e);
      this._backends = [];
    }
  }

  // ── Tab Bar ──

  _renderTabBar() {
    const bar = document.createElement('div');
    bar.className = 'orbital-tab-bar';

    const tabProject = document.createElement('button');
    tabProject.className = 'orbital-tab active';
    tabProject.dataset.tab = 'projects';
    tabProject.textContent = '项目';

    const tabFiles = document.createElement('button');
    tabFiles.className = 'orbital-tab';
    tabFiles.dataset.tab = 'files';
    tabFiles.textContent = '文件';

    bar.appendChild(tabProject);
    bar.appendChild(tabFiles);
    this.el.appendChild(bar);

    bar.addEventListener('click', (e) => {
      const btn = e.target.closest('.orbital-tab');
      if (!btn) return;
      this._switchTab(btn.dataset.tab);
    });
  }

  _switchTab(name) {
    if (this._activeTab === name) return;
    this._activeTab = name;

    this.el.querySelectorAll('.orbital-tab').forEach((btn) => {
      btn.classList.toggle('active', btn.dataset.tab === name);
    });

    this.el.querySelectorAll('.orbital-tab-content').forEach((panel) => {
      panel.classList.toggle('hidden', panel.dataset.tab !== name);
    });

    if (name === 'files') {
      if (this._fileExplorer) {
        this._fileExplorer.setVisible(true);
        this._fileExplorer.syncCwd(this.tm?.getActiveCwd());
      }
    } else {
      if (this._fileExplorer) this._fileExplorer.setVisible(false);
    }
  }

  /** Switch to Files tab (called by Cmd+Shift+F shortcut). */
  switchToFiles() {
    const sidebar = this.el;
    if (sidebar.classList.contains('collapsed')) {
      sidebar.classList.remove('collapsed');
    }
    this._switchTab('files');
  }

  /** Set the FileExplorerController instance. */
  setFileExplorer(fe) {
    this._fileExplorer = fe;
    if (this._filesView) fe.init(this._filesView, this.tm);
  }

  // ── Tab Content ──

  _renderTabContent() {
    // ── Projects Tab ──
    const projectPanel = document.createElement('div');
    projectPanel.className = 'orbital-tab-content';
    projectPanel.dataset.tab = 'projects';

    const projectBtn = document.createElement('button');
    projectBtn.className = 'orbital-action-btn';
    projectBtn.innerHTML =
      `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" style="width:14px;height:14px"><path d="M2 4h12v9H2z"/><path d="M2 4l3-2h6l3 2"/></svg>` +
      `New Project`;
    projectBtn.addEventListener('click', () => this._newProject());
    projectPanel.appendChild(projectBtn);

    const projectList = document.createElement('div');
    projectList.className = 'project-list';
    projectPanel.appendChild(projectList);

    this.el.appendChild(projectPanel);
    this._projectListView = projectPanel;

    // ── Files Tab ──
    const filesPanel = document.createElement('div');
    filesPanel.className = 'orbital-tab-content hidden';
    filesPanel.dataset.tab = 'files';
    this.el.appendChild(filesPanel);
    this._filesView = filesPanel;

    // Initialize file explorer if already set
    if (this._fileExplorer) {
      this._fileExplorer.init(filesPanel, this.tm);
    }

    // Load projects on init
    this._loadProjects();
  }

  // ── Project List ──

  async _loadProjects() {
    // ★ 去抖：300ms 内多次调用只执行最后一次，避免 onChange 频繁触发 DOM 全量重建
    if (this._loadProjectsTimer) clearTimeout(this._loadProjectsTimer);
    this._loadProjectsTimer = setTimeout(async () => {
      this._loadProjectsTimer = null;
      await this._doLoadProjects();
    }, 300);
  }

  async _doLoadProjects() {
    const container = this._projectListView.querySelector('.project-list');
    if (container) {
      container.innerHTML = '<div class="fe-loading">加载中…</div>';
    }

    try {
      const projects = await ipc.listProjects();
      this._projects = projects || [];
      this._projectsLoaded = true;
    } catch (e) {
      console.warn('[orbital] load projects failed', e);
      this._projects = [];
    }
    this._renderProjectList();
  }

  _renderProjectList() {
    const container = this._projectListView.querySelector('.project-list');
    if (!container) return;
    container.innerHTML = '';

    // ★ 前端去重：按 (id || name) 去重，避免同名不同 ID 的项目重复显示
    const seenIds = new Set();
    const dedupedProjects = [];
    for (const proj of this._projects) {
      const projId = proj.id || proj.name;
      if (seenIds.has(projId)) continue;
      seenIds.add(projId);
      dedupedProjects.push(proj);
    }

    if (dedupedProjects.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'project-empty';
      empty.textContent = '尚无项目';
      container.appendChild(empty);
      return;
    }

    dedupedProjects.forEach((proj, i) => {
      const projId = proj.id || proj.name;

      // Count active terminals for this project
      // ★ 修复: 只用 projectId 精确匹配，不用 cwd OR 条件
      // OR 条件导致共享目录的多个项目重复计入同一个终端
      let runningCount = 0;
      let exitedCount = 0;
      let lastRunningId = null;
      let lastAnyId = null;
      if (this.tm && this.tm.tabs) {
        this.tm.tabs.forEach((st) => {
          if (st.projectId === projId) {
            lastAnyId = st.id;
            if (st.status === 'running' && !st.isError) {
              runningCount++;
              lastRunningId = st.id;
            } else {
              exitedCount++;
            }
          }
        });
      }

      const card = document.createElement('div');
      card.className = 'project-card';
      card.dataset.projectDir = proj.dir || '';
      card.dataset.projectId = projId;

      // Identity color for icon
      const identity = IDENTITY_COLORS[i % IDENTITY_COLORS.length];
      const initial = (proj.name || '?')[0].toUpperCase();

      const projBackend = proj.backend || null;
      const projBackendDisplay = projBackend ? (this._backends.find(b => b.id === projBackend)?.display_name || projBackend) : null;

      card.innerHTML =
        `<div class="project-icon" style="background:${identity.hex}22;color:${identity.hex}">${esc(initial)}</div>` +
        `<div class="project-info">` +
        `<div class="project-name">${esc(proj.name)}${projBackendDisplay ? `<span class="project-backend-badge">${esc(projBackendDisplay)}</span>` : ''}</div>` +
        (proj.dir ? `<div class="project-dir">${esc(proj.dir.split('/').pop())}</div>` : '') +
        (proj.description ? `<div class="project-desc">${esc(proj.description)}</div>` : '') +
        `</div>` +
        (runningCount > 0 ? `<span class="project-count has">${runningCount} 活跃</span>` : '') +
        (exitedCount > 0 ? `<span class="project-count exited">${exitedCount} 异常</span>` : '') +
        `<button class="project-more" title="更多">⋯</button>`;

      // Click → switch to existing terminal, or create one for this project
      card.addEventListener('click', (e) => {
        if (e.target.closest('.project-more')) return;
        const projId2 = proj.id || proj.name;
        if (this.tm && this.tm.tabs) {
          // ★ 修复: 优先切换到 running tab，其次切换到 error/exited tab
          // 之前 !st.isError 导致 error tab 被跳过，每次点击都创建新终端
          let firstRunning = null;
          let firstAny = null;
          for (const [tid, st] of this.tm.tabs) {
            if (st.projectId === projId2) {
              if (!firstAny) firstAny = tid;
              if (!st.isError && st.status === 'running' && !firstRunning) {
                firstRunning = tid;
              }
            }
          }
          const targetId = firstRunning || firstAny;
          if (targetId) {
            this.tm.switchTo(targetId);
            return;
          }
        }
        // No terminal yet — create one, pass backend from project metadata
        this.tm.createTab({ label: proj.name, cwd: proj.dir, projectId: projId2, backend: projBackend });
      });

      // ⋯ button → project action menu (compute termIds at click time, not render time)
      card.querySelector('.project-more').addEventListener('click', (e) => {
        e.stopPropagation();
        const projId2 = proj.id || proj.name;
        const nowTermIds = [];
        if (this.tm && this.tm.tabs) {
          this.tm.tabs.forEach((st, tid) => {
            // ★ 修复: error/exited tab 也应计入，用户可从菜单操作
            if (st.projectId === projId2) {
              nowTermIds.push(tid);
            }
          });
        }
        this._showProjectCardMenu(e, proj, nowTermIds);
      });

      container.appendChild(card);
    });

    // Re-attach existing project terminal orbs under their project cards
    if (this.tm && this.tm.tabs) {
      this.tm.tabs.forEach((st) => {
        if (!st.projectId) return;
        const orb = this.el.querySelector(`.orb[data-id="${st.id}"]`);
        if (!orb) return;
        const projCard = container.querySelector(`.project-card[data-project-id="${st.projectId}"]`);
        if (!projCard) return;
        let termList = projCard.nextElementSibling;
        if (!termList || !termList.classList.contains('project-terminals')) {
          termList = document.createElement('div');
          termList.className = 'project-terminals';
          projCard.parentNode.insertBefore(termList, projCard.nextSibling);
        }
        termList.appendChild(orb);
      });
    }
  }

  // ── New Project Dialog ──

  async _newProject() {
    // If dialog already open, focus it
    const existing = document.getElementById('newProjectOverlay');
    if (existing) { existing.querySelector('#npName')?.focus(); return; }

    const home = await getHome();

    // Refresh backends list
    await this._loadBackends();

    // Build backend selector HTML
    const backends = this._backends || [];
    const installed = backends.filter(b => b.installed);
    const uninstalled = backends.filter(b => !b.installed);
    const sortedBackends = [...installed, ...uninstalled];

    const backendOptionsHtml = sortedBackends.map(b => {
      const icon = b.installed ? '✓' : '✗';
      const cls = b.installed ? '' : ' disabled';
      const selected = b.id === 'claude-code' ? ' selected' : '';
      return `<option value="${esc(b.id)}"${cls}${selected}>${icon} ${esc(b.display_name)}</option>`;
    }).join('');

    // Create overlay + dialog (same pattern as Palette)
    const overlay = document.createElement('div');
    overlay.id = 'newProjectOverlay';
    overlay.className = 'np-overlay';
    overlay.innerHTML =
      `<div class="np-dialog">` +
        `<div class="np-header">` +
          `<div class="np-title">新建项目</div>` +
          `<button class="np-close" title="关闭">✕</button>` +
        `</div>` +
        `<div class="np-body">` +
          `<label class="np-label">项目名称</label>` +
          `<input type="text" id="npName" class="np-input" placeholder="my-project" autocomplete="off" spellcheck="false">` +
          `<label class="np-label">后端</label>` +
          `<div class="np-backend-row">` +
            `<select id="npBackend" class="np-select">${backendOptionsHtml}</select>` +
          `</div>` +
          `<div id="npInstallHint" class="np-hint" style="display:none"></div>` +
          `<label class="np-label">项目目录</label>` +
          `<div class="np-dir-row">` +
            `<input type="text" id="npDir" class="np-input np-mono" placeholder="${home}/my-project" autocomplete="off" spellcheck="false">` +
            `<button id="npBrowse" class="np-browse-btn" title="选择目录">…</button>` +
          `</div>` +
          `<label class="np-label">项目描述 <span class="np-optional">选填</span></label>` +
          `<input type="text" id="npDesc" class="np-input" placeholder="简短描述这个项目…" autocomplete="off" spellcheck="false">` +
        `</div>` +
        `<div class="np-footer">` +
          `<button id="npCancel" class="np-btn np-btn-cancel">取消</button>` +
          `<button id="npConfirm" class="np-btn np-btn-confirm">创建项目</button>` +
        `</div>` +
      `</div>`;

    document.body.appendChild(overlay);
    requestAnimationFrame(() => overlay.classList.add('on'));

    const nameInput = overlay.querySelector('#npName');
    const dirInput = overlay.querySelector('#npDir');
    const descInput = overlay.querySelector('#npDesc');
    const backendSelect = overlay.querySelector('#npBackend');
    const confirmBtn = overlay.querySelector('#npConfirm');

    nameInput.focus();

    // Auto-fill dir when name changes
    nameInput.addEventListener('input', () => {
      const name = (nameInput.value || '').trim();
      if (name && !dirInput.dataset.userEdited) {
        dirInput.value = `${home}/${name}`;
      }
      updateConfirmState();
    });

    // Mark dir as user-edited
    dirInput.addEventListener('input', () => {
      dirInput.dataset.userEdited = 'true';
    });

    // Browse — use Tauri dialog
    overlay.querySelector('#npBrowse').addEventListener('click', async () => {
      try {
        const { open } = window.__TAURI__.dialog;
        const selected = await open({ directory: true, title: '选择项目目录' });
        if (selected) {
          dirInput.value = selected;
          dirInput.dataset.userEdited = 'true';
          if (!nameInput.value) {
            nameInput.value = selected.split('/').pop() || '';
          }
          updateConfirmState();
        }
      } catch (e) {
        console.warn('[browse] dialog not available', e);
      }
    });

    // Confirm button state — also check that selected backend is installed
    // Show install hint when uninstalled backend is selected
    const installHintEl = overlay.querySelector('#npInstallHint');
    const updateConfirmState = () => {
      const name = (nameInput.value || '').trim();
      const selBackend = backendSelect.value;
      const beInfo = backends.find(b => b.id === selBackend);
      const notInstalled = beInfo && !beInfo.installed;
      confirmBtn.disabled = !name || notInstalled;
      if (notInstalled && beInfo.install_hint) {
        installHintEl.innerHTML = `📦 安装方式: <span class="np-mono">${esc(beInfo.install_hint)}</span>`;
        installHintEl.style.display = '';
      } else {
        installHintEl.style.display = 'none';
      }
    };
    updateConfirmState();

    backendSelect.addEventListener('change', updateConfirmState);

    const close = () => {
      overlay.classList.remove('on');
      overlay.classList.add('dismissing');
      setTimeout(() => overlay.remove(), 200);
    };

    const commit = async () => {
      const name = (nameInput.value || '').trim();
      const dir = (dirInput.value || '').trim();
      const desc = (descInput.value || '').trim();
      const backend = backendSelect.value || null;
      if (!name) return;
      close();
      const projectDir = dir || `${home}/${name}`;
      let projectId = name;
      try {
        const id = await ipc.saveProject({ name, dir: projectDir, description: desc, backend });
        if (id) projectId = id;
      } catch (_) {}
      await this.tm.createTab({ label: name, cwd: projectDir, projectId, backend });
      this._loadProjects();
    };

    // Event listeners
    confirmBtn.addEventListener('click', commit);
    overlay.querySelector('#npCancel').addEventListener('click', close);
    overlay.querySelector('.np-close').addEventListener('click', close);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });

    const handleKey = (e) => {
      if (e.key === 'Enter' && !confirmBtn.disabled) { e.preventDefault(); commit(); }
      if (e.key === 'Escape') close();
      e.stopPropagation();
    };
    nameInput.addEventListener('keydown', handleKey);
    dirInput.addEventListener('keydown', handleKey);
    descInput.addEventListener('keydown', handleKey);
    backendSelect.addEventListener('keydown', handleKey);
  }

  // ── Orb Management ──

  addOrb(id, label, status, cwd) {
    // All terminals are project terminals (no chat concept)
    const st = this.tm && this.tm.tabs.get(id);
    if (!st) return;

    const identity = nextIdentity();
    const orb = document.createElement('div');
    orb.className = `orb ${status}`;
    orb.dataset.id = id;
    orb.dataset.identityColor = identity.color;
    orb.dataset.identityGlow = identity.glow;
    orb.dataset.identityHex = identity.hex;
    orb.style.setProperty('--orb-color', identity.color);
    orb.style.setProperty('--orb-glow', identity.glow);

    const cwdBasename = cwd ? cwd.split('/').pop() || cwd : '';

    orb.innerHTML =
      `<span class="orb-dot ${status}"></span>` +
      `<div class="orb-info">` +
      `<div class="orb-label">${esc(label)}</div>` +
      (cwdBasename ? `<div class="orb-cwd">${esc(cwdBasename)}</div>` : '') +
      `</div>`;

    // Click → switch
    orb.addEventListener('click', (e) => {
      const dot = e.target.closest('.orb-dot');
      if (dot && status === 'exited') {
        this.tm.restartTab(id);
      } else {
        this.tm.switchTo(id);
      }
    });

    // Double-click label → rename
    const labelEl = orb.querySelector('.orb-label');
    labelEl.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      this.startRename(id, labelEl);
    });

    // Insert orb after the matching project card in the project list
    const projCard = this.el.querySelector(`.project-card[data-project-id="${st.projectId}"]`);
    if (projCard) {
      // Find or create a terminal list container after this project card
      let termList = projCard.nextElementSibling;
      if (!termList || !termList.classList.contains('project-terminals')) {
        termList = document.createElement('div');
        termList.className = 'project-terminals';
        projCard.parentNode.insertBefore(termList, projCard.nextSibling);
      }
      termList.appendChild(orb);
    } else {
      // Project card not yet rendered — append to project list
      const projectList = this._projectListView?.querySelector('.project-list');
      if (projectList) projectList.appendChild(orb);
    }
    this._updateEmptyState();
    return identity;
  }

  removeOrb(id) {
    const orb = this.el.querySelector(`.orb[data-id="${id}"]`);
    if (orb) {
      const termList = orb.closest('.project-terminals');
      orb.style.transition = 'opacity 200ms ease-out, max-height 200ms ease-out';
      orb.style.opacity = '0';
      orb.style.maxHeight = '0';
      orb.style.overflow = 'hidden';
      orb.style.padding = '0 12px';
      setTimeout(() => {
        orb.remove();
        // Clean up empty terminal list container
        if (termList && termList.children.length === 0) termList.remove();
      }, 200);
    }
    this._updateEmptyState();
  }

  setActive(id) {
    this.el.querySelectorAll('.orb').forEach((o) => {
      o.classList.toggle('active', o.dataset.id === id);
    });
  }

  updateOrbStatus(id, status) {
    const orb = this.el.querySelector(`.orb[data-id="${id}"]`);
    if (!orb) return;
    orb.classList.remove('running', 'exited', 'crashed', 'thinking', 'active');
    orb.classList.add(status);
    if (this.tm.activeId === id) orb.classList.add('active');
    const dot = orb.querySelector('.orb-dot');
    if (dot) {
      dot.classList.remove('running', 'exited', 'crashed', 'thinking');
      dot.classList.add(status);
    }
  }

  updateOrbLabel(id, label) {
    const orb = this.el.querySelector(`.orb[data-id="${id}"]`);
    if (!orb) return;
    const el = orb.querySelector('.orb-label');
    if (el && el.tagName !== 'INPUT') el.textContent = label;
  }

  startRename(id, labelEl) {
    const cur = labelEl.textContent;
    const input = document.createElement('input');
    input.className = 'sb-rename-input';
    input.value = cur;
    input.style.cssText = 'font:inherit;font-size:12px;font-weight:600;background:var(--cream);color:var(--tx-warm);border:1px solid var(--sand);border-radius:4px;padding:1px 4px;outline:none;width:100%;';
    labelEl.replaceWith(input);
    input.focus();
    input.select();
    const commit = () => {
      const val = (input.value || '').trim() || cur;
      const span = document.createElement('span');
      span.className = 'orb-label';
      span.textContent = val;
      input.replaceWith(span);
      ipc.ptyRename(id, val);
      const st = this.tm && this.tm.tabs.get(id);
      if (st) st.label = val;
      span.addEventListener('dblclick', (e) => {
        e.stopPropagation();
        this.startRename(id, span);
      });
      this.tm._scheduleSave();
      this.tm._notifyChange();
    };
    input.addEventListener('blur', commit);
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') input.blur();
      if (e.key === 'Escape') { input.value = cur; input.blur(); }
      e.stopPropagation();
    });
  }

  // ── Context Menu ──

  _showCtxMenu(e, id) {
    this._dismissCtxMenu();
    const st = this.tm && this.tm.tabs.get(id);
    if (!st) return;

    const menu = document.createElement('div');
    menu.className = 'ctx-menu';
    menu.style.left = e.clientX + 'px';
    menu.style.top = e.clientY + 'px';

    const items = [
      { label: '重命名', action: () => { const orb = this.el.querySelector(`.orb[data-id="${id}"]`); const l = orb?.querySelector('.orb-label'); if (l) this.startRename(id, l); } },
      { label: '重启', action: () => this.tm.restartTab(id) },
    ];
    // "复制对话" — 所有终端都属项目，皆可复制
    items.push({ label: '复制对话', action: () => this.tm.createTab({ label: st.label + '-2', cmd: st.cmd, args: st.args, cwd: st.cwd, projectId: st.projectId, backend: st.backend }) });
    items.push(
      { sep: true },
      { label: '复制路径', action: () => { if (st.cwd) navigator.clipboard.writeText(st.cwd); } },
      { sep: true },
      { label: '关闭', cls: 'danger', action: () => this.tm.closeTab(id) },
    );

    items.forEach((it) => {
      if (it.sep) {
        // 用 createElement 追加，不用 innerHTML +=
        // innerHTML += 会序列化再反序列化整个 DOM，销毁之前绑定的 click 监听器
        const sep = document.createElement('div');
        sep.className = 'ctx-menu-sep';
        menu.appendChild(sep);
      } else {
        const el = document.createElement('div');
        el.className = 'ctx-menu-item' + (it.cls ? ' ' + it.cls : '');
        el.textContent = it.label;
        el.addEventListener('click', () => { it.action(); this._dismissCtxMenu(); });
        menu.appendChild(el);
      }
    });

    document.body.appendChild(menu);
    this._ctxMenu = menu;

    const rect = menu.getBoundingClientRect();
    if (rect.right > window.innerWidth) menu.style.left = (window.innerWidth - rect.width - 8) + 'px';
    if (rect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - rect.height - 8) + 'px';

    const dismiss = (ev) => {
      if (ev.key === 'Escape' || !menu.contains(ev.target)) {
        this._dismissCtxMenu();
        document.removeEventListener('mousedown', dismiss);
        document.removeEventListener('keydown', dismiss);
      }
    };
    setTimeout(() => {
      document.addEventListener('mousedown', dismiss);
      document.addEventListener('keydown', dismiss);
    }, 0);
  }

  _dismissCtxMenu() {
    if (this._ctxMenu) { this._ctxMenu.remove(); this._ctxMenu = null; }
  }

  _showProjectCardMenu(e, proj, termIds) {
    this._dismissCtxMenu();
    const menu = document.createElement('div');
    menu.className = 'ctx-menu';
    const btn = e.target.closest('.project-more');
    if (btn) {
      const rect = btn.getBoundingClientRect();
      menu.style.left = (rect.right - 140) + 'px';
      menu.style.top = rect.bottom + 4 + 'px';
    } else {
      menu.style.left = e.clientX + 'px';
      menu.style.top = e.clientY + 'px';
    }

    const projId = proj.id || proj.name;
    const projBackend = proj.backend || null;
    const items = [];

    // 打开新终端 — 始终可用
    items.push({ label: '打开新终端', action: () => {
      this.tm.createTab({ label: proj.name, cwd: proj.dir, projectId: projId, backend: projBackend });
    }});

    // 在 Finder 中显示 — 始终可用
    if (proj.dir) {
      items.push({ label: '在 Finder 中显示', action: () => {
        const { invoke } = window.__TAURI__.core;
        invoke('plugin:shell|open', { path: proj.dir }).catch(() => {});
      }});
    }

    // 关闭 — 仅在有终端时显示
    if (termIds.length > 0) {
      items.push({ sep: true });
      items.push({ label: `关闭（${termIds.length} 个终端）`, cls: 'danger', action: () => {
        [...termIds].forEach(tid => this.tm.closeTab(tid));
      }});
    }

    // 删除项目 — 始终可用
    items.push({ sep: true });
    items.push({ label: '删除项目', cls: 'danger', action: async () => {
      let activeCount = 0;
      if (this.tm && this.tm.tabs) {
        this.tm.tabs.forEach((st) => {
          if (st.projectId === projId && st.status === 'running') activeCount++;
        });
      }
      const msg = activeCount > 0
        ? `确定删除项目「${proj.name}」？${activeCount} 个活跃终端将继续运行。`
        : `确定删除项目「${proj.name}」？`;
      if (!await this._confirmDialog(msg)) return;
      try {
        await ipc.deleteProject(proj.name);
        if (this.tm && this.tm.tabs) {
          const delIds = [];
          this.tm.tabs.forEach((st, tid) => {
            if (st.projectId === projId) delIds.push(tid);
          });
          for (const tid of delIds) {
            await this.tm.closeTab(tid);
          }
        }
        this._loadProjects();
      } catch (err) {
        console.warn('[project] delete failed', err);
      }
    }});

    items.forEach((it) => {
      if (it.sep) {
        // 用 createElement 追加，不用 innerHTML +=
        // innerHTML += 会序列化再反序列化整个 DOM，销毁之前绑定的 click 监听器
        const sep = document.createElement('div');
        sep.className = 'ctx-menu-sep';
        menu.appendChild(sep);
      } else {
        const el = document.createElement('div');
        el.className = 'ctx-menu-item' + (it.cls ? ' ' + it.cls : '');
        el.textContent = it.label;
        el.addEventListener('click', () => { it.action(); this._dismissCtxMenu(); });
        menu.appendChild(el);
      }
    });

    document.body.appendChild(menu);
    this._ctxMenu = menu;

    const mRect = menu.getBoundingClientRect();
    if (mRect.right > window.innerWidth) menu.style.left = (window.innerWidth - mRect.width - 8) + 'px';
    if (mRect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - mRect.height - 8) + 'px';

    const dismiss = (ev) => {
      if (ev.key === 'Escape' || !menu.contains(ev.target)) {
        this._dismissCtxMenu();
        document.removeEventListener('mousedown', dismiss);
        document.removeEventListener('keydown', dismiss);
      }
    };
    setTimeout(() => {
      document.addEventListener('mousedown', dismiss);
      document.addEventListener('keydown', dismiss);
    }, 0);
  }

  _showProjectCtxMenu(e, proj) {
    this._dismissCtxMenu();
    const menu = document.createElement('div');
    menu.className = 'ctx-menu';
    menu.style.left = e.clientX + 'px';
    menu.style.top = e.clientY + 'px';

    const projBackend = proj.backend || null;
    const items = [
      { label: '打开新终端', action: () => this.tm.createTab({ label: proj.name, cwd: proj.dir, projectId: proj.id || proj.name, backend: projBackend }) },
      { label: '在 Finder 中显示', action: () => { if (proj.dir) { const { invoke } = window.__TAURI__.core; invoke('plugin:shell|open', { path: proj.dir }).catch(() => {}); } } },
      { sep: true },
      { label: '删除项目', cls: 'danger', action: async () => {
        // Count running terminals for this project
        let activeCount = 0;
        const projId = proj.id || proj.name;
        if (this.tm && this.tm.tabs) {
          this.tm.tabs.forEach((st) => {
            if (st.projectId === projId && st.status === 'running') activeCount++;
          });
        }
        const msg = activeCount > 0
          ? `确定删除项目「${proj.name}」？${activeCount} 个活跃终端将继续运行。`
          : `确定删除项目「${proj.name}」？`;
        if (!await this._confirmDialog(msg)) return;
        try {
          await ipc.deleteProject(proj.name);
          // Close all terminals belonging to this project
          if (this.tm && this.tm.tabs) {
            const termIds = [];
            this.tm.tabs.forEach((st, tid) => {
              if (st.projectId === projId) termIds.push(tid);
            });
            for (const tid of termIds) {
              await this.tm.closeTab(tid);
            }
          }
          this._loadProjects();
        } catch (err) {
          console.warn('[project] delete failed', err);
        }
      }},
    ];

    items.forEach((it) => {
      if (it.sep) {
        const sep = document.createElement('div');
        sep.className = 'ctx-menu-sep';
        menu.appendChild(sep);
      } else {
        const el = document.createElement('div');
        el.className = 'ctx-menu-item' + (it.cls ? ' ' + it.cls : '');
        el.textContent = it.label;
        el.addEventListener('click', () => { it.action(); this._dismissCtxMenu(); });
        menu.appendChild(el);
      }
    });

    document.body.appendChild(menu);
    this._ctxMenu = menu;

    const rect = menu.getBoundingClientRect();
    if (rect.right > window.innerWidth) menu.style.left = (window.innerWidth - rect.width - 8) + 'px';
    if (rect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - rect.height - 8) + 'px';

    const dismiss = (ev) => {
      if (ev.key === 'Escape' || !menu.contains(ev.target)) {
        this._dismissCtxMenu();
        document.removeEventListener('mousedown', dismiss);
        document.removeEventListener('keydown', dismiss);
      }
    };
    setTimeout(() => {
      document.addEventListener('mousedown', dismiss);
      document.addEventListener('keydown', dismiss);
    }, 0);
  }

  _updateEmptyState() {
    const empty = document.getElementById('termEmpty');
    if (!empty) return;
    const hasOrbs = this.el && this.el.querySelectorAll('.orb:not(.exiting)').length > 0;
    empty.classList.toggle('on', !hasOrbs);
  }

  /** 从 project-card DOM 元素反查项目数据。 */
  _projectByCard(cardEl) {
    const pid = cardEl.dataset.projectId;
    if (!pid || !this._projects) return null;
    return this._projects.find((p) => (p.id || p.name) === pid) || null;
  }

  /** 自定义确认弹窗（替代原生 confirm()）。 */
  _confirmDialog(msg) {
    return new Promise((resolve) => {
      const overlay = document.createElement('div');
      overlay.className = 'np-overlay';
      overlay.innerHTML =
        `<div class="np-dialog" style="max-width:360px">` +
          `<div class="np-header"><div class="np-title">确认操作</div>` +
          `<button class="np-close" title="取消">✕</button></div>` +
          `<div class="np-body"><p style="color:var(--tx-warm2);line-height:1.5;margin:0">${esc(msg)}</p></div>` +
          `<div class="np-footer">` +
            `<button class="np-btn np-btn-cancel" id="cfCancel">取消</button>` +
            `<button class="np-btn np-btn-confirm" id="cfOk">确认</button>` +
          `</div>` +
        `</div>`;
      document.body.appendChild(overlay);
      requestAnimationFrame(() => overlay.classList.add('on'));

      const close = (val) => {
        overlay.classList.remove('on');
        overlay.classList.add('dismissing');
        setTimeout(() => overlay.remove(), 200);
        resolve(val);
      };

      overlay.querySelector('#cfOk').addEventListener('click', () => close(true));
      overlay.querySelector('#cfCancel').addEventListener('click', () => close(false));
      overlay.querySelector('.np-close').addEventListener('click', () => close(false));
      overlay.addEventListener('click', (e) => { if (e.target === overlay) close(false); });
      overlay.querySelector('#cfCancel').addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === 'Escape') close(false);
      });
      overlay.querySelector('#cfOk').addEventListener('keydown', (e) => {
        if (e.key === 'Enter') close(true);
        if (e.key === 'Escape') close(false);
      });
      overlay.querySelector('#cfCancel').focus();
    });
  }
}

let _cachedHome = null;

async function getHome() {
  if (_cachedHome) return _cachedHome;
  try {
    _cachedHome = await ipc.getHomeDir();
  } catch (_) {
    // Don't cache the fallback — allow retry if backend becomes available later
    return '/';
  }
  return _cachedHome;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function truncate(s, maxLen) {
  if (!s) return '';
  if (s.length <= maxLen) return s;
  let end = maxLen;
  while (end > 0 && !s.isCharBoundary(end)) end--;
  return s.slice(0, end) + '…';
}
