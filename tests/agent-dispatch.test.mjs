// agent-dispatch.js 的单测 —— 侧栏 / 光球 / 命令面板三处共用的**唯一**实现。
//
// 这里钉住的是语义边界，而不是某个界面的行为：
//   点击 = 派发（新建 Slot） ｜  ⌥/Ctrl/Cmd + 点击 = 点名（插入 @id）
// 两条路都必须**如实提示**失败，不能悄悄退回另一种语义。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import {
  dispatchAgent,
  insertAgentMention,
  isMentionModifier,
  sendTaskToSlot,
  writeToActiveSlot,
} from '../src/agent-dispatch.js';

const AGENTS = [
  { id: 'arch', name: 'Architect', icon: '', scope: 'project' },
  { id: 'dev', name: 'DevOps', icon: '🚀', scope: 'project' },
];

function makeTm({ active = true, isError = false, createTab } = {}) {
  const created = [];
  const tabs = new Map();
  if (active) {
    tabs.set('t1', {
      id: 't1',
      isError,
      projectId: 'proj-1',
      cwd: '/tmp/p1',
      backend: 'claude-code',
      providerId: 'deepseek',
      model: 'deepseek-chat',
      term: { focus() {} },
      mention: null,
    });
  }
  return {
    created,
    tm: {
      activeId: active ? 't1' : null,
      tabs,
      createTab: createTab || (async (opts) => { created.push(opts); return 'new'; }),
    },
  };
}

describe('dispatchAgent — 点击 = 派发新员工 Slot', () => {
  test('创建 Slot 并带上 agent 身份，不写 PTY', async () => {
    const { tm, created } = makeTm();
    const ok = await dispatchAgent({ tabManager: tm, agentId: 'arch', agents: AGENTS });
    assert.equal(ok, true);
    assert.equal(created.length, 1);
    assert.equal(created[0].agent, 'arch');
  });

  test('继承当前项目的 projectId / cwd / backend / 路由', async () => {
    const { tm, created } = makeTm();
    await dispatchAgent({ tabManager: tm, agentId: 'arch', agents: AGENTS });
    assert.equal(created[0].projectId, 'proj-1');
    assert.equal(created[0].cwd, '/tmp/p1');
    assert.equal(created[0].backend, 'claude-code');
    assert.equal(created[0].providerId, 'deepseek');
    assert.equal(created[0].model, 'deepseek-chat');
  });

  test('标签取 agent 名称（有 icon 带上），取不到则回退 @id', async () => {
    const { tm, created } = makeTm();
    await dispatchAgent({ tabManager: tm, agentId: 'dev', agents: AGENTS });
    assert.equal(created[0].label, '🚀 DevOps');

    tm._last = null;
    await dispatchAgent({ tabManager: tm, agentId: 'ghost', agents: AGENTS });
    assert.equal(created[1].label, '@ghost');
    assert.equal(created[1].agent, 'ghost');
  });

  test('没有项目上下文 → 如实提示，且不创建 Slot', async () => {
    const { tm, created } = makeTm({ active: false });
    const notices = [];
    const ok = await dispatchAgent({ tabManager: tm, agentId: 'arch', agents: AGENTS, onNotice: (m) => notices.push(m) });
    assert.equal(ok, false);
    assert.equal(created.length, 0);
    assert.match(notices[0], /项目/);
  });

  test('活跃终端是 error tab → 同样拒绝（没有可用项目上下文）', async () => {
    const { tm, created } = makeTm({ isError: true });
    const ok = await dispatchAgent({ tabManager: tm, agentId: 'arch', agents: AGENTS, onNotice: () => {} });
    assert.equal(ok, false);
    assert.equal(created.length, 0);
  });

  test('没有 tabManager → 如实提示，不抛异常', async () => {
    const notices = [];
    const ok = await dispatchAgent({ tabManager: null, agentId: 'arch', onNotice: (m) => notices.push(m) });
    assert.equal(ok, false);
    assert.equal(notices.length, 1);
  });

  test('createTab 抛错 → 提示失败并返回 false（不吞异常）', async () => {
    const { tm } = makeTm({ createTab: async () => { throw new Error('boom'); } });
    const notices = [];
    const ok = await dispatchAgent({ tabManager: tm, agentId: 'arch', agents: AGENTS, onNotice: (m) => notices.push(m) });
    assert.equal(ok, false);
    assert.match(notices[0], /boom/);
  });
});

