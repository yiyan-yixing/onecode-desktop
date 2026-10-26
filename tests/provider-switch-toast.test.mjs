// 切换提示的文案：**自动切换必须看起来就是自动的**。
//
// 熔断触发的切换会重启终端；如果提示与用户自己点的完全一样，
// 用户会以为是自己手滑了，或者根本没注意到。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { switchToast } from '../src/switch-toast.js';

const nameOf = (id) => ({ glm: 'GLM', kimi: 'Kimi' })[id] || '';

describe('switchToast', () => {
  test('手动切换：普通文案', () => {
    const t = switchToast({ from: 'kimi', to: 'glm', reason: 'manual' }, nameOf);
    assert.equal(t.text, '已切换到 GLM');
    assert.equal(t.kind, 'ok');
  });

  test('★ 熔断触发的切换必须说清"自动"和"因为连续失败"', () => {
    const t = switchToast({ from: 'kimi', to: 'glm', reason: 'consecutive_failure' }, nameOf);
    assert.ok(t.text.includes('自动'), t.text);
    assert.ok(t.text.includes('连续失败'), t.text);
    assert.ok(t.text.includes('GLM'), t.text);
    assert.equal(t.kind, 'warn', '自动切换要用告警样式，不能与手动一模一样');
  });

  test('自动切换要提到终端会重启（这是个有副作用的动作）', () => {
    const t = switchToast({ from: 'kimi', to: 'glm', reason: 'consecutive_failure' }, nameOf);
    assert.ok(t.text.includes('重启'), t.text);
  });

  test('拿不到名字时退回 id，不显示空白', () => {
    const t = switchToast({ from: 'x', to: 'unknown-provider', reason: 'manual' }, nameOf);
    assert.equal(t.text, '已切换到 unknown-provider');
  });

  test('reason 缺失当成手动（老数据/其它来源）', () => {
    const t = switchToast({ from: 'kimi', to: 'glm' }, nameOf);
    assert.equal(t.kind, 'ok');
  });

  test('from 是 <none> 时不说"<none> 连续失败"', () => {
    const t = switchToast({ from: '<none>', to: 'glm', reason: 'consecutive_failure' }, nameOf);
    assert.ok(!t.text.includes('<none>'), t.text);
  });

  test('不炸', () => {
    assert.equal(typeof switchToast(null, nameOf).text, 'string');
    assert.equal(typeof switchToast({}, null).text, 'string');
  });
});
