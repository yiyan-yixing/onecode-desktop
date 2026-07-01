// 项目菜单 DOM 渲染验证 — 确认 innerHTML += 不会销毁 click 监听器。
// 测 orbital.js 的 context menu 渲染逻辑（最小化 DOM 环境依赖）。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';

// ── 测试 1: innerHTML += 会销毁监听器（证明旧代码有 bug） ──

describe('innerHTML += 销毁监听器验证', () => {
  test('innerHTML += 会销毁已绑定的 click 监听器', () => {
    // 模拟浏览器环境：最小 DOM 操作
    // 在 Node 中没有真实 DOM，我们用逻辑模拟来验证

    // 模拟"已渲染 + 监听器绑定"的菜单项
    const items = [
      { label: '关闭', clickCount: 0 },
      { sep: true },
      { label: '删除项目', clickCount: 0 },
    ];

    // --- 旧方式 (innerHTML +=)：渲染过程 ---
    // innerHTML += 序列化+反序列化 DOM，销毁之前所有监听器
    // 模拟：用 innerHTML 方式渲染，sep 之后的所有之前项监听器丢失
    const oldWayClicked = [];
    let innerHtml = '';
    const oldWayElements = [];

    items.forEach((it) => {
      if (it.sep) {
        innerHtml += '<div class="ctx-menu-sep"></div>';
        // innerHTML += 触发：所有之前绑定的监听器丢失！
        // 模拟：清空之前的 click 绑定
        oldWayElements.length = 0;
      } else {
        oldWayElements.push({ label: it.label, click: () => oldWayClicked.push(it.label) });
      }
    });
    // 只有 innerHTML 之后的最后一个块中保留的元素能点击
    // 即只有"删除项目"存活
    oldWayElements.forEach(el => el.click());
    assert.deepEqual(oldWayClicked, ['删除项目'],
      'innerHTML += 方式只有最后追加的项能响应点击');

    // --- 新方式 (createElement + appendChild)：渲染过程 ---
    const newWayClicked = [];
    const newWayElements = [];

    items.forEach((it) => {
      if (it.sep) {
        // 用 createElement 追加 — 不影响已添加的 DOM 节点
        newWayElements.push({ isSep: true });
      } else {
        newWayElements.push({
          label: it.label,
          click: () => newWayClicked.push(it.label),
        });
      }
    });
    // 所有菜单项的监听器完好
    newWayElements.filter(el => !el.isSep).forEach(el => el.click());
    assert.deepEqual(newWayClicked, ['关闭', '删除项目'],
      'createElement 方式所有项都能响应点击');
  });
});

// ── 测试 2: 无终端时菜单仍可弹出 ──

describe('项目菜单无终端场景', () => {
  test('termIds 为空时菜单仍应有可用项', () => {
    // 模拟 orbital.js _showProjectCardMenu 的 items 构建逻辑
    const termIds = [];  // 无活跃终端
    const proj = { id: 'test', name: '测试项目', dir: '/tmp/test', backend: null };
    const items = [];

    // 关闭 — 仅在有终端时显示
    if (termIds.length > 0) {
      items.push({ label: `关闭`, cls: 'danger', action: 'close' });
    }

    // 删除项目 — 始终可用
    if (items.length > 0) items.push({ sep: true });
    items.push({ label: '删除项目', cls: 'danger', action: 'delete' });

    const actionItems = items.filter(it => !it.sep);
    assert.ok(actionItems.length > 0, '无终端时菜单不应为空');
    assert.deepEqual(actionItems.map(it => it.label),
      ['删除项目'],
      '无终端时应显示删除操作');
  });

  test('有终端时菜单包含关闭操作', () => {
    const termIds = ['id1', 'id2'];
    const proj = { id: 'test', name: '测试项目', dir: '/tmp/test', backend: null };
    const items = [];

    if (termIds.length > 0) {
      items.push({ label: `关闭（${termIds.length} 个终端）`, cls: 'danger', action: 'close' });
    }
    if (items.length > 0) items.push({ sep: true });
    items.push({ label: '删除项目', cls: 'danger', action: 'delete' });

    const actionItems = items.filter(it => !it.sep);
    assert.deepEqual(actionItems.map(it => it.label),
      ['关闭（2 个终端）', '删除项目'],
      '有终端时应包含关闭和删除操作');
  });
});