describe('insertAgentMention — ⌥/Ctrl/Cmd + 点击 = 当前会话点名', () => {
  test('优先走 mention.sendInput', () => {
    const { tm } = makeTm();
    const sent = [];
    tm.tabs.get('t1').mention = { active: false, sendInput: (s) => sent.push(s) };
    const ok = insertAgentMention({ tabManager: tm, agentId: 'arch' });
    assert.equal(ok, true);
    assert.deepEqual(sent, ['@arch ']);
  });

  test('弹窗活跃时先 hide 再写入', () => {
    const { tm } = makeTm();
    const sent = [];
    let hidden = false;
    tm.tabs.get('t1').mention = { active: true, hide: () => { hidden = true; }, sendInput: (s) => sent.push(s) };
    insertAgentMention({ tabManager: tm, agentId: 'arch' });
    assert.equal(hidden, true);
    assert.deepEqual(sent, ['@arch ']);
  });

  test('mention 不可用 → 回退 ptyWrite', () => {
    const { tm } = makeTm();
    const wrote = [];
    const ok = insertAgentMention({ tabManager: tm, agentId: 'dev', ptyWrite: (id, d) => { wrote.push([id, d]); return Promise.resolve(); } });
    assert.equal(ok, true);
    assert.deepEqual(wrote, [['t1', '@dev ']]);
  });

  test('mention 抛错 → 回退 ptyWrite', () => {
    const { tm } = makeTm();
    tm.tabs.get('t1').mention = { active: false, sendInput: () => { throw new Error('broken'); } };
    const wrote = [];
    const origWarn = console.warn;
    console.warn = () => {};
    let ok;
    try {
      ok = insertAgentMention({ tabManager: tm, agentId: 'arch', ptyWrite: (id, d) => { wrote.push(d); return Promise.resolve(); } });
    } finally {
      console.warn = origWarn;
    }
    assert.equal(ok, true);
    assert.deepEqual(wrote, ['@arch ']);
  });

  test('两条路都不可用 → 返回 false 并提示（不假装成功）', () => {
    const { tm } = makeTm();
    const notices = [];
    const ok = insertAgentMention({ tabManager: tm, agentId: 'arch', onNotice: (m) => notices.push(m) });
    assert.equal(ok, false);
    assert.equal(notices.length, 0, '这里不提示（有终端只是没通道），由调用方决定视觉反馈');
  });

  test('无活跃终端 → 提示并返回 false', () => {
    const { tm } = makeTm({ active: false });
    const notices = [];
    const ok = insertAgentMention({ tabManager: tm, agentId: 'arch', onNotice: (m) => notices.push(m) });
    assert.equal(ok, false);
    assert.equal(notices.length, 1);
  });
});

describe('isMentionModifier — 两种语义的分界', () => {
  test('⌥ / Ctrl / Cmd 为真', () => {
    assert.equal(isMentionModifier({ altKey: true }), true);
    assert.equal(isMentionModifier({ ctrlKey: true }), true);
    assert.equal(isMentionModifier({ metaKey: true }), true);
  });
  test('无修饰键 / 无事件为假', () => {
    assert.equal(isMentionModifier({}), false);
    assert.equal(isMentionModifier(null), false);
    assert.equal(isMentionModifier(undefined), false);
  });
});

// ── 派活：把任务正文填进当前活跃 Slot ────────────────────────────────
// 与"点名"共用同一条写入路径（writeToActiveSlot）—— 这里钉住"派活"的语义：
// 不写 @owner（自己 @ 自己没意义）、**不代按回车**（人看过再发）。

