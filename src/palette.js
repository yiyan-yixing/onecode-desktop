// 指令调色板 — Cmd+K 打开的浮动玻璃面板，替代侧栏/设置/搜索。
//
// 前缀语法：
//   无前缀 → 模糊搜索终端 + agents
//   @      → Agent 列表，**选中 = 派发一个新员工 Slot**（与侧栏/光球一致）
//   >      → 设置模式，内联编辑
//   new/+  → 新建终端

import * as ipc from './ipc-bridge.js';
import { dispatchAgent, sendTaskToSlot } from './agent-dispatch.js';

const ICONS = {
  skills: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M8 1l2 4 4.5.7-3.3 3.1.8 4.5L8 11l-4 2.3.8-4.5L1.5 5.7 6 5z"/></svg>',
  hooks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M6 2v4l-3 3v2h4v3l1 2 1-2v-3h4V9l-3-3V2"/><rect x="5" y="1" width="6" height="2" rx="1"/></svg>',
  plugins: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>',
  tasks: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><circle cx="8" cy="8" r="6"/><polyline points="8 4 8 8 11 9.5"/></svg>',
};

export class PaletteController {
  constructor() {
    this.tm = null;
    this.overlay = null;
    this.input = null;
    this.results = null;
    this.agentProvider = null;
    this._modelSwitch = null; // M1: 切换模型回调
    this._openEditor = null; // M3: 打开 Monaco 编辑器原型回调
    this._openWork = null; // 打开右面板「工作」页（产出 + 任务）
    this._selIdx = 0;
    this._items = [];
    this._open = false;
  }

  init(tm) {
    this.tm = tm;
    this.overlay = document.getElementById('paletteOverlay');
    this.input = document.getElementById('paletteInput');
    this.results = document.getElementById('paletteResults');
    this._openSettings = null; // callback for opening settings panel

    // Input
    this.input.addEventListener('input', () => this._render());
    this.input.addEventListener('keydown', (e) => this._onKey(e));

    // Click outside to dismiss
    this.overlay.addEventListener('click', (e) => {
      if (e.target === this.overlay) this.close();
    });
  }

  setAgentProvider(fn) {
    this.agentProvider = fn;
  }

  /** P1-4: set callback to open settings panel */
  setOpenSettings(fn) {
    this._openSettings = fn;
  }

  /** M1: set callback to open model switcher (/model) */
  setModelSwitch(fn) {
    this._modelSwitch = fn;
  }

  /** M3: set callback to open Monaco 编辑器原型 */
  setOpenWork(fn) {
    this._openWork = fn;
  }

  setOpenEditor(fn) {
    this._openEditor = fn;
  }

  open() {
    if (this._open) return;
    this._open = true;
    this.overlay.classList.remove('dismissing');
    this.overlay.classList.add('on');
    this.input.value = '';
    this._selIdx = 0;
    this._render();
    requestAnimationFrame(() => this.input.focus());
  }

  close() {
    if (!this._open) return;
    this._open = false;
    this.overlay.classList.add('dismissing');
    this.overlay.classList.remove('on');
    setTimeout(() => {
      this.overlay.classList.remove('dismissing');
      // Return focus to terminal
      if (this.tm && this.tm.activeId) {
        const st = this.tm.tabs.get(this.tm.activeId);
        if (st) st.term.focus();
      }
    }, 200);
  }

  toggle() {
    this._open ? this.close() : this.open();
  }

