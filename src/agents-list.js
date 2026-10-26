// Agents 列表控制器 — 侧边栏「Agents」Tab，展示 .claude/agents/ 中的 agent 定义。
//
// 数据源复用 CcStatusView 的 agents 列表（通过 onAgents 回调注入）。
// 渲染：icon + name + @id + description + scope 标签。
//
// ★ 点击语义（@name 真派发）：
//   - **点击** = 派发：新开一个以该 agent 为身份的 Slot（自己的 system prompt、
//     自己的 transcript、自己的权限姿态），而不是往当前终端打 `@name` 文本。
//   - **⌥/Ctrl/Cmd + 点击** = 在与活跃终端同一会话里临时点名（插入 `@id `）。
//   两种语义必须可区分 —— 否则"管理员工"会退化成"打字"。

import { dispatchAgent, insertAgentMention, isMentionModifier } from './agent-dispatch.js';

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

/** 防抖阈值：两次点击间隔 < 300ms 视为重复，忽略 */
const CLICK_DEBOUNCE_MS = 300;

export class AgentsListController {
  constructor() {
    this._container = null;
    this._agents = [];
    this._listEl = null;
    this._provider = null; // () => agents[]
    this._tabManager = null; // TabManager 引用（F2: 获取活跃终端）
    this._ptyWrite = null; // (id, data) => void（F2: 写入 PTY 的回调，从 main.js 注入）
    this._lastClickTime = 0; // 防抖时间戳
    this._toastTimer = null; // toast 自动消失定时器
    this._terminalStateChanged = true; // P1-1 fix: 终端状态变更标记，确保首次/切换时重新渲染
  }

  /** 初始化：container 是 agents tab 的内容容器 DOM */
  init(container) {
    this._container = container;
    this._buildUI();
  }

  /** 注入 TabManager 引用（F2: 需要获取 activeId 和终端状态） */
  setTabManager(tabManager) {
    this._tabManager = tabManager;
  }

  /** 注入 ptyWrite 回调（F2: 写入 PTY 的函数，即 ipc.ptyWrite） */
  setPtyWrite(ptyWrite) {
    this._ptyWrite = ptyWrite;
  }

  /** 设置 agents 数据提供者（从 CcStatusView.onAgents 回调注入） */
  setProvider(provider) {
    this._provider = provider;
  }

  /**
   * 标记终端状态已变更（TabManager.onChange 调用）。
   * P1-1 fix: 解耦终端状态检查与 agents 数据比较，
   * 确保关闭最后一个终端后切到 agents tab 时 disabled 状态正确更新。
   */
  markTerminalStateChanged() {
    this._terminalStateChanged = true;
  }

  /** 刷新 agents 列表（数据变更或终端状态变更时调用） */
  refresh() {
    if (!this._listEl) return;
    const provider = this._provider || (this._tabManager && this._tabManager.agentProvider);
    if (!provider) return;
    const agents = provider();
    if (!Array.isArray(agents)) return;
    // Agent objects come from CcStatusView with stable key order;
    // JSON.stringify is used as a cheap structural equality check.
    if (JSON.stringify(agents) !== JSON.stringify(this._agents) || this._terminalStateChanged) {
      this._agents = agents;
      this._terminalStateChanged = false;
      this._renderList();
    }
  }