describe('sendTaskToSlot（派活）', () => {
  const TASK = { id: 'write-draft', title: '写一篇公众号稿', note: '面向技术读者' };

  function withMention() {
    const written = [];
    const { tm } = makeTm();
    tm.tabs.get('t1').mention = { active: false, sendInput: (t) => written.push(t), hide() {} };
    return { tm, written };
  }

  test('把任务 id、标题、备注填进活跃 Slot', () => {
    const { tm, written } = withMention();
    assert.equal(sendTaskToSlot({ tabManager: tm, task: TASK }), true);
    assert.equal(written.length, 1);
    const text = written[0];
    assert.ok(text.includes('write-draft'), '要带任务 id');
    assert.ok(text.includes('写一篇公众号稿'), '要带标题');
    assert.ok(text.includes('面向技术读者'), '要带备注');
    assert.ok(text.includes('产出记录') && text.includes('review'), '要提示产出+等待验收');
  });

  test('不代按回车 —— 由人确认后发送', () => {
    const { tm, written } = withMention();
    sendTaskToSlot({ tabManager: tm, task: TASK });
    assert.ok(written[0].endsWith('\n'), '末尾只留一个换行，不额外提交');
    assert.ok(!written[0].includes('\r'), '不能带回车（\r）触发提交');
  });

  test('不写 @owner —— 我们本来就在这个员工的会话里', () => {
    const { tm, written } = withMention();
    sendTaskToSlot({ tabManager: tm, task: { ...TASK, owner: 'dev' } });
    assert.ok(!written[0].startsWith('@dev'), '自己 @ 自己没有意义');
  });

  test('mention 不可用 → 回退 ptyWrite（与点名同一条兜底）', () => {
    const { tm } = makeTm();
    const pty = [];
    assert.equal(sendTaskToSlot({ tabManager: tm, task: TASK, ptyWrite: (id, t) => { pty.push([id, t]); } }), true);
    assert.equal(pty.length, 1);
    assert.equal(pty[0][0], 't1');
    assert.ok(pty[0][1].includes('write-draft'));
  });

  test('没有任务 → 提示且不写', () => {
    const { tm, written } = withMention();
    const notices = [];
    assert.equal(sendTaskToSlot({ tabManager: tm, task: null, onNotice: (m) => notices.push(m) }), false);
    assert.equal(written.length, 0);
    assert.ok(notices.length > 0);
  });

  test('没有活跃终端 → 提示且返回 false（不假装成功）', () => {
    const { tm } = makeTm({ active: false });
    const notices = [];
    assert.equal(sendTaskToSlot({ tabManager: tm, task: TASK, onNotice: (m) => notices.push(m) }), false);
    assert.ok(notices.some((m) => m.includes('终端')));
  });

  test('两种写入路径都不可用 → false', () => {
    const { tm } = makeTm();
    assert.equal(sendTaskToSlot({ tabManager: tm, task: TASK }), false);
  });
});

describe('writeToActiveSlot（有意写入的唯一实现）', () => {
  test('点名与派活走的是同一段代码（同一兜底行为）', () => {
    const { tm } = makeTm();
    const viaMention = [];
    tm.tabs.get('t1').mention = { active: false, sendInput: (t) => viaMention.push(t), hide() {} };
    writeToActiveSlot({ tabManager: tm, text: 'X' });
    insertAgentMention({ tabManager: tm, agentId: 'dev' });
    assert.deepEqual(viaMention, ['X', '@dev ']);
  });

  test('mention 面板打开时先收起再写入（避免文字落到补全框里）', () => {
    const { tm } = makeTm();
    let hidden = 0;
    const written = [];
    tm.tabs.get('t1').mention = {
      active: true,
      hide() { hidden++; },
      sendInput: (t) => written.push(t),
    };
    writeToActiveSlot({ tabManager: tm, text: 'Y' });
    assert.equal(hidden, 1);
    assert.deepEqual(written, ['Y']);
  });
});
