// @mention 弹窗控制器（P1-6）。
// 移植自 onecode/agent-runtime/gateway/onecode.html 的 showMention/hideMention + 键盘导航。
//
// 行为：
// - term.onData 跟踪用户输入，检测 `@<word>` 触发弹窗（agent 列表来自 cc_status.agents）。
// - 选中：发 N 个 backspace 清除已输入的 `@prefix`（这些字符已实时发往 PTY 并被回显），
//   再发 `@id `。
// - 导航键（↑↓ Enter Tab Esc）在弹窗可见时于「捕获阶段」拦截，不传给 xterm，
//   避免产生 ESC 序列污染输入跟踪。
//
// 注：每个终端一个 controller；共享同一个 .mention-pop 弹窗 DOM（同一时刻仅活跃终端可见）。

const BACKSPACE = '\x7f';
const PREFIX_CHARS = /[0-9A-Za-z._\-]/;

export class MentionController {
  constructor({ term, termEl, popEl, sendInput, getAgents }) {
    this.term = term;
    this.termEl = termEl;
    this.popEl = popEl;
    this.sendInput = sendInput; // (str) => void  写入 PTY
    this.getAgents = getAgents; // () => AgentInfo[]
    this.active = false;
    this.prefix = '';
    this.idx = -1;

    term.onData((d) => this._onInput(d));
    termEl.addEventListener('keydown', (e) => this._onKeyDown(e), true); // capture
    termEl.addEventListener('focusout', () => this.hide());
  }

  /** 用户输入流。逐字符识别 @ 触发 / 累积前缀 / 非词字符收尾。 */
  _onInput(data) {
    for (const ch of data) {
      if (!this.active) {
        if (ch === '@') {
          this.active = true;
          this.prefix = '';
          this._render();
        }
        continue;
      }
      if (PREFIX_CHARS.test(ch)) {
        this.prefix += ch;
        this._render();
      } else {
        this.hide();
      }
    }
  }

  /** 捕获阶段拦截导航键（弹窗可见时）。 */
  _onKeyDown(e) {
    if (!this.active || !this.popEl || !this.popEl.classList.contains('on')) return;
    const items = this.popEl.querySelectorAll('.mp-item');
    if (!items.length) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault(); e.stopPropagation();
      this.idx = Math.min(this.idx + 1, items.length - 1);
      this._markSel(items);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault(); e.stopPropagation();
      this.idx = Math.max(this.idx - 1, 0);
      this._markSel(items);
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      const target = this.idx >= 0 ? items[this.idx] : items[0];
      if (target) {
        e.preventDefault(); e.stopPropagation();
        this._select(target);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault(); e.stopPropagation();
      this.hide();
    }
  }

  get matches() {
    const p = this.prefix.toLowerCase();
    return (this.getAgents() || []).filter(
      (a) =>
        a.id.toLowerCase().startsWith(p) ||
        (a.name || '').toLowerCase().startsWith(p),
    );
  }

  _render() {
    if (!this.popEl) return;
    const matches = this.matches;
    if (!matches.length) {
      this.hide();
      return;
    }
    this.idx = 0; // 预选第一项（Enter 直接选中）
    this.popEl.innerHTML = matches.map((a, i) => this._itemHtml(a, i)).join('');
    this.popEl.classList.add('on');
    const items = this.popEl.querySelectorAll('.mp-item');
    this._markSel(items);
    items.forEach((el) => {
      el.addEventListener('mouseenter', () => {
        this.idx = Number(el.dataset.idx);
        this._markSel(items);
      });
      el.addEventListener('mousedown', (ev) => {
        ev.preventDefault(); // 阻止失焦
        this._select(el);
      });
    });
  }

  _itemHtml(a, i) {
    const color = normalizeColor(a.color);
    const letter = (a.name || a.id || '?').charAt(0).toUpperCase();
    const icon = a.icon
      ? `<span class="mp-icon" style="background:${color}22;color:${color}">${esc(a.icon)}</span>`
      : `<span class="mp-icon mp-letter" style="background:${color}22;color:${color}">${esc(letter)}</span>`;
    return (
      `<div class="mp-item" data-idx="${i}" data-agent="${esc(a.id)}">${icon}` +
      `<span class="mp-name">${esc(a.name || a.id)}</span>` +
      `<span class="mp-id">@${esc(a.id)}</span>` +
      `<span class="mp-desc">${esc(a.description || '')}</span></div>`
    );
  }

  _markSel(items) {
    items.forEach((el, i) => el.classList.toggle('sel', i === this.idx));
  }

  _select(el) {
    const aid = el.dataset.agent;
    this.hide();
    if (!aid) return;
    // 清除已输入的 @prefix：N = prefix.length + 1（含 @）
    const n = this.prefix.length + 1;
    if (n > 0) this.sendInput(BACKSPACE.repeat(n));
    this.sendInput(`@${aid} `);
    this.term.focus();
  }

  hide() {
    this.active = false;
    this.prefix = '';
    this.idx = -1;
    if (this.popEl) {
      this.popEl.classList.remove('on');
      this.popEl.innerHTML = '';
    }
  }
}

function normalizeColor(c) {
  if (!c || !c.startsWith('#')) return '#A78BFA';
  return c;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (ch) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]
  ));
}
