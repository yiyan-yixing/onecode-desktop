// structured-view.js 的单测 —— 结构化槽位的排版口径。
//
// 与 work-panel 同一纪律：**判断与聚合在 Rust**。这里断言的全是"转述"——
// 合计金额/用量直接用返回值里的数字，前端绝不自己把 turns 加一遍。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { buildStructuredHtml, esc } from '../src/structured-view.js';

const VIEW = {
  id: 'slot-a',
  agent: 'dev',
  provider_id: 'glm',
  model: 'glm-4-plus',
  session_id: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
  engine_session_id: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
  created_at: '2026-10-14T00:00:00Z',
  total_cost_usd: 0.4,
  total_input_tokens: 300,
  total_output_tokens: 30,
  session_mismatch: false,
  turns: [
    { at: 't1', prompt: '写一段', reply: '写好了', ok: true, cost_usd: 0.1,
      input_tokens: 100, output_tokens: 10, duration_ms: 500, session_id: 's', error: '' },
    { at: 't2', prompt: '再改改', reply: '', ok: false, cost_usd: 0.3,
      input_tokens: 200, output_tokens: 20, duration_ms: 700, session_id: 's', error: '超时' },
  ],
};

describe('buildStructuredHtml（纯排版）', () => {
  test('每一轮都显示"谁说了什么 + 这一轮花了多少"', () => {
    const h = buildStructuredHtml(VIEW);
    assert.ok(h.includes('&gt; 写一段'));
    assert.ok(h.includes('写好了'));
    assert.ok(h.includes('$0.100000'), '这一轮的金额');
    assert.ok(h.includes('$0.300000'), '失败那轮的金额也要显示（花了就是花了）');
  });

  test('合计直接用 Rust 算好的数字，不自己加一遍', () => {
    const h = buildStructuredHtml(VIEW);
    assert.ok(h.includes('合计 $0.400000'), '用 view.total_cost_usd');
    assert.ok(h.includes('输入 300'), '用 view.total_input_tokens');
    // 造一个"故意算错"的合计：如果前端自己加，就会显示 0.4 而不是 9
    const weird = { ...VIEW, total_cost_usd: 9, total_input_tokens: 9, total_output_tokens: 9 };
    const h2 = buildStructuredHtml(weird);
    assert.ok(h2.includes('合计 $9.000000'), '前端必须原样转述：说明它没有自己重算');
  });

  test('失败的轮次要写出原因，不能装作没事', () => {
    const h = buildStructuredHtml(VIEW);
    assert.ok(h.includes('⚠ 失败：超时'));
  });

  test('不把 session-id 说成"会话" —— 实测它不接续上下文', () => {
    const h = buildStructuredHtml(VIEW);
    assert.ok(h.includes('标签 '), '要叫"标签"');
    assert.ok(h.includes('多轮上下文由本机回放'), '要说清多轮靠回放');
    assert.ok(h.includes('不接续'), '要明确说明它不接续（否则会被误解成已续上）');
  });

  test('会话不一致要显眼（标签与引擎回的不一致）', () => {
    const ok = buildStructuredHtml(VIEW);
    assert.ok(!ok.includes('不一致'));
    const bad = buildStructuredHtml({ ...VIEW, session_mismatch: true, engine_session_id: 'zzz' });
    assert.ok(bad.includes('会话 id 与我们请求的不一致'));
  });

  test('身份与路由要显示出来（这个槽位是谁、走哪条路线）', () => {
    const h = buildStructuredHtml(VIEW);
    assert.ok(h.includes('@dev'));
    assert.ok(h.includes('glm / glm-4-plus'));
  });

  test('没指派员工时如实说明', () => {
    const h = buildStructuredHtml({ ...VIEW, agent: '' });
    assert.ok(h.includes('(未指派员工)'));
  });

  test('空会话给出下一步，而不是空白', () => {
    const h = buildStructuredHtml({ ...VIEW, turns: [] });
    assert.ok(h.includes('还没有说话'));
    assert.ok(h.includes('data-ss-input'), '输入框要在');
  });

  test('协议特有的事实（ACP 的 stop/上下文）要显示出来', () => {
    const h = buildStructuredHtml({
      ...VIEW,
      turns: [{ ...VIEW.turns[0], extra_meta: 'ACP（mock） · stop=end_turn · 上下文 53000/200000' }],
    });
    assert.ok(h.includes('ACP（mock）'), '要显示协议与 agent');
    assert.ok(h.includes('stop=end_turn'), '要显示 stopReason');
    assert.ok(h.includes('53000/200000'), '上下文占用（不等于本轮 token，所以单独一行）');
  });

  test('extra_meta 里的 HTML 也会被转义', () => {
    const h = buildStructuredHtml({
      ...VIEW,
      turns: [{ ...VIEW.turns[0], extra_meta: '<b>x</b>' }],
    });
    assert.ok(!h.includes('<b>x</b>'));
    assert.ok(h.includes('&lt;b&gt;'));
  });

  test('没有视图时不炸', () => {
    assert.equal(typeof buildStructuredHtml(null), 'string');
    assert.equal(typeof buildStructuredHtml({}), 'string');
    assert.equal(typeof buildStructuredHtml(), 'string');
  });

  test('回复里的 HTML 会被转义（引擎产出属不可信输入）', () => {
    const h = buildStructuredHtml({
      ...VIEW,
      turns: [{ ...VIEW.turns[0], reply: '<img src=x onerror=alert(1)>' }],
    });
    assert.ok(!h.includes('<img'), '不能把标签原样塞进 DOM');
    assert.ok(h.includes('&lt;img'));
  });

  test('缺字段不炸', () => {
    const h = buildStructuredHtml({ id: 'x', turns: [{ prompt: 'a' }, {}] });
    assert.ok(h.includes('a'));
  });

  test('esc 覆盖五种字符', () => {
    assert.equal(esc(`&<>"'`), '&amp;&lt;&gt;&quot;&#39;');
    assert.equal(esc(null), '');
  });
});
