// work-panel.js 的单测 —— 目标 ④⑤ 的日常入口。
//
// 这里钉住的是**排版口径**，以及一条比排版更重要的纪律：
// 面板**不许自己判断**"验过没有/完成没有/文件在不在" —— 那些必须来自 Rust。
// 所以测试用的是 Rust 返回值的形状，断言面板只做**转述**。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { buildWorkHtml, esc } from '../src/work-panel.js';

const PENDING = {
  id: 'd1',
  title: '公众号排版成品',
  summary: '生成了 out.html',
  status: 'draft',
  files: [
    { path: 'out.html', exists: true, bytes: 1200 },
    { path: 'ghost.png', exists: false, bytes: null },
  ],
  evidence: [
    { kind: 'exec', grade: 'machine-runnable', detail: '可机器验证', command: 'node check.js' },
    { kind: 'self-report', grade: 'not-evidence', detail: '不是证据', command: null },
  ],
  problem: '声明的文件不存在：ghost.png',
  verification: null,
};

const VERIFIED = {
  ...PENDING,
  id: 'd2',
  title: '已验证的产出',
  problem: null,
  files: [{ path: 'ok.html', exists: true, bytes: 10 }],
  verification: {
    at: '2026-10-14T00:00:00Z',
    results: [
      { command: 'a', exit_code: 0, timed_out: false, duration_ms: 5, output_tail: '' },
      { command: 'b', exit_code: 1, timed_out: false, duration_ms: 5, output_tail: 'boom' },
    ],
  },
};

const ACCEPTED = { ...VERIFIED, id: 'd3', title: '已接受', status: 'accepted' };

const TASKS = {
  tasks: [
    {
      id: 't-doing',
      title: '写一篇稿',
      owner: 'dev',
      status: 'doing',
      status_source: 'declared',
      note: '进行中',
      problem: null,
      deliverable_refs: [{ id: 'd1', exists: true, status: 'draft', verified: null }],
      unresolved_blockers: [],
    },
    {
      id: 't-selfdone',
      title: '自报完成的',
      owner: '',
      status: 'done',
      status_source: 'declared',
      note: '',
      problem: '状态 done 来自任务文件本身（未经人裁决）',
      deliverable_refs: [],
      unresolved_blockers: [],
    },
    {
      id: 't-human-done',
      title: '人裁决完成的',
      owner: 'dev',
      status: 'done',
      status_source: 'human',
      note: '',
      problem: null,
      deliverable_refs: [],
      unresolved_blockers: [],
    },
  ],
};

