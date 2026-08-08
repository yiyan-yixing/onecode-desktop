// M1: 多供应商管理 + 手动切换 — 前端控制器
//
// 职责：
// - 状态栏「模型芯片」（C1）：显示当前供应商+型号，点击弹出 F2 选择器
// - F2 / /model：选择供应商 → 若有活跃会话先确认（G1）→ 调 providers_switch
// - F3：供应商管理弹窗（增删改测 + 两档预设 + 自定义四要素）
// - 监听 provider-switched / providers-changed，切换后刷新芯片 + 重启会话
//
// 事件流（手动切换）：
//   chip 点击 → openSwitcher() → selectProvider(id) → (活跃会话?) 确认
//   → ipc.providersSwitch(id) → 后端写 desktop.json + providers.json
//   → emit provider-switched → 本模块收到事件 → refreshChip() + restartSessions()

import * as ipc from './ipc-bridge.js';

const ICON_CHIP = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:11px;height:11px"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M3.6 12.4L5 11M11 5l1.4-1.4"/></svg>';
const ICON_PLUS = '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" style="width:11px;height:11px"><line x1="7" y1="2" x2="7" y2="12"/><line x1="2" y1="7" x2="12" y2="7"/></svg>';
const ICON_EDIT = '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:11px;height:11px"><path d="M9.5 1.5l3 3L5 12H2V9z"/><line x1="8" y1="3" x2="11" y2="6"/></svg>';
const ICON_TRASH = '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:11px;height:11px"><path d="M2 4h10M5 4V2h4v2M4 4l.5 8h5l.5-8"/><line x1="6" y1="7" x2="6" y2="10"/><line x1="8" y1="7" x2="8" y2="10"/></svg>';
const ICON_TEST = '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="width:11px;height:11px"><path d="M1.5 5.5h4M3.5 3.5v4M8 1.5v6.5M8 8c-1.5 0-2.5 1-2.5 2.5S6.5 13 8 13s2.5-1 2.5-2.5S9.5 8 8 8z"/></svg>';

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** 简易 toast（复用现有 .health-warn 类似的浮层，直接 append 到 body） */
let _toastStack = [];
function toast(msg, kind = 'ok', ms = 3200) {
  const el = document.createElement('div');
  el.className = `ps-toast ps-toast-${kind}`;
  el.textContent = msg;
  document.body.appendChild(el);
  requestAnimationFrame(() => el.classList.add('on'));
  _toastStack.push(el);
  if (_toastStack.length > 3) {
    const old = _toastStack.shift();
    old?.remove();
  }
  setTimeout(() => {
    el.classList.remove('on');
    el.classList.add('dismissing');
    setTimeout(() => el.remove(), 220);
    const i = _toastStack.indexOf(el);
    if (i >= 0) _toastStack.splice(i, 1);
  }, ms);
}

export class ModelSwitchController {
  constructor() {
    this.tm = null;
    this.catalog = null; // ProviderCatalog（providers_list 快照）
    this._switching = false; // 防抖：切换进行中禁止重复
    this._overlay = null; // 当前打开的 overlay
    this._unsubs = [];
  }

