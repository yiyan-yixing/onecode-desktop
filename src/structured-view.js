// 结构化槽位的视图 —— 目标 ①「可换引擎」不该要求引擎长得像终端。
//
// 与终端槽位的区别：这里显示的是**谁说了什么、这一轮花了多少**，而不是一堆字节。
//
// 分工纪律（与 work-panel.js 同源）：**判断与聚合在 Rust，排版在 JS**。
// 合计金额 / 用量 / "引擎回的会话 id 与请求的不一致" 全部由 Rust 算好放进
// `StructuredSlotView`，这里只负责把它们排出来 —— 前端一旦自己把 turns 加一遍，
// 就会与后端漂移（第 9 刀本地档、第 12 刀工作台都吃过这个亏）。

/** HTML 转义：回复内容是引擎产出的，属不可信输入。 */
export function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function money(n) {
  const v = Number(n) || 0;
  return '$' + v.toFixed(6);
}

function shortSession(id) {
  return id ? String(id).slice(0, 8) : '(无)';
}

/**
 * 把一个结构化槽位排成 HTML。**纯函数**（不碰 DOM），所以能直接单测。
 *
 * @param {object} view Rust `structured_*` 命令返回的 `StructuredSlotView`
 * @returns {string} HTML
 */
export function buildStructuredHtml(view) {
  if (!view || !view.id) {
    return '<div class="ss-empty">还没有会话。</div>';
  }
  const turns = Array.isArray(view.turns) ? view.turns : [];

  let h = '';

  // ── 头部：身份 / 路由 / 会话 / 合计（合计来自 Rust，不重算）──
  h += '<div class="ss-head">';
  const who = view.agent ? '@' + esc(view.agent) : '(未指派员工)';
  const route = [view.provider_id, view.model].filter(Boolean).map(esc).join(' / ') || '跟随全局';
  h += `<div class="ss-title">${who} <span class="ss-route">${route}</span></div>`;
  // ★ 口径更正：实测证明引擎的 session-id **不接续上下文**（同 id 第二轮输入从
  // 10038 掉到 367、模型说"没有之前的对话记录"）。所以这里**不叫"会话"**——
  // 叫"标签"，并写明多轮靠本机回放。把不接续的东西显示成"会话"就是误导。
  h += `<div class="ss-meta">标签 ${esc(shortSession(view.session_id))}`;
  if (view.engine_session_id) h += ` → 引擎回 ${esc(shortSession(view.engine_session_id))}`;
  h += ` ｜ ${turns.length} 轮 ｜ 合计 ${money(view.total_cost_usd)}`;
  h += ` ｜ 输入 ${view.total_input_tokens || 0} ｜ 输出 ${view.total_output_tokens || 0}`;
  h += '</div>';
  h += '<div class="ss-meta">多轮上下文由本机回放（引擎的 session-id 只作标签，不接续）</div>';
  // 引擎回了不同的会话 id → 接续可能没按预期走，必须显眼
  if (view.session_mismatch) {
    h += '<div class="ss-warn">⚠ 引擎回的会话 id 与我们请求的不一致 —— 多轮接续可能没按预期走</div>';
  }
  h += '</div>';

  // ── 对话 ──
  h += '<div class="ss-turns">';
  if (!turns.length) {
    h += '<div class="ss-empty">还没有说话。在下面输入第一句。</div>';
  }
  for (const t of turns) {
    h += '<div class="ss-turn">';
    h += `<div class="ss-prompt">&gt; ${esc(t.prompt)}</div>`;
    if (t.ok) {
      h += `<div class="ss-reply">${esc(t.reply)}</div>`;
    } else {
      h += `<div class="ss-fail">⚠ 失败：${esc(t.error || '未知原因')}</div>`;
    }
    h += `<div class="ss-cost">${money(t.cost_usd)} ｜ 输入 ${t.input_tokens || 0} ｜ 输出 ${t.output_tokens || 0} ｜ ${t.duration_ms || 0}ms</div>`;
    // 协议特有的事实（ACP 的 stopReason / 上下文占用 / 权限请求次数…）由后端算好，这里只转述
    if (t.extra_meta) h += `<div class="ss-cost">${esc(t.extra_meta)}</div>`;
    h += '</div>';
  }
  h += '</div>';

  // ── 输入 ──
  h += '<form class="ss-form" data-ss-form>';
  h += '<input class="ss-input" data-ss-input placeholder="说一句…（Enter 发送）" autocomplete="off">';
  h += '<button class="ss-send" type="submit">发送</button>';
  h += '</form>';
  return h;
}

/**
 * 控制器：只做三件事 —— 取数、渲染、把发送转给命令。**不自己算任何合计**。
 */
export class StructuredViewController {
  constructor({ ipc, cwd, agent, providerId, model, onError } = {}) {
    this.ipc = ipc;
    this.cwd = cwd;
    this.agent = agent || null;
    this.providerId = providerId || null;
    this.model = model || null;
    this.onError = typeof onError === 'function' ? onError : (m) => console.warn('[structured]', m);
    this.el = null;
    this.view = null;
    this._sending = false;
  }

  async init(container) {
    this.el = container;
    if (!this.el) return;
    this.el.innerHTML = '<div class="ss-empty">正在开会话…</div>';
    this.el.addEventListener('submit', (e) => {
      if (e.target.matches('[data-ss-form]')) {
        e.preventDefault();
        const input = e.target.querySelector('[data-ss-input]');
        this.send(input ? input.value : '');
      }
    });
    try {
      this.view = await this.ipc.structuredOpen(this.cwd, this.agent, this.providerId, this.model);
      this.render();
    } catch (e) {
      this.el.innerHTML = `<div class="ss-empty ss-fail">开会话失败：${esc(e && e.message ? e.message : e)}</div>`;
    }
  }

  render() {
    if (!this.el) return;
    this.el.innerHTML = buildStructuredHtml(this.view);
    const list = this.el.querySelector('.ss-turns');
    if (list) list.scrollTop = list.scrollHeight;
  }

  async send(text) {
    const prompt = String(text == null ? '' : text).trim();
    if (!prompt || this._sending || !this.view) return;
    this._sending = true;
    const btn = this.el && this.el.querySelector('.ss-send');
    if (btn) { btn.disabled = true; btn.textContent = '…'; }
    try {
      // 发送期间把这一句先显示出来（否则点了没反应像卡住）；回复到了再整体重渲染
      if (this.el) {
        const list = this.el.querySelector('.ss-turns');
        if (list) list.insertAdjacentHTML('beforeend', `<div class="ss-turn ss-pending"><div class="ss-prompt">&gt; ${esc(prompt)}</div><div class="ss-cost">等待引擎…</div></div>`);
      }
      this.view = await this.ipc.structuredSend(this.cwd, this.view.id, prompt);
      this.render();
    } catch (e) {
      this.onError(`发送失败：${e && e.message ? e.message : e}`);
    } finally {
      this._sending = false;
      const b = this.el && this.el.querySelector('.ss-send');
      if (b) { b.disabled = false; b.textContent = '发送'; }
      const input = this.el && this.el.querySelector('[data-ss-input]');
      if (input) { input.value = ''; input.focus(); }
    }
  }
}
