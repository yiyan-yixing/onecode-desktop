// 工作台面板 —— 目标 ④（管理产出）与 ⑤（管理工作）的**日常入口**。
//
// 为什么要有它：到目前为止，产出审查 / 证据验证 / 任务板 / 账本 全都只能从
// ⌘K 命令面板或对话框进去。"管理"要的是一个**常驻的面板**，不是一个个弹窗。
//
// ── 分工（这条比代码本身重要）────────────────────────────────────────
// **判断在 Rust，排版在 JS。**
//
// "这条证据验过没有"「这个任务的完成是不是 agent 自报的」「声明的文件到底存不存在」
// —— 全部由 Rust 算好放进返回值（`verification` / `status_source` / `problem` /
// `files[].exists` / `deliverable_refs[].verified`）。面板**只负责把它们排出来**。
//
// 前端一旦自己重算一遍，就会与后端漂移 —— 第 9 刀的本地模型档就是这么被前端
// 误判成"无凭据"的（同一判断两处实现，规则 7）。所以这个文件里**没有**任何
// "可用/已验证/完成"的判断逻辑，只有格式与排序。

/** HTML 转义：标题/备注都是 agent 写进文件的内容，属于不可信输入。 */
export function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

const DONE_STATUSES = new Set(['done', 'dropped']);

/**
 * 把「产出扫描 + 任务板」排成 HTML。**纯函数**（不碰 DOM），所以能直接单测。
 *
 * @param {object} o
 * @param {object} [o.deliverables] Rust `deliverables_scan` 的返回
 * @param {object} [o.tasks]        Rust `task_list` 的返回
 * @returns {string} HTML
 */
export function buildWorkHtml({ deliverables, tasks } = {}) {
  const ds = (deliverables && deliverables.deliverables) || [];
  const ts = (tasks && tasks.tasks) || [];

  const pending = ds.filter((d) => d.status !== 'accepted' && d.status !== 'rejected');
  const open = ts.filter((t) => !DONE_STATUSES.has(t.status));
  // ★ agent 自报"完成"、但**没有人裁决过**的任务。
  // 渲染夹具实测：早先只按 status 过滤，这类任务整个从面板消失 ——
  // 于是"它自称完成了"这件事恰恰没人看得见，而它正是最该被看见的
  // （④ 里"模型自述不算证据"的同一条规矩，在 ⑤ 上的对应物）。
  const selfClaimed = ts.filter((t) => DONE_STATUSES.has(t.status) && t.status_source !== 'human');

  let html = '';

  // ── 摘要 ──
  const unverified = ds.filter((d) => !d.verification).length;
  html += '<div class="wp-summary">';
  html += `<span class="wp-chip">待验收 ${pending.length}</span>`;
  html += `<span class="wp-chip">未完成任务 ${open.length}</span>`;
  html += `<span class="wp-chip${unverified ? ' wp-warn' : ''}">证据未验证 ${unverified}</span>`;
  if (selfClaimed.length) {
    html += `<span class="wp-chip wp-warn">自报完成待裁决 ${selfClaimed.length}</span>`;
  }
  html += '</div>';

  // ── agent 自报完成（未经人裁决）──
  if (selfClaimed.length) {
    html += '<div class="wp-group wp-group-warn">任务 · agent 自报完成（未经你裁决）</div>';
    html += '<div class="wp-line wp-warn">这些任务的完成是<b>文件里写的</b>，不是人裁决的 —— 按我们的规矩，它还不算完成。</div>';
    for (const t of selfClaimed) {
      html += renderTask(t);
    }
  }

  // ── 待验收产出（审查队列）──
  html += '<div class="wp-group">产出 · 待验收</div>';
  if (!ds.length) {
    html += '<div class="wp-empty">这个项目还没有产出记录。<br>员工的产出协议要求把交付写成 <code>.onecode/deliverables/&lt;id&gt;.json</code>。</div>';
  } else if (!pending.length) {
    html += `<div class="wp-empty">${ds.length} 条产出都已处理完。</div>`;
  }
  for (const d of pending) {
    html += renderDeliverable(d);
  }

  // ── 未完成任务 ──
  html += '<div class="wp-group">任务 · 未完成</div>';
  if (!ts.length) {
    html += '<div class="wp-empty">这个项目还没有任务记录。</div>';
  } else if (!open.length) {
    html += `<div class="wp-empty">${ts.length} 条任务都已完成。</div>`;
  }
  for (const t of open) {
    html += renderTask(t);
  }

  return html;
}