  /** 强制刷新（项目切换时） */
  forceRefresh() {
    this._agents = [];
    this._terminalStateChanged = true;
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

    const hasActiveTerminal = this._tabManager && this._tabManager.activeId
      && this._tabManager.tabs.get(this._tabManager.activeId)
      && !this._tabManager.tabs.get(this._tabManager.activeId).isError;

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
      if (!hasActiveTerminal) {
        item.classList.add('al-disabled');
      }
      item.dataset.scope = agent.scope;
      item.dataset.id = agent.id;
      // P2-4: 键盘可访问性 — role/tabindex 使 Tab 导航和屏幕阅读器可用
      item.setAttribute('role', 'button');
      item.setAttribute('tabindex', '0');

      // 图标颜色：优先使用 agent.color（需通过 hex 校验），否则自动生成
      const color = (agent.color && isValidHexColor(agent.color)) ? agent.color : colorForId(agent.id);
      const iconText = agent.icon || (agent.name && agent.name.charAt(0).toUpperCase()) || '?';

      // Scope 标签颜色（未知 scope 回退到 global 样式）
      const scopeStyle = SCOPE_COLORS[agent.scope] || SCOPE_COLORS.global;

      item.innerHTML =
        `<span class="al-icon" style="background:${color}18;color:${color};border:1px solid ${color}30">${esc(iconText)}</span>` +
        `<div class="al-info">` +
          `<div class="al-name-row">` +
            `<span class="al-name">${esc(agent.name)}</span>` +
          `</div>` +
          (agent.description
            ? `<div class="al-desc">${esc(agent.description)}</div>`
            : '') +
        `</div>` +
        `<span class="al-scope" style="background:${scopeStyle.bg};color:${scopeStyle.color};border:1px solid ${scopeStyle.border}">${esc(agent.scope)}</span>`;

      // 说明点击语义（否则用户不知道点一下会发生什么）
      item.title = `点击 = 以 @${agent.id} 的身份新开一个终端\n⌥/Ctrl/Cmd + 点击 = 在当前终端插入 @${agent.id}`;

      // ★ 点击 = 派发新员工 Slot；⌥/Ctrl/Cmd + 点击 = 临时点名（保留旧行为）
      item.addEventListener('click', (e) => this._onAgentClick(agent.id, item, {
        insertMention: isMentionModifier(e),
      }));
      // P2-4: 键盘支持 — Enter/Space 走派发
      item.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          this._onAgentClick(agent.id, item);
        }
      });

      list.appendChild(item);
    });
  }

  /**
   * 点击 agent 项。
   * - 默认：**派发** —— 新开一个以该 agent 为身份的 Slot（返回 Promise，便于等待）。
   * - `insertMention`：把 `@id ` 写进当前终端（同会话临时点名）。
   * 防抖：300ms 内重复点击忽略。
   */
  _onAgentClick(agentId, itemEl, opts = {}) {
    const now = Date.now();
    if (now - this._lastClickTime < CLICK_DEBOUNCE_MS) return Promise.resolve();
    this._lastClickTime = now;

    if (opts.insertMention) {
      this._insertMention(agentId, itemEl);
      return Promise.resolve();
    }
    return this._dispatchAgent(agentId, itemEl);
  }

  /**
   * 派发：以该 agent 为身份**新开一个 Slot**。
   * 实现见 `agent-dispatch.js` —— 三处可点 agent 的界面共用同一份，
   * 避免"改一处漏三处"。
   */
  async _dispatchAgent(agentId, itemEl) {
    itemEl.classList.add('al-mention-flash');
    setTimeout(() => itemEl.classList.remove('al-mention-flash'), 400);
    await dispatchAgent({
      tabManager: this._tabManager,
      agentId,
      agents: typeof this._provider === 'function' ? this._provider() : this._agents,
      onNotice: (m) => this._showToast(m),
    });
  }

  /**
   * 临时点名：向活跃终端插入 `@agent-id `。
   * 实现见 `agent-dispatch.js`。
   */
  _insertMention(agentId, itemEl) {
    const ok = insertAgentMention({
      tabManager: this._tabManager,
      agentId,
      ptyWrite: this._ptyWrite,
      onNotice: (m) => this._showToast(m),
    });
    if (ok) {
      const st = this._tabManager.tabs.get(this._tabManager.activeId);
      if (st) this._flashAndFocus(itemEl, st);
    }
  }

  /** 视觉反馈：短暂高亮 + 聚焦终端 */
  _flashAndFocus(itemEl, st) {
    itemEl.classList.add('al-mention-flash');
    setTimeout(() => itemEl.classList.remove('al-mention-flash'), 400);
    // P2-5: focus 失败时轻量日志，避免完全静默吞掉错误
    try { st.term.focus(); } catch (e) { console.warn('[agents-list] focus failed:', e); }
  }

  /** 底部提示 toast（派发缺上下文、派发失败、无终端等都用它） */
  _showToast(msg) {
    let toast = this._container.querySelector('.al-toast');
    if (!toast) {
      toast = document.createElement('div');
      toast.className = 'al-toast';
      toast.innerHTML = '<span class="al-toast-msg"></span>';
      this._container.appendChild(toast);
    }
    toast.querySelector('.al-toast-msg').textContent = msg;
    toast.classList.add('on');
    if (this._toastTimer) clearTimeout(this._toastTimer);
    this._toastTimer = setTimeout(() => {
      toast.classList.remove('on');
    }, 2400);
  }
}
