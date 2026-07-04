// Agents 列表控制器 — 侧边栏「Agents」Tab，展示 .claude/agents/ 中的 agent 定义。
//
// 数据源复用 CcStatusView 的 agents 列表（通过 onAgents 回调注入）。
// 渲染：icon + name + @id + description + scope 标签。

const SCOPE_COLORS = {
  project: { bg: 'rgba(5,150,105,.08)', color: 'var(--em)', border: 'rgba(5,150,105,.15)' },
  global:  { bg: 'rgba(167,139,250,.08)', color: 'var(--lavender)', border: 'rgba(167,139,250,.15)' },
};

const IDENTITY_COLORS = [
  '#10B981', '#22D3EE', '#A78BFA', '#F472B6',
  '#F7C948', '#7DD3FC', '#FB923C', '#84CC16',
  '#D946EF', '#2DD4BF',
];

/** 根据 agent id 生成稳定颜色（哈希取模） */
function colorForId(id) {
  let hash = 0;
  for (let i = 0; i < id.length; i++) {
    hash = ((hash << 5) - hash + id.charCodeAt(i)) | 0;
  }
  return IDENTITY_COLORS[Math.abs(hash) % IDENTITY_COLORS.length];
}

/** 校验颜色值是否为合法 6 位 hex（防止 CSS 注入） */
function isValidHexColor(c) {
  return /^#[0-9a-fA-F]{6}$/.test(c);
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** 合法的 Tab ID 白名单 */
const VALID_TABS = new Set(['file', 'agents']);

export class AgentsListController {
  constructor() {
    this._container = null;
    this._agents = [];
    this._listEl = null;
    this._provider = null; // () => agents[]
  }

  /** 初始化：container 是 agents tab 的内容容器 DOM */
  init(container) {
    this._container = container;
    this._buildUI();
  }

  /** 设置 agents 数据提供者（从 CcStatusView.onAgents 回调注入） */
  setProvider(provider) {
    this._provider = provider;
  }

  /** 刷新 agents 列表（数据变更时调用） */
  refresh() {
    if (!this._provider || !this._listEl) return;
    const agents = this._provider();
    if (!Array.isArray(agents)) return;
    if (JSON.stringify(agents) !== JSON.stringify(this._agents)) {
      this._agents = agents;
      this._renderList();
    }
  }

  /** 强制刷新（项目切换时） */
  forceRefresh() {
    this._agents = [];
    this.refresh();
  }

  _buildUI() {
    const c = this._container;

    // Agents 列表
    const list = document.createElement('div');
    list.className = 'al-list';
    const empty = document.createElement('div');
    empty.className = 'al-empty';
    empty.textContent = '暂无 agents';
    list.appendChild(empty);
    c.appendChild(list);
    this._listEl = list;
  }

  _renderList() {
    const list = this._listEl;
    list.innerHTML = '';

    if (this._agents.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'al-empty';
      empty.textContent = '暂无 agents';
      list.appendChild(empty);
      return;
    }

    // 先 project scope，后 global scope
    const sorted = [...this._agents].sort((a, b) => {
      if (a.scope === b.scope) return a.name.localeCompare(b.name);
      return a.scope === 'project' ? -1 : 1;
    });

    sorted.forEach((agent) => {
      const item = document.createElement('div');
      item.className = 'al-item';
      item.dataset.scope = agent.scope;
      item.dataset.id = agent.id;

      // 图标颜色：优先使用 agent.color（需通过 hex 校验），否则自动生成
      const color = (agent.color && isValidHexColor(agent.color)) ? agent.color : colorForId(agent.id);
      const iconText = agent.icon || (agent.name && agent.name.charAt(0).toUpperCase()) || '?';

      // Scope 标签颜色
      const scopeStyle = SCOPE_COLORS[agent.scope] || SCOPE_COLORS.global;

      item.innerHTML =
        `<span class="al-icon" style="background:${color}18;color:${color};border:1px solid ${color}30">${esc(iconText)}</span>` +
        `<div class="al-info">` +
          `<div class="al-name-row">` +
            `<span class="al-name">${esc(agent.name)}</span>` +
            `<span class="al-id">@${esc(agent.id)}</span>` +
          `</div>` +
          (agent.description
            ? `<div class="al-desc">${esc(agent.description)}</div>`
            : '') +
        `</div>` +
        `<span class="al-scope" style="background:${scopeStyle.bg};color:${scopeStyle.color};border:1px solid ${scopeStyle.border}">${esc(agent.scope)}</span>`;

      list.appendChild(item);
    });
  }
}