describe('buildWorkHtml（纯排版）', () => {
  test('待验收队列只包含未处理的产出', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [PENDING, VERIFIED, ACCEPTED] },
      tasks: { tasks: [] },
    });
    assert.ok(html.includes('公众号排版成品'), '待处理要列出');
    assert.ok(html.includes('已验证的产出'), '未接受的一律进队列（包括已验过但没裁决的）');
    assert.ok(!html.includes('>已接受<'), '已接受的不该出现在待验收队列');
    assert.ok(html.includes('待验收 2'));
  });

  test('"声明了证据"与"证据验过了"必须能区分', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [PENDING, VERIFIED] },
      tasks: { tasks: [] },
    });
    assert.ok(html.includes('证据 <b>未验证</b>'), '没验过要明确写出来');
    assert.ok(!html.includes('**'), 'HTML 里绝不能出现 markdown 星号（渲染实测发现过）');
    assert.ok(html.includes('实测：1/2 通过'), '验过要报实测通过数');
  });

  test('不存在的文件与 problem 都要显眼', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [PENDING] }, tasks: { tasks: [] } });
    assert.ok(html.includes('文件 1/2 存在'));
    assert.ok(html.includes('1 个不存在'));
    assert.ok(html.includes('⚠ 声明的文件不存在：ghost.png'));
  });

  test('证据分级计数是转述 Rust 的 grade，不是自己判断', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [PENDING] }, tasks: { tasks: [] } });
    assert.ok(html.includes('可验证 1'));
    assert.ok(html.includes('不算证据 1'));
  });

  test('已完成不进未完成列表；但 agent 自报完成的必须另立一组可见', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [] }, tasks: TASKS });
    assert.ok(html.includes('写一篇稿'));
    assert.ok(html.includes('未完成任务 1'));
    assert.ok(!html.includes('人裁决完成的'), '人裁决过的完成不该再要人处理');

    // ★ 这条是**渲染实测**逼出来的：早先只按 status 过滤，自报 done 的任务
    // **整个从面板消失** —— 于是"它自称完成了"这件事恰恰没人看得见，
    // 而它是最该被看见的（④「模型自述不算证据」在 ⑤ 上的对应物）。
    assert.ok(html.includes('agent 自报完成'), '要另立一组');
    assert.ok(html.includes('自报完成的'), '自报的任务本身要列出来让人裁决');
    assert.ok(html.includes('自报完成待裁决 1'), '摘要里要有计数');
    assert.ok(html.includes('不是人裁决的'), '要说清它为什么还不算完成');
    // 次序：它与审查队列同属"等人裁决"，必须排在长列表**之前**，
    // 否则面板一长就没人往下滚、这条最该看见的信号就被埋了。
    assert.ok(
      html.indexOf('agent 自报完成') < html.indexOf('产出 · 待验收'),
      '"自报完成"要排在待验收队列之前'
    );
  });

  test('任务的状态来源要标出来，并且在有 problem 时告警', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [] }, tasks: TASKS });
    assert.ok(html.includes('doing（文件声明）'), '来源来自 Rust 的 status_source');
    // 自报完成的任务即使不在未完成列表，它的 problem 也必须能被看见 —— 用一条在列表里的任务验证格式
    const withProblem = {
      tasks: [
        {
          ...TASKS.tasks[0],
          problem: '状态 done 来自任务文件本身（未经人裁决）',
        },
      ],
    };
    const h2 = buildWorkHtml({ deliverables: { deliverables: [] }, tasks: withProblem });
    assert.ok(h2.includes('⚠ 状态 done 来自任务文件本身'));
  });

  test('任务没有关联产出、被卡住，都要说出来', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [] },
      tasks: {
        tasks: [
          { ...TASKS.tasks[0], deliverable_refs: [], unresolved_blockers: ['t-other'] },
        ],
      },
    });
    assert.ok(html.includes('没有关联任何产出'));
    assert.ok(html.includes('被卡于 t-other'));
  });

  test('空项目给出下一步怎么做，而不是空白', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [] }, tasks: { tasks: [] } });
    assert.ok(html.includes('.onecode/deliverables/'), '要说清产出记录该写哪');
    assert.ok(html.includes('还没有任务记录'));
  });

  test('全部处理完时明确说处理完了', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [ACCEPTED] },
      tasks: { tasks: [TASKS.tasks[2]] },
    });
    assert.ok(html.includes('1 条产出都已处理完'));
    assert.ok(html.includes('1 条任务都已完成'));
  });

  test('动作按钮带出 id，交给控制器转发（面板自己不做任何事）', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [PENDING] }, tasks: TASKS });
    assert.ok(html.includes('data-wp-action="review" data-wp-id="d1"'));
    assert.ok(html.includes('data-wp-action="verify" data-wp-id="d1"'));
    assert.ok(html.includes('data-wp-action="taskreview" data-wp-id="t-doing"'));
    assert.ok(html.includes('data-wp-action="dispatch" data-wp-id="t-doing"'));
  });

  test('自报完成的任务带裁决按钮（能直接处理，不用去翻命令面板）', () => {
    const html = buildWorkHtml({ deliverables: { deliverables: [] }, tasks: TASKS });
    assert.ok(html.includes('data-wp-action="taskreview" data-wp-id="t-selfdone"'));
  });

  test('没有人裁决过、也没有自报完成的，就不出现那一组', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [] },
      tasks: { tasks: [TASKS.tasks[0]] },
    });
    assert.ok(!html.includes('agent 自报完成'));
    assert.ok(!html.includes('自报完成待裁决'));
  });

  test('任务级模型要显示出来（目标 ② 按任务自选模型）', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [] },
      tasks: {
        tasks: [
          { ...TASKS.tasks[0], provider_id: 'glm', model: 'glm-4-plus' },
          { ...TASKS.tasks[0], id: 't-nomodel', provider_id: '', model: '' },
        ],
      },
    });
    assert.ok(html.includes('模型：glm / glm-4-plus'), '有覆盖就要显示');
    assert.ok(html.includes('压过员工花名册'), '要说清优先级');
    // 没写的任务不该出现这一行
    assert.equal(html.match(/模型：/g).length, 1, '只有设了的才显示');
  });

  test('缺字段不炸（agent 写的记录什么都可能缺）', () => {
    const html = buildWorkHtml({
      deliverables: { deliverables: [{ id: 'bare' }] },
      tasks: { tasks: [{ id: 'bare-task' }] },
    });
    assert.ok(html.includes('bare'));
    assert.ok(html.includes('bare-task'));
    assert.ok(html.includes('未指派'));
  });

  test('没有参数也不炸', () => {
    assert.equal(typeof buildWorkHtml(), 'string');
    assert.equal(typeof buildWorkHtml({}), 'string');
  });

  test('标题里的 HTML 会被转义（标题来自 agent 写的文件，属不可信输入）', () => {
    const html = buildWorkHtml({
      deliverables: {
        deliverables: [{ id: 'x', title: '<img src=x onerror=alert(1)>', status: 'draft', files: [], evidence: [] }],
      },
      tasks: { tasks: [] },
    });
    assert.ok(!html.includes('<img'), '不能把标签原样塞进 DOM');
    assert.ok(html.includes('&lt;img'));
  });

  test('esc 覆盖五种字符', () => {
    assert.equal(esc(`&<>"'`), '&amp;&lt;&gt;&quot;&#39;');
    assert.equal(esc(null), '');
    assert.equal(esc(undefined), '');
  });
});