function renderDeliverable(d) {
  const files = d.files || [];
  const okFiles = files.filter((f) => f.exists).length;
  const evidence = d.evidence || [];
  const v = d.verification;

  // 证据分级计数（等级本身是 Rust 判的，这里只数）
  const byGrade = { 'machine-runnable': 0, measured: 0, unverifiable: 0, 'not-evidence': 0 };
  for (const e of evidence) byGrade[e.grade] = (byGrade[e.grade] || 0) + 1;

  let h = '<div class="wp-item" data-wp-id="' + esc(d.id) + '">';
  h += `<div class="wp-title">${esc(d.title || d.id)}</div>`;
  h += `<div class="wp-meta">${esc(d.id)} · ${esc(d.status)}</div>`;
  if (d.summary) h += `<div class="wp-note">${esc(d.summary)}</div>`;

  // 问题排在最显眼处（与 Rust 的 review_text 同一原则）
  if (d.problem) h += `<div class="wp-problem">⚠ ${esc(d.problem)}</div>`;

  if (files.length) {
    const missing = files.length - okFiles;
    h += `<div class="wp-line${missing ? ' wp-warn' : ''}">文件 ${okFiles}/${files.length} 存在`;
    if (missing) h += `（${missing} 个不存在）`;
    h += '</div>';
  }

  if (evidence.length) {
    const parts = [];
    if (byGrade['machine-runnable']) parts.push(`可验证 ${byGrade['machine-runnable']}`);
    if (byGrade.measured) parts.push(`可测量 ${byGrade.measured}`);
    if (byGrade.unverifiable) parts.push(`核不了 ${byGrade.unverifiable}`);
    if (byGrade['not-evidence']) parts.push(`不算证据 ${byGrade['not-evidence']}`);
    h += `<div class="wp-line">证据：${esc(parts.join(' · '))}</div>`;
  }

  // 验证结果：**"声明了"与"验过了"必须能区分**（这是整个 ④ 的核心）
  if (!v) {
    const runnable = byGrade['machine-runnable'] || 0;
    h += `<div class="wp-line${runnable ? ' wp-warn' : ''}">证据 <b>未验证</b>${runnable ? `（有 ${runnable} 条可跑）` : ''}</div>`;
  } else {
    const passed = v.results.filter((r) => !r.timed_out && r.exit_code === 0).length;
    const cls = passed === v.results.length ? '' : ' wp-warn';
    h += `<div class="wp-line${cls}">实测：${passed}/${v.results.length} 通过</div>`;
  }

  h += '<div class="wp-actions">';
  h += `<button class="wp-btn" data-wp-action="review" data-wp-id="${esc(d.id)}">审查 / 裁决</button>`;
  h += `<button class="wp-btn" data-wp-action="verify" data-wp-id="${esc(d.id)}">跑证据</button>`;
  h += '</div></div>';
  return h;
}

function renderTask(t) {
  const refs = t.deliverable_refs || [];
  // 完成状态的**来源**由 Rust 判断（human / declared）—— 面板只显示
  const statusMark =
    t.status_source === 'human' ? `${t.status}（人裁决）` : `${t.status}（文件声明）`;

  let h = '<div class="wp-item" data-wp-id="' + esc(t.id) + '">';
  h += `<div class="wp-title">${esc(t.title || t.id)}</div>`;
  h += `<div class="wp-meta">${esc(t.id)} · ${esc(t.owner || '未指派')} · ${esc(statusMark)}</div>`;
  if (t.note) h += `<div class="wp-note">${esc(t.note)}</div>`;
  // 任务级模型（目标 ②「按任务自选模型」）—— 有就显示，让人知道这条活会走哪条路线
  if (t.provider_id || t.model) {
    const route = [t.provider_id, t.model].filter(Boolean).join(' / ');
    h += `<div class="wp-line">模型：${esc(route)}（任务级，压过员工花名册）</div>`;
  }
  if (t.problem) h += `<div class="wp-problem">⚠ ${esc(t.problem)}</div>`;

  if (refs.length) {
    const missing = refs.filter((r) => !r.exists).length;
    const unver = refs.filter((r) => r.exists && !r.verified).length;
    h += `<div class="wp-line${missing ? ' wp-warn' : ''}">产出 ${refs.length} 条`;
    if (missing) h += `（${missing} 个不存在）`;
    if (unver) h += `（${unver} 个证据未验证）`;
    h += '</div>';
  } else {
    h += '<div class="wp-line wp-warn">没有关联任何产出</div>';
  }
  if (t.unresolved_blockers && t.unresolved_blockers.length) {
    h += `<div class="wp-line wp-warn">被卡于 ${esc(t.unresolved_blockers.join(', '))}</div>`;
  }

  h += '<div class="wp-actions">';
  h += `<button class="wp-btn" data-wp-action="dispatch" data-wp-id="${esc(t.id)}">派活到当前终端</button>`;
  // 结构化跑一次：不走 TUI，跑完把金额记进账本（比"派活"多一份花费记录）
  h += `<button class="wp-btn" data-wp-action="run" data-wp-id="${esc(t.id)}">结构化跑一次</button>`;
  h += `<button class="wp-btn" data-wp-action="taskreview" data-wp-id="${esc(t.id)}">裁决</button>`;
  h += '</div></div>';
  return h;
}