  _onKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      this.close();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      this._selIdx = Math.min(this._selIdx + 1, this._items.length - 1);
      this._highlightSel();
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      this._selIdx = Math.max(this._selIdx - 1, 0);
      this._highlightSel();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (this._items[this._selIdx]) this._items[this._selIdx].action();
      this.close();
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      // Autocomplete: if only one result, select it
      if (this._items.length === 1 && this._items[0]) {
        this._items[0].action();
        this.close();
      }
      return;
    }
  }

  _highlightSel() {
    this.results.querySelectorAll('.palette-item').forEach((el, i) => {
      el.classList.toggle('sel', i === this._selIdx);
    });
    // Scroll into view
    const sel = this.results.querySelector('.palette-item.sel');
    if (sel) sel.scrollIntoView({ block: 'nearest' });
  }

  _render() {
    const q = (this.input.value || '').toLowerCase().trim();
    this._items = [];
    this._selIdx = 0;

    const isSettingMode = q.startsWith('>');
    const isAgentMode = q.startsWith('@');
    const isNewMode = q.startsWith('new') || q.startsWith('+');
    const isModelMode = q.startsWith('/model');
    const isRosterMode = q.startsWith('/roster');

    if (isRosterMode) {
      this._renderRoster(q);
    } else if (isModelMode) {
      this._renderModel(q);
    } else if (isSettingMode) {
      this._renderSettings(q.slice(1).trim());
    } else if (isAgentMode) {
      this._renderAgents(q.slice(1).trim());
    } else {
      this._renderDefault(q, isNewMode);
    }

    this._highlightSel();
  }

  _renderDefault(q, isNewMode) {
    let html = '';

    // Terminals
    const terminals = [];
    if (this.tm) {
      for (const [id, st] of this.tm.tabs) {
        if (!q || st.label.toLowerCase().includes(q) || (st.cwd && st.cwd.toLowerCase().includes(q))) {
          terminals.push({ id, label: st.label, cwd: st.cwd, status: st.status, index: this.tm.order.indexOf(id) });
        }
      }
    }

    if (terminals.length) {
      html += '<div class="palette-group-label">终端</div>';
      terminals.forEach((t, i) => {
        const dotClass = t.status === 'running' ? 'running' : t.status === 'crashed' ? 'crashed' : 'exited';
        const kbd = t.index < 9 ? `<span class="palette-item-kbd">⌘${t.index + 1}</span>` : '';
        const cwdBasename = t.cwd ? t.cwd.split('/').pop() : '';
        // Guard against empty label — use '?' fallback if label is empty
        const initial = esc(((t.label || '?')[0] || '?').toUpperCase());
        html +=
          `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:${this._orbColorForId(t.id)}">${initial}</span>` +
          `<span class="palette-item-label">${esc(t.label)}</span>` +
          `<span class="palette-item-detail">${esc(cwdBasename)}</span>` +
          `${kbd}</div>`;
        this._items.push({ action: () => this.tm.switchTo(t.id) });
      });
    }

    // Agents
    const agents = this.agentProvider ? this.agentProvider() : [];
    const filteredAgents = q ? agents.filter((a) =>
      (a.name || '').toLowerCase().includes(q) || (a.id || '').toLowerCase().includes(q)
    ) : agents;

    if (filteredAgents.length) {
      html += '<div class="palette-group-label">Agents</div>';
      filteredAgents.forEach((a, i) => {
        const color = (a.color && a.color.startsWith('#')) ? a.color : '#A78BFA';
        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:${color}">${esc((a.icon || a.id || '?')[0].toUpperCase())}</span>` +
          `<span class="palette-item-label">@${esc(a.id)}</span>` +
          `<span class="palette-item-detail">${esc(a.name || '')}</span></div>`;
        this._items.push({ action: () => {
          // 与侧栏 / 光球一致：选中 agent = **派发**一个新员工 Slot
          dispatchAgent({
            tabManager: this.tm,
            agentId: a.id,
            agents: filteredAgents,
            onNotice: () => {},
          });
        }});
      });
    }

    // Actions
    html += '<div class="palette-group-label">操作</div>';

    // New terminal
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--id-emerald)">+</span>` +
      `<span class="palette-item-label">新建终端</span>` +
      `<span class="palette-item-kbd">⌘T</span></div>`;
    this._items.push({ action: () => this.tm.createTab() });

    // 产出审查（目标 ④）：把当前项目的产出记录摊开给人验收。
    // 真文件/证据由本机核对，验收状态只有人能改 —— 这是"管理产出"的人机门。
    // 只在有项目上下文时提供：产出记录写在 <cwd>/.onecode/deliverables/ 里，
    // 没有项目就没有"当前项目"可言（也省掉一个"请先打开项目"的提示机制）。
    {
      const st = this.tm && this.tm.activeId ? this.tm.tabs.get(this.tm.activeId) : null;
      const activeCwd = st && !st.isError ? st.cwd : null;
      if (activeCwd) {
        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-emerald)">✓</span>` +
          `<span class="palette-item-label">产出审查</span>` +
          `<span class="palette-item-detail">当前项目 · 人对证据拍板</span></div>`;
        this._items.push({ action: () => this._reviewDeliverables(activeCwd) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-emerald)">▶</span>` +
          `<span class="palette-item-label">验证证据</span>` +
          `<span class="palette-item-detail">跑一遍 agent 声明的检查（需确认）</span></div>`;
        this._items.push({ action: () => this._verifyDeliverable(activeCwd) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-amber,#d9a441)">☰</span>` +
          `<span class="palette-item-label">任务板</span>` +
          `<span class="palette-item-detail">当前项目 · 状态与产出关联</span></div>`;
        this._items.push({ action: () => this._showTaskBoard(activeCwd) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-amber,#d9a441)">✓</span>` +
          `<span class="palette-item-label">任务裁决</span>` +
          `<span class="palette-item-detail">完成 / 退回 · 只有人能裁决</span></div>`;
        this._items.push({ action: () => this._reviewTask(activeCwd) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-amber,#d9a441)">→</span>` +
          `<span class="palette-item-label">派活到当前终端</span>` +
          `<span class="palette-item-detail">把最早的未完成任务填进去（不代发）</span></div>`;
        this._items.push({ action: () => this._dispatchTask(activeCwd) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-amber,#d9a441)">Σ</span>` +
          `<span class="palette-item-label">账本</span>` +
          `<span class="palette-item-detail">任务/产出/验收 + token 用量（不含金额）</span></div>`;
        this._items.push({ action: () => this._showLedger(activeCwd) });

        // 结构化槽位（目标 ①）：不走终端的引擎槽位，每轮带结算
        {
          const st = this.tm && this.tm.activeId ? this.tm.tabs.get(this.tm.activeId) : null;
          html +=
            `<div class="palette-item" data-idx="${this._items.length}">` +
            `<span class="palette-item-icon" style="background:var(--id-emerald)">◫</span>` +
            `<span class="palette-item-label">结构化槽位</span>` +
            `<span class="palette-item-detail">不走终端 · 每轮带结算</span></div>`;
          this._items.push({ action: () => {
            this.close?.();
            this.tm.createStructuredTab?.({
              projectId: st ? st.projectId : null,
              cwd: activeCwd,
              agent: st ? st.agent : null,
              providerId: st ? st.providerId : null,
              model: st ? st.model : null,
            });
          } });
        }

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-amber,#d9a441)">Σ</span>` +
          `<span class="palette-item-label">账本（含全库合计）</span>` +
          `<span class="palette-item-detail">较慢：要读完全部历史转录</span></div>`;
        this._items.push({ action: () => this._showLedger(activeCwd, true) });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-emerald)">◈</span>` +
          `<span class="palette-item-label">员工模型</span>` +
          `<span class="palette-item-detail">按员工自选供应商/模型</span></div>`;
        this._items.push({ action: () => {
          this.input.value = '/roster';
          this._render();
        } });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--id-emerald)">▤</span>` +
          `<span class="palette-item-label">工作台</span>` +
          `<span class="palette-item-detail">产出 + 任务面板（右栏「工作」页）</span></div>`;
        this._items.push({ action: () => this._openWork?.() });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--stone)">⚙</span>` +
          `<span class="palette-item-label">引擎</span>` +
          `<span class="palette-item-detail">内置 + 你自己加的（可换引擎）</span></div>`;
        this._items.push({ action: () => { ipc.enginesShow(activeCwd || undefined).catch((e) => console.error(e)); } });

        html +=
          `<div class="palette-item" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:var(--stone)">+</span>` +
          `<span class="palette-item-label">引擎配置示例</span>` +
          `<span class="palette-item-detail">怎么接一个新 CLI</span></div>`;
        this._items.push({ action: () => { ipc.enginesExampleShow().catch((e) => console.error(e)); } });

        // 探测当前终端所用引擎的能力（问引擎自己，不花 token）
        if (activeCwd) {
          const st = this.tm && this.tm.activeId ? this.tm.tabs.get(this.tm.activeId) : null;
          const engineId = (st && st.backend) || 'claude-code';
          html +=
            `<div class="palette-item" data-idx="${this._items.length}">` +
            `<span class="palette-item-icon" style="background:var(--stone)">?</span>` +
            `<span class="palette-item-label">探测引擎能力</span>` +
            `<span class="palette-item-detail">${esc(engineId)} · 问引擎自己（不花 token）</span></div>`;
          this._items.push({
            action: () => { ipc.engineProbeShow(engineId, activeCwd).catch((e) => console.error(e)); },
          });
        }
      }
    }

    // Settings
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--stone)"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:14px;height:14px"><circle cx="8" cy="8" r="2.5"/><path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41"/></svg></span>` +
      `<span class="palette-item-label">偏好设置</span>` +
      `<span class="palette-item-detail">输入 &gt; 编辑</span></div>`;
    this._items.push({ action: () => { this._openSettings?.(); } });

    // M3: Monaco 编辑器原型（探索性，可回退）
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--stone)">{ }</span>` +
      `<span class="palette-item-label">编辑器原型</span>` +
      `<span class="palette-item-kbd">⌘E</span></div>`;
    this._items.push({ action: () => { this._openEditor?.(); } });

    // CC Status summary
    html += '<div class="palette-group-label">状态</div>';
    html += '<div class="palette-item" style="cursor:default;gap:6px">';
    html += '<span style="font-size:11px;color:var(--text-void)">Skills · Hooks · Plugins · Tasks</span>';
    html += '</div>';

    this.results.innerHTML = html;
  }

  /** M1: /model — 打开模型切换 */
  _renderModel(q) {
    let html = '<div class="palette-group-label">模型</div>';
    if (q === '/model' || q === '/model ') {
      html +=
        `<div class="palette-item sel" data-idx="0">` +
        `<span class="palette-item-icon" style="background:var(--id-emerald)">⟳</span>` +
        `<span class="palette-item-label">切换模型…</span>` +
        `<span class="palette-item-detail">F2</span></div>`;
      this._items.push({ action: () => { this._modelSwitch?.(); } });
    } else {
      html += '<div class="palette-item" style="cursor:default"><span class="palette-item-label" style="color:var(--text-void)">输入 /model 回车打开切换器</span></div>';
    }
    this.results.innerHTML = html;
  }

  /**
   * `/roster` 模式：给员工绑定模型（目标 ② 按员工自选模型）。
   *
   * 两级：`/roster` 选员工 → `/roster <agent>` 选供应商。
   * 绑定是异步读的（宿主的 providers.json），所以先渲染"加载中"再补渲染 ——
   * 不在渲染函数里同步阻塞。
   */
  _renderRoster(q) {
    const rest = q.replace('/roster', '').trim();
    const [agentArg] = rest.split(/\s+/);
    const agents = this.agentProvider ? this.agentProvider() : [];

    if (!agentArg) {
      const rows = this._rosterRows || [];
      const byAgent = new Map(rows.map((r) => [r.agent_id, r]));
      let html = '<div class="palette-group-label">员工模型（选一个员工）</div>';
      agents.forEach((a, i) => {
        const r = byAgent.get(a.id);
        const detail = r
          ? `${r.provider_id}${r.model ? ' / ' + r.model : ''}${r.switched_from ? ' ⚠已切换' : ''}`
          : '走全局激活供应商';
        html +=
          `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
          `<span class="palette-item-icon" style="background:${esc(a.color || '#A78BFA')}">${esc((a.icon || a.id || '?')[0].toUpperCase())}</span>` +
          `<span class="palette-item-label">@${esc(a.id)}</span>` +
          `<span class="palette-item-detail">${esc(detail)}</span></div>`;
        this._items.push({ action: () => this._gotoRosterAgent(a.id) });
      });
      if (!agents.length) {
        html += '<div class="palette-item" style="cursor:default"><span class="palette-item-label" style="color:var(--text-void)">未配置 agents</span></div>';
      }
      html += '<div class="palette-group-label">其他</div>';
      html +=
        `<div class="palette-item" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:var(--stone)">Σ</span>` +
        `<span class="palette-item-label">查看花名册</span></div>`;
      this._items.push({ action: () => { ipc.rosterShow().catch((e) => console.error(e)); } });
      this.results.innerHTML = html;
      this._loadRosterRows();
      return;
    }

    // 第二级：选供应商
    const rows = this._providersCache || [];
    let html = `<div class="palette-group-label">@${esc(agentArg)} 用哪个供应商</div>`;
    rows.forEach((p, i) => {
      // ★ 可用性由 Rust 算好（`ProviderCatalogView.usable`）—— 前端不自己抄一份判断。
      // 抄一份就会与后端漂移：本地模型档（Ollama 之类）本来没有 api_key，
      // 前端按"有没有 key"判断就会把可用说成"无凭据"。
      const usable = this._providersUsable ? this._providersUsable.get(p.id) !== false : true;
      html +=
        `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:var(--id-emerald)">${usable ? '✓' : '!'}</span>` +
        `<span class="palette-item-label">${esc(p.id)}</span>` +
        `<span class="palette-item-detail">${esc(p.model || '')}${usable ? '' : ' · 无凭据'}</span></div>`;
      this._items.push({ action: () => this._bindRoster(agentArg, p.id, null) });
    });
    if (!rows.length) {
      html += '<div class="palette-item" style="cursor:default"><span class="palette-item-label" style="color:var(--text-void)">没有供应商；先添加一个</span></div>';
    }
    html +=
      `<div class="palette-item" data-idx="${this._items.length}">` +
      `<span class="palette-item-icon" style="background:var(--stone)">×</span>` +
      `<span class="palette-item-label">解除绑定</span>` +
      `<span class="palette-item-detail">回到全局激活供应商</span></div>`;
    this._items.push({ action: () => this._unbindRoster(agentArg) });
    this.results.innerHTML = html;
    this._loadProvidersCache();
  }

  _gotoRosterAgent(agentId) {
    // 二级命令写回输入框：命令面板是"输入即导航"，不引入新的状态机
    this.input.value = `/roster ${agentId}`;
    this._render();
  }

  async _bindRoster(agentId, providerId, model) {
    try {
      await ipc.rosterSet(agentId, providerId, model);
      this._rosterRows = null; // 失效缓存，下次进模式重新读
      console.log('[palette] 已绑定', agentId, '→', providerId);
    } catch (e) {
      console.error('[palette] 绑定失败：', e);
      this._rosterError = String(e && e.message ? e.message : e);
    }
    this.close?.();
  }

  async _unbindRoster(agentId) {
    try {
      await ipc.rosterClear(agentId);
      this._rosterRows = null;
      console.log('[palette] 已解绑', agentId);
    } catch (e) {
      console.error('[palette] 解绑失败：', e);
    }
    this.close?.();
  }

  async _loadRosterRows() {
    if (this._rosterRows) return;
    try {
      this._rosterRows = (await ipc.rosterList()) || [];
      // 只在还停在第一级时补渲染，避免把用户已经输入的二级命令冲掉
      const v = this.input.value || '';
      if (v.startsWith('/roster') && !v.replace('/roster', '').trim()) this._render();
    } catch (e) {
      console.error('[palette] 读花名册失败：', e);
    }
  }

  async _loadProvidersCache() {
    if (this._providersCache) return;
    try {
      const cat = await ipc.providersList();
      this._providersCache = (cat && cat.providers) || [];
      this._providersUsable = new Map(Object.entries((cat && cat.usable) || {}));
      const v = this.input.value || '';
      if (v.startsWith('/roster')) this._render();
    } catch (e) {
      console.error('[palette] 读供应商失败：', e);
    }
  }

  _renderAgents(q) {
    const agents = this.agentProvider ? this.agentProvider() : [];
    const filtered = q ? agents.filter((a) =>
      (a.name || '').toLowerCase().includes(q) || (a.id || '').toLowerCase().includes(q)
    ) : agents;

    let html = '<div class="palette-group-label">Agents</div>';
    filtered.forEach((a, i) => {
      const color = (a.color && a.color.startsWith('#')) ? a.color : '#A78BFA';
      html +=
        `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:${color}">${esc((a.icon || a.id || '?')[0].toUpperCase())}</span>` +
        `<span class="palette-item-label">@${esc(a.id)} ${esc(a.name || '')}</span></div>`;
      this._items.push({ action: () => {
        // 同上：派发，不插入
        dispatchAgent({
          tabManager: this.tm,
          agentId: a.id,
          agents: filtered,
          onNotice: () => {},
        });
      }});
    });

    if (!filtered.length) {
      html += '<div class="palette-item" style="cursor:default"><span class="palette-item-label" style="color:var(--text-void)">未配置 agents</span></div>';
    }

    this.results.innerHTML = html;
  }

  _renderSettings(q) {
    const settings = [
      { key: 'command', label: '默认命令', getValue: () => 'claude' },
      { key: 'args', label: '默认参数', getValue: () => '--permission-mode bypassPermissions' },
      { key: 'cwd', label: '工作目录', getValue: () => '~/.onecode/workspace' },
      { key: 'max', label: '最大终端数', getValue: () => '10' },
      { key: 'buffer', label: '缓冲区大小 (MB)', getValue: () => '10' },
    ];

    // Try loading from config
    ipc.loadConfig().then((cfg) => {
      settings[0].getValue = () => cfg.default_cmd || 'claude';
      settings[1].getValue = () => (cfg.default_args || []).join(' ');
      settings[2].getValue = () => cfg.default_cwd || '';
      settings[3].getValue = () => String(cfg.max_terminals || 10);
      settings[4].getValue = () => String(cfg.ring_buffer_max_mb || 10);
      this._renderSettingsItems(settings, q);
    }).catch(() => {
      this._renderSettingsItems(settings, q);
    });
  }

  _renderSettingsItems(settings, q) {
    const filtered = q ? settings.filter((s) => s.label.toLowerCase().includes(q)) : settings;
    let html = '<div class="palette-group-label">偏好设置</div>';

    filtered.forEach((s, i) => {
      html +=
        `<div class="palette-item${i === 0 ? ' sel' : ''}" data-idx="${this._items.length}">` +
        `<span class="palette-item-icon" style="background:var(--stone)"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:14px;height:14px"><circle cx="8" cy="8" r="2.5"/><path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41"/></svg></span>` +
        `<span class="palette-item-label">${esc(s.label)}</span>` +
        `<span class="palette-item-detail">${esc(s.getValue())}</span></div>`;
      this._items.push({ action: () => this._editSetting(s) });
    });

    this.results.innerHTML = html;
  }

  /**
   * 产出审查门（人验收）。
   *
   * `cwd` 由渲染时的活跃终端决定 —— 产出记录写在 `<cwd>/.onecode/deliverables/`。
   * 对话框里摆的是：声明的文件（本机 stat 核对过）、按可否机器验证分级的证据、
   * 以及本机自己量到的 git 改动。人点「接受 / 驳回」才写状态。
   */
  async _reviewDeliverables(cwd) {
    try {
      const decision = await ipc.deliverableReview(cwd);
      if (decision) console.log('[palette] 产出验收：', decision);
    } catch (e) {
      console.error('[palette] 产出审查失败：', e);
    }
  }

  /**
   * 验证证据：把 agent 声明的 exec 证据跑一遍。
   *
   * 安全边界在 Rust 侧：**先弹出命令清单让人确认**，确认后才以本机用户身份执行
   * （没有沙箱），超时用进程树清扫。这里只负责触发。
   */
  async _verifyDeliverable(cwd) {
    try {
      const v = await ipc.deliverableVerify(cwd);
      if (v) {
        console.log('[palette] 证据验证：', `${v.results.length} 条，${v.results.filter((r) => r.exit_code === 0 && !r.timed_out).length} 通过`);
      }
    } catch (e) {
      console.error('[palette] 证据验证失败：', e);
    }
  }

  /** 任务板（只读）：把任务、状态来源、产出关联摆出来看。 */
  async _showTaskBoard(cwd) {
    try {
      await ipc.taskBoard(cwd);
    } catch (e) {
      console.error('[palette] 打开任务板失败：', e);
    }
  }

  /** 任务裁决门：请人在对话框里点「完成 / 退回重做」。 */
  async _reviewTask(cwd) {
    try {
      const decision = await ipc.taskReview(cwd);
      if (decision) console.log('[palette] 任务裁决：', decision);
    } catch (e) {
      console.error('[palette] 任务裁决失败：', e);
    }
  }

  /**
   * 派活：把**最早的未完成任务**正文填进当前活跃终端。
   *
   * 不自己挑任务 —— 排序规则由 Rust 的 `task_list` 决定（未完成的、有问题的在前），
   * 这里只取第一条。规则抄两份就会漂移。
   */
  async _dispatchTask(cwd) {
    try {
      const board = await ipc.taskList(cwd);
      const open = (board && board.tasks ? board.tasks : []).find(
        (t) => t.status !== 'done' && t.status !== 'dropped'
      );
      if (!open) {
        console.log('[palette] 没有未完成的任务可派');
        return;
      }
      const ok = sendTaskToSlot({
        tabManager: this.tm,
        task: open,
        ptyWrite: ipc.ptyWrite,
        onNotice: (m) => console.warn('[palette]', m),
      });
      if (ok) console.log('[palette] 已填入任务：', open.id, '（回车发送）');
    } catch (e) {
      console.error('[palette] 派活失败：', e);
    }
  }

  /**
   * 账本：任务/产出/验收/证据 + 本机实测的 token 用量。
   *
   * 金额不显示 —— transcript 里没有成本字段，算金额需要价格表；
   * 编一个数字出来比不报更糟（账本里会写明这一点）。
   */
  async _showLedger(cwd, full = false) {
    try {
      await ipc.ledgerShow(cwd, full);
    } catch (e) {
      console.error('[palette] 打开账本失败：', e);
    }
  }

  _editSetting(setting) {
    // Replace the palette content with an inline input for this setting
    const current = setting.getValue();
    this.results.innerHTML =
      `<div class="palette-group-label">编辑：${esc(setting.label)}</div>` +
      `<div class="palette-item" style="padding:8px 20px">` +
      `<input class="palette-setting-input" id="settingInput" value="${esc(current)}">` +
      `</div>` +
      `<div class="palette-item" style="cursor:default;padding:4px 20px">` +
      `<span style="font-size:11px;color:var(--text-void)">Enter 保存 · Escape 取消</span></div>`;

    const input = document.getElementById('settingInput');
    input.focus();
    input.select();

    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        const val = input.value.trim();
        this._saveSetting(setting.key, val);
        this.close();
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        this.input.value = '> ';
        this._render();
      }
      e.stopPropagation();
    });

    input.addEventListener('blur', () => {
      // Don't auto-close on blur, let Escape handle it
    });
  }

  async _saveSetting(key, value) {
    // P1-18 fix: read-merge-write to avoid partial config overwriting other keys.
    // If loadConfig fails, fall back to saving just the changed key (better than nothing).
    let cfg = {};
    try {
      cfg = await ipc.loadConfig();
      if (!cfg || typeof cfg !== 'object') cfg = {};
    } catch (_) {
      // Use empty base — first-run or backend unreachable
    }

    if (key === 'command') cfg.default_cmd = value;
    else if (key === 'args') cfg.default_args = value.split(/\s+/).filter(Boolean);
    else if (key === 'cwd') cfg.default_cwd = value;
    else if (key === 'max') cfg.max_terminals = parseInt(value) || 10;
    else if (key === 'buffer') cfg.ring_buffer_max_mb = parseInt(value) || 10;

    // P1-18: Visual feedback on save result
    const feedbackTarget = this.results.querySelector('.palette-group-label');
    try {
      await ipc.saveConfig(cfg);
      // Success: brief green flash on the label
      if (feedbackTarget) {
        feedbackTarget.style.color = '#4ade80';
        setTimeout(() => { feedbackTarget.style.color = ''; }, 200);
      }
    } catch (e) {
      console.warn('[palette] save failed', e);
      // Failure: red flash + ✗ indicator
      if (feedbackTarget) {
        const origText = feedbackTarget.textContent;
        feedbackTarget.style.color = '#f87171';
        feedbackTarget.innerHTML = origText + ' <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" style="width:10px;height:10px;vertical-align:-1px"><line x1="3" y1="3" x2="9" y2="9"/><line x1="9" y1="3" x2="3" y2="9"/></svg>';
        setTimeout(() => {
          feedbackTarget.style.color = '';
          feedbackTarget.textContent = origText;
        }, 1200);
      }
    }
  }

  _orbColorForId(id) {
    const orb = document.querySelector(`.orb[data-id="${id}"]`);
    if (orb) {
      const c = orb.dataset.identityColor;
      if (c) {
        // If already a var() expression, return as-is; if a bare CSS variable, wrap with var()
        if (c.startsWith('var(')) return c;
        if (c.startsWith('--')) return `var(${c})`;
        return c; // hex or other valid CSS value
      }
    }
    return 'var(--id-emerald)';
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}