  // ── 生命周期 ──────────────────────────────────────────────
  init(tm) {
    this.tm = tm;
    const chip = document.getElementById('ribbonProviderChip');
    if (chip) {
      chip.addEventListener('click', () => this.openSwitcher());
      chip.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); this.openSwitcher(); }
      });
    }
    // 事件订阅
    this._unsubs.push(ipc.onProviderSwitched((payload) => {
      // P2-1: 保持 _switching 直到会话重启完成，避免用户并发第二次切换导致同一 tab 重复重启
      (async () => {
        try {
          await this.refresh();
          if (payload && payload.to) {
            const name = this._providerName(payload.to) || payload.to;
            toast(`已切换到 ${name}`, 'ok');
          }
          await this.restartSessions();
        } catch (e) {
          console.warn('[model-switch] after-switch failed:', e);
        } finally {
          this._switching = false;
          this.renderChip();
        }
      })();
    }));
    this._unsubs.push(ipc.onProvidersChanged(() => {
      this.refresh();
    }));
    // 首次渲染
    this.refresh();
  }

  destroy() {
    this._unsubs.forEach((u) => { try { u?.(); } catch {} });
    this._unsubs = [];
    this._closeOverlay();
  }

  /** 拉取目录快照 + 渲染芯片 */
  async refresh() {
    try {
      this.catalog = await ipc.providersList();
    } catch (e) {
      console.warn('[model-switch] providers_list failed:', e);
      this.catalog = null;
    }
    this.renderChip();
  }

  // ── 状态栏芯片（C1）───────────────────────────────────────
  renderChip() {
    const chip = document.getElementById('ribbonProviderChip');
    if (!chip) return;
    const cat = this.catalog;
    const activeId = cat?.active_provider_id;
    const active = activeId ? (cat?.providers || []).find((p) => p.id === activeId) : null;
    let text;
    if (this._switching) {
      text = '⟳ 切换中…';
      chip.classList.add('switching');
    } else if (active) {
      text = `${active.name} ▾`;
      chip.classList.remove('switching', 'unconfigured');
    } else if (cat && cat.providers.length > 0) {
      // 目录有供应商但未激活 → 显示第一个供应商名，引导切换
      text = `${cat.providers[0].name} ▾`;
      chip.classList.remove('switching', 'unconfigured');
    } else {
      // 无目录 → 回退显示配置 model；再没有则显示引导色
      this._fallbackModel().then((model) => {
        if (model) {
          chip.textContent = `${model} ▾`;
          chip.classList.remove('switching', 'unconfigured');
        } else {
          chip.innerHTML = `${ICON_CHIP}<span>配置模型 ▾</span>`;
          chip.classList.add('unconfigured');
        }
        chip.title = '切换模型（F2）';
      });
      return; // 异步回填，避免竞态
    }
    chip.textContent = text;
    chip.title = '切换模型（F2）';
  }

  /** 无 providers.json 时回退读取 desktop.json 的 model */
  async _fallbackModel() {
    try {
      const cfg = await ipc.loadConfig();
      return (cfg && cfg.model) || '';
    } catch {
      return '';
    }
  }

  _providerName(id) {
    return (this.catalog?.providers || []).find((p) => p.id === id)?.name || id;
  }

  // ── F2 选择器 ──────────────────────────────────────────────
  async openSwitcher() {
    if (!this.catalog) await this.refresh();
    const cat = this.catalog || { providers: [] };
    const activeId = cat.active_provider_id;
    const providers = cat.providers || [];

    const rows = providers.map((p) => {
      const isActive = p.id === activeId;
      const sub = p.model ? `${esc(p.model)} · ${esc(p.base_url)}` : esc(p.base_url);
      return (
        `<div class="ps-item${isActive ? ' active' : ''}" data-id="${esc(p.id)}">` +
        `<span class="ps-item-check">${isActive ? '●' : ''}</span>` +
        `<span class="ps-item-main">${esc(p.name)}</span>` +
        `<span class="ps-item-sub">${sub}</span>` +
        `<span class="ps-item-kbd">${isActive ? '当前' : ''}</span>` +
        `</div>`
      );
    }).join('') ||
      `<div class="ps-item" style="cursor:default"><span class="ps-item-main" style="color:var(--tx-warm3)">尚无供应商，先新增一个</span></div>`;

    const html =
      `<div class="np-dialog ps-dialog">` +
        `<div class="np-header"><div class="np-title">切换模型</div>` +
        `<button class="np-close ps-close" title="关闭">✕</button></div>` +
        `<div class="np-body">` +
          `<div class="ps-list">${rows}</div>` +
          `<div class="ps-footer">` +
            `<button class="ps-btn ps-btn-ghost ps-manage" type="button">管理供应商…</button>` +
            `<span class="ps-hint">Enter 选择 · Esc 关闭</span>` +
          `</div>` +
        `</div>` +
      `</div>`;

    this._openOverlay(html, (overlay) => {
      const list = overlay.querySelector('.ps-list');
      let selIdx = 0;
      const items = Array.from(list.querySelectorAll('.ps-item[data-id]'));
      const highlight = () => {
        items.forEach((el, i) => el.classList.toggle('sel', i === selIdx));
        const sel = items[selIdx];
        if (sel) sel.scrollIntoView({ block: 'nearest' });
      };
      // 默认选中当前项
      const cur = items.findIndex((el) => el.classList.contains('active'));
      selIdx = cur >= 0 ? cur : 0;
      highlight();

      const choose = () => {
        const el = items[selIdx];
        if (!el) return;
        const id = el.dataset.id;
        if (el.classList.contains('active')) { this._closeOverlay(); return; }
        this._closeOverlay();
        this.selectProvider(id);
      };

      list.addEventListener('click', (e) => {
        const row = e.target.closest('.ps-item[data-id]');
        if (!row) return;
        const id = row.dataset.id;
        if (row.classList.contains('active')) { this._closeOverlay(); return; }
        this._closeOverlay();
        this.selectProvider(id);
      });
      overlay.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowDown') { e.preventDefault(); selIdx = Math.min(selIdx + 1, items.length - 1); highlight(); }
        else if (e.key === 'ArrowUp') { e.preventDefault(); selIdx = Math.max(selIdx - 1, 0); highlight(); }
        else if (e.key === 'Enter') { e.preventDefault(); choose(); }
        else if (e.key === 'Escape') { this._closeOverlay(); }
      });
      overlay.querySelector('.ps-close').addEventListener('click', () => this._closeOverlay());
      overlay.querySelector('.ps-manage').addEventListener('click', () => {
        this._closeOverlay();
        this.openManagement();
      });
    });
  }

  /** 选择供应商：有活跃会话先确认（G1），否则直接切 */
  async selectProvider(providerId) {
    const running = this._runningSessions();
    if (running.length === 0) {
      await this.performSwitch(providerId);
      return;
    }
    const name = this._providerName(providerId) || providerId;
    const html =
      `<div class="np-dialog ps-dialog ps-confirm">` +
        `<div class="np-header"><div class="np-title">切换模型</div>` +
        `<button class="np-close ps-close" title="关闭">✕</button></div>` +
        `<div class="np-body">` +
          `<p class="ps-confirm-text">将切到 <b>${esc(name)}</b></p>` +
          `<p class="ps-confirm-sub">当前有 ${running.length} 个活跃会话，切换会重启它们。</p>` +
          `<div class="ps-confirm-actions">` +
            `<button class="ps-btn ps-btn-ghost ps-cancel" type="button">取消</button>` +
            `<button class="ps-btn ps-btn-primary ps-go" type="button">切换</button>` +
          `</div>` +
        `</div>` +
      `</div>`;
    this._openOverlay(html, (overlay) => {
      overlay.querySelector('.ps-cancel').addEventListener('click', () => this._closeOverlay());
      overlay.querySelector('.ps-close').addEventListener('click', () => this._closeOverlay());
      overlay.querySelector('.ps-go').addEventListener('click', async () => {
        this._closeOverlay();
        await this.performSwitch(providerId);
      });
      overlay.querySelector('.ps-go').focus();
    });
  }

  async performSwitch(providerId) {
    if (this._switching) return; // 防抖
    this._switching = true;
    this.renderChip();
    try {
      await ipc.providersSwitch(providerId);
      // 事件驱动后续（provider-switched），这里只处理命令自身失败
    } catch (e) {
      this._switching = false;
      this.renderChip();
      toast(String(e && e.message ? e.message : e), 'err');
    }
  }

  /** 切换成功后重启所有活跃会话（pty_refresh_env → pty_restart） */
  async restartSessions() {
    if (!this.tm) return;
    for (const [id, st] of this.tm.tabs) {
      if (st.isError) continue;
      if (st.status !== 'running' && st.status !== 'restarting') continue;
      try {
        await ipc.ptyRefreshEnv(id);
      } catch (e) {
        console.warn(`[model-switch] pty_refresh_env failed ${id}:`, e);
      }
      try {
        await this.tm.restartTab(id);
      } catch (e) {
        console.warn(`[model-switch] restartTab failed ${id}:`, e);
      }
    }
  }

  _runningSessions() {
    if (!this.tm) return [];
    const out = [];
    for (const [id, st] of this.tm.tabs) {
      if (st.isError) continue;
      if (st.status === 'running' || st.status === 'restarting') out.push(id);
    }
    return out;
  }

  // ── F3 供应商管理 ──────────────────────────────────────────
  async openManagement() {
    if (!this.catalog) await this.refresh();
    const cat = this.catalog || { providers: [], failover_queue: [], auto_failover_enabled: true, failover_params: {} };
    const activeId = cat.active_provider_id;

    const rows = (cat.providers || []).map((p) => {
      const isActive = p.id === activeId;
      const statusDot = isActive
        ? '<span class="ps-status-dot using" title="使用中"></span><span class="ps-status-text">使用中</span>'
        : '<span class="ps-status-dot ok" title="可用"></span><span class="ps-status-text">可用</span>';
      return (
        `<div class="ps-provider-row" data-id="${esc(p.id)}">` +
          `<div class="ps-provider-info">` +
            `<div class="ps-provider-name">${esc(p.name)} ${isActive ? '<span class="ps-now">当前</span>' : ''}</div>` +
            `<div class="ps-provider-sub">${esc(p.model)} · ${esc(p.base_url)}</div>` +
          `</div>` +
          `<div class="ps-provider-status">${statusDot}</div>` +
          `<div class="ps-provider-actions">` +
            `<button class="ps-iconbtn ps-test" title="测试连接">${ICON_TEST}</button>` +
            `<button class="ps-iconbtn ps-edit" title="编辑">${ICON_EDIT}</button>` +
            `<button class="ps-iconbtn ps-del" title="删除">${ICON_TRASH}</button>` +
          `</div>` +
        `</div>`
      );
    }).join('') || '<div class="ps-provider-empty">尚无供应商，点击右上角「新增」添加。</div>';

    const failoverBadge = cat.auto_failover_enabled ? '开' : '关';
    const queueText = (cat.failover_queue || []).join(' → ') || (activeId ? activeId : '（空）');

    const html =
      `<div class="np-dialog ps-dialog ps-manage">` +
        `<div class="np-header"><div class="np-title">模型供应商</div>` +
        `<button class="np-close ps-close" title="关闭">✕</button></div>` +
        `<div class="np-body">` +
          `<div class="ps-manage-toolbar">` +
            `<button class="ps-btn ps-btn-primary ps-add" type="button">${ICON_PLUS} 新增</button>` +
            `<span class="ps-manage-hint">状态图例：<span class="ps-status-dot using"></span>使用中 <span class="ps-status-dot ok"></span>可用</span>` +
          `</div>` +
          `<div class="ps-provider-list">${rows}</div>` +
          `<div class="ps-failover-box">` +
            `<div class="ps-failover-row"><span>自动切换</span><span class="ps-failover-val">${failoverBadge === '开' ? '● 开' : '○ 关'}</span></div>` +
            `<div class="ps-failover-row"><span>备用顺序</span><span class="ps-failover-val">${esc(queueText)}</span></div>` +
            `<div class="ps-failover-row"><span>失败判定</span><span class="ps-failover-val">连续 ${esc(String(cat.failover_params?.consecutive_failures ?? 4))} 次 / 超时 ${esc(String(cat.failover_params?.timeout_seconds ?? 60))}s</span></div>` +
          `</div>` +
        `</div>` +
      `</div>`;

    this._openOverlay(html, (overlay) => {
      overlay.querySelector('.ps-close').addEventListener('click', () => this._closeOverlay());
      overlay.querySelector('.ps-add').addEventListener('click', () => {
        overlay.remove();
        this._openEditForm(null);
      });
      overlay.querySelectorAll('.ps-test').forEach((btn) => {
        btn.addEventListener('click', (e) => {
          e.stopPropagation();
          const row = btn.closest('.ps-provider-row');
          this.testProvider(row.dataset.id, btn);
        });
      });
      overlay.querySelectorAll('.ps-edit').forEach((btn) => {
        btn.addEventListener('click', (e) => {
          e.stopPropagation();
          const row = btn.closest('.ps-provider-row');
          overlay.remove();
          this._openEditForm(row.dataset.id);
        });
      });
      overlay.querySelectorAll('.ps-del').forEach((btn) => {
        btn.addEventListener('click', async (e) => {
          e.stopPropagation();
          const row = btn.closest('.ps-provider-row');
          const id = row.dataset.id;
          const name = (this.catalog?.providers || []).find((p) => p.id === id)?.name || id;
          if (!window.confirm(`删除供应商「${name}」？`)) return;
          try {
            await ipc.providersDelete(id);
            await this.refresh();
            toast(`已删除 ${name}`, 'ok');
            // 刷新管理弹窗
            if (this._overlay) { this._overlay.remove(); this._overlay = null; }
            this.openManagement();
          } catch (err) {
            toast(String(err.message || err), 'err');
          }
        });
      });
    });
  }

  async testProvider(id, btn) {
    const orig = btn.innerHTML;
    btn.disabled = true;
    btn.textContent = '…';
    try {
      const r = await ipc.providersTest(id);
      if (r.ok) {
        btn.innerHTML = '✓';
        btn.classList.add('ok');
        toast(`连接正常 · ${r.latency_ms}ms`, 'ok');
      } else {
        btn.innerHTML = '✗';
        btn.classList.add('err');
        toast(`连接失败：${r.error || '未知'}`, 'err');
      }
    } catch (err) {
      btn.innerHTML = '✗';
      btn.classList.add('err');
      toast(String(err.message || err), 'err');
    }
    setTimeout(() => {
      btn.disabled = false;
      btn.innerHTML = orig;
      btn.classList.remove('ok', 'err');
    }, 2500);
  }

  /** 新增（provider=null）或编辑（provider=id）表单 */
  async _openEditForm(providerId) {
    const presets = await ipc.providersPresets().catch(() => []);
    let editing = null;
    if (providerId && this.catalog) {
      editing = (this.catalog.providers || []).find((p) => p.id === providerId);
    }
    const isEdit = !!editing;

    const presetOptions = presets.map((pr) =>
      `<option value="${esc(pr.id)}">${esc(pr.name)}（${esc(pr.model)}）</option>`
    ).join('');

    const html =
      `<div class="np-dialog ps-dialog ps-form">` +
        `<div class="np-header"><div class="np-title">${isEdit ? '编辑供应商' : '新增供应商'}</div>` +
        `<button class="np-close ps-close" title="关闭">✕</button></div>` +
        `<div class="np-body">` +
          `${isEdit ? '' :
            `<div class="ps-field"><label>从预设选择（可选，自动带出 URL/型号）</label>` +
            `<select class="ps-input ps-preset"><option value="">— 自定义 —</option>${presetOptions}</select></div>`}` +
          `<div class="ps-field"><label>供应商名称</label>` +
            `<input class="ps-input ps-name" type="text" value="${esc(editing?.name || '')}" placeholder="如 DeepSeek-V4-Flash" autocomplete="off"></div>` +
          `<div class="ps-field"><label>Base URL</label>` +
            `<input class="ps-input ps-url" type="text" value="${esc(editing?.base_url || '')}" placeholder="https://api.deepseek.com" autocomplete="off"></div>` +
          `<div class="ps-field"><label>API Key${isEdit ? '（留空保持不变）' : ''}</label>` +
            `<input class="ps-input ps-key" type="password" value="" placeholder="${isEdit ? '不修改请留空' : 'sk-…'}" autocomplete="off"></div>` +
          `<div class="ps-field"><label>型号</label>` +
            `<input class="ps-input ps-model" type="text" value="${esc(editing?.model || '')}" placeholder="deepseek-v4-flash" autocomplete="off"></div>` +
          `<div class="ps-form-actions">` +
            `<button class="ps-btn ps-btn-ghost ps-test-btn" type="button">测试连接</button>` +
            `<div class="ps-form-right">` +
              `<button class="ps-btn ps-btn-ghost ps-cancel" type="button">取消</button>` +
              `<button class="ps-btn ps-btn-primary ps-save" type="button">${isEdit ? '保存' : '添加'}</button>` +
            `</div>` +
          `</div>` +
        `</div>` +
      `</div>`;

    this._openOverlay(html, (overlay) => {
      const nameEl = overlay.querySelector('.ps-name');
      const urlEl = overlay.querySelector('.ps-url');
      const keyEl = overlay.querySelector('.ps-key');
      const modelEl = overlay.querySelector('.ps-model');
      const presetEl = overlay.querySelector('.ps-preset');
      const saveBtn = overlay.querySelector('.ps-save');

      if (presetEl) {
        presetEl.addEventListener('change', () => {
          const pr = presets.find((x) => x.id === presetEl.value);
          if (!pr) return;
          urlEl.value = pr.base_url;
          modelEl.value = pr.model;
          if (!nameEl.value || nameEl.value === urlEl.dataset.prevName) {
            nameEl.value = pr.name;
            urlEl.dataset.prevName = pr.name;
          }
        });
      }

      const close = () => this._closeOverlay();
      overlay.querySelector('.ps-close').addEventListener('click', close);
      overlay.querySelector('.ps-cancel').addEventListener('click', close);

      overlay.querySelector('.ps-test-btn').addEventListener('click', async () => {
        const payload = {
          name: nameEl.value.trim(),
          base_url: urlEl.value.trim(),
          api_key: keyEl.value.trim(),
          model: modelEl.value.trim(),
        };
        if (!payload.base_url || !payload.api_key) { toast('请先填 Base URL 和 API Key', 'err'); return; }
        // 借用 test_connection：临时构造一个 provider 对象走后端 providers_test 不便，
        // 这里直接用已保存逻辑——若当前编辑的是已有供应商则测它，否则提示先保存。
        if (isEdit && editing) {
          const btn = overlay.querySelector('.ps-test-btn');
          btn.textContent = '…';
          btn.disabled = true;
          try {
            const r = await ipc.providersTest(editing.id);
            if (r.ok) toast(`连接正常 · ${r.latency_ms}ms`, 'ok');
            else toast(`连接失败：${r.error || '未知'}`, 'err');
          } catch (err) {
            toast(String(err.message || err), 'err');
          }
          btn.disabled = false;
          btn.textContent = '测试连接';
        } else {
          toast('新增供应商请先「添加」保存后再测试', 'info');
        }
      });

      saveBtn.addEventListener('click', async () => {
        const payload = {
          name: nameEl.value.trim(),
          base_url: urlEl.value.trim(),
          api_key: keyEl.value.trim(),
          model: modelEl.value.trim(),
        };
        if (!payload.name || !payload.base_url || !payload.model) {
          toast('名称 / Base URL / 型号不能为空', 'err');
          return;
        }
        try {
          if (isEdit && editing) {
            const updates = {};
            if (payload.name !== editing.name) updates.name = payload.name;
            if (payload.base_url !== editing.base_url) updates.base_url = payload.base_url;
            // 仅当用户填了新 key 才更新（留空 = 不修改，避免把旧 key 回传）
            if (payload.api_key && payload.api_key !== editing.api_key) updates.api_key = payload.api_key;
            if (payload.model !== editing.model) updates.model = payload.model;
            await ipc.providersUpdate(editing.id, updates);
            toast('已保存', 'ok');
          } else {
            await ipc.providersAdd(payload);
            toast('已添加，可用 F2 切换', 'ok');
          }
          this._closeOverlay();
          await this.refresh();
          this.openManagement();
        } catch (err) {
          toast(String(err.message || err), 'err');
        }
      });

      // Enter 保存
      overlay.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') close();
        if (e.key === 'Enter' && e.target.tagName === 'INPUT') {
          e.preventDefault();
          saveBtn.click();
        }
      });

      nameEl.focus();
      if (nameEl.value) nameEl.select();
    });
  }

  // ── Overlay 管理 ──────────────────────────────────────────
  _openOverlay(html, onReady) {
    this._closeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'np-overlay';
    overlay.innerHTML = html;
    document.body.appendChild(overlay);
    this._overlay = overlay;
    requestAnimationFrame(() => overlay.classList.add('on'));
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) this._closeOverlay();
    });
    onReady?.(overlay);
  }

  _closeOverlay() {
    const ov = this._overlay;
    if (!ov) return;
    this._overlay = null;
    ov.classList.remove('on');
    ov.classList.add('dismissing');
    setTimeout(() => ov.remove(), 200);
    // 关闭后焦点回到终端
    if (this.tm && this.tm.activeId) {
      const st = this.tm.tabs.get(this.tm.activeId);
      if (st && st.term) setTimeout(() => st.term.focus(), 220);
    }
  }
}