/**
 * 工作台控制器：只做三件事 —— 取数、渲染、把点击转给已有命令。
 *
 * 面板**自己不做任何判断**，也不自己写 PTY：派活走 `agent-dispatch.js` 的
 * `sendTaskToSlot`（有意写入的唯一实现）。
 */
export class WorkPanelController {
  constructor({ ipc, tabManager, sendTaskToSlot, onNotice } = {}) {
    this.ipc = ipc;
    this.tm = tabManager;
    this.sendTaskToSlot = sendTaskToSlot;
    this.onNotice = typeof onNotice === 'function' ? onNotice : (m) => console.warn('[work]', m);
    this.el = null;
    this._busy = false;
    // 递增序号：用来丢弃"过期的响应"。渲染夹具实测到过这个问题 ——
    // 两次 refresh 交叠时，先发出的那个后返回，会把新内容覆盖成**旧项目**的数据。
    this._seq = 0;
  }

  init(container) {
    this.el = container;
    if (!this.el) return;
    this.el.innerHTML = '<div class="wp-empty">加载中…</div>';
    this.el.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-wp-action]');
      if (!btn) return;
      this._onAction(btn.dataset.wpAction, btn.dataset.wpId);
    });
    this.refresh();
  }

  /** 活跃终端的 cwd = "当前项目"。没有就不猜，如实说明。 */
  cwd() {
    const st = this.tm && this.tm.activeId ? this.tm.tabs.get(this.tm.activeId) : null;
    return st && !st.isError ? st.cwd : null;
  }

  async refresh() {
    if (!this.el) return;
    const cwd = this.cwd();
    if (!cwd) {
      this.el.innerHTML = '<div class="wp-empty">没有活跃终端 —— 工作台按项目组织，<br>先开一个终端再回来。</div>';
      return;
    }
    const seq = ++this._seq;
    this._busy = true;
    try {
      const [deliverables, tasks] = await Promise.all([
        this.ipc.deliverablesScan(cwd),
        this.ipc.taskList(cwd),
      ]);
      // 过期响应直接丢：期间可能切了终端/切了 tab，写进去就是"显示别的项目的数据"
      if (seq !== this._seq || this.cwd() !== cwd) return;
      this.el.innerHTML = buildWorkHtml({ deliverables, tasks });
      this._last = { deliverables, tasks };
    } catch (e) {
      if (seq !== this._seq) return;
      // 失败要看得见，不能留一个空面板
      this.el.innerHTML = `<div class="wp-empty wp-warn">读取出错：${esc(e && e.message ? e.message : e)}</div>`;
    } finally {
      if (seq === this._seq) this._busy = false;
    }
  }

  async _onAction(action, id) {
    const cwd = this.cwd();
    if (!cwd) return;
    try {
      if (action === 'review') {
        await this.ipc.deliverableReview(cwd, id);
      } else if (action === 'verify') {
        await this.ipc.deliverableVerify(cwd, id);
      } else if (action === 'taskreview') {
        await this.ipc.taskReview(cwd, id);
      } else if (action === 'run') {
        const board = this._last?.tasks;
        const task = board && (board.tasks || []).find((t) => t.id === id);
        if (!task) {
          this.onNotice('找不到这条任务，先刷新一下');
          return;
        }
        // 提示词由任务本身组成；跑完引擎会把金额写进 runs.jsonl
        const prompt = [
          `任务 ${task.id}：${task.title}`,
          task.note || '',
          '',
          '做完后：把交付写成产出记录，再把本任务状态改成 review 等我验收。',
        ]
          .filter((x) => x !== '')
          .join('\n');
        // 把 taskId 传给后端：任务上的模型覆盖会生效，且花费记到这条任务名下
        await this.ipc.engineRunShow(cwd, prompt, task.owner || null, task.id);
        await this.refresh();
      } else if (action === 'dispatch') {
        const board = this._last?.tasks;
        const task = board && (board.tasks || []).find((t) => t.id === id);
        if (!task) {
          this.onNotice('找不到这条任务，先刷新一下');
          return;
        }
        // 复用"有意写入 PTY"的唯一实现；不代按回车
        this.sendTaskToSlot({
          tabManager: this.tm,
          task,
          ptyWrite: this.ipc.ptyWrite,
          onNotice: this.onNotice,
        });
        return; // 派活不改数据，不必刷新
      } else {
        return;
      }
      await this.refresh(); // 动作改动了状态 → 刷新，别让人看旧数据
    } catch (e) {
      this.onNotice(`操作失败：${e && e.message ? e.message : e}`);
    }
  }
}
