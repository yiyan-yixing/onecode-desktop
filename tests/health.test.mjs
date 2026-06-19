// 健康判定 report_of 验证。
// 对象：tests/lib/health.mjs（pty/health.rs:22-92 忠实移植）。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { healthAction, reportOf, ACTIONS, RSS_WARN_BYTES } from './lib/health.mjs';

const GIB = 1024 ** 3;
const TWO_GIB = RSS_WARN_BYTES; // 2 * 2^30

describe('healthAction — 判定优先级（health.rs:69）', () => {
  test('僵尸 → kill（最高优先级，即使 RSS 也高）', () => {
    assert.equal(healthAction({ status: 'running', alive: false, isZombie: true, rssBytes: TWO_GIB }), ACTIONS.KILL);
  });
  test('进程不在但 slot 仍 running → stale', () => {
    assert.equal(healthAction({ status: 'running', alive: false, isZombie: false, rssBytes: null }), ACTIONS.STALE);
  });
  test('RSS >= 2GiB → warn', () => {
    assert.equal(healthAction({ status: 'running', alive: true, isZombie: false, rssBytes: TWO_GIB }), ACTIONS.WARN);
    assert.equal(healthAction({ status: 'running', alive: true, isZombie: false, rssBytes: TWO_GIB + 1 }), ACTIONS.WARN);
  });
  test('RSS 刚好 2GiB-1 → none（边界 <）', () => {
    assert.equal(healthAction({ status: 'running', alive: true, isZombie: false, rssBytes: TWO_GIB - 1 }), ACTIONS.NONE);
  });
  test('RSS 远低 → none', () => {
    assert.equal(healthAction({ status: 'running', alive: true, isZombie: false, rssBytes: 100 * 1024 * 1024 }), ACTIONS.NONE);
  });
  test('RSS 未知且存活 → none', () => {
    assert.equal(healthAction({ status: 'running', alive: true, isZombie: false, rssBytes: null }), ACTIONS.NONE);
  });
  test('僵尸优先于 stale（即使进程列表已无）', () => {
    // alive=false 但 isZombie=true：僵尸尚未被 wait 回收时 alive 可能 false → 仍 kill
    assert.equal(healthAction({ status: 'running', alive: false, isZombie: true, rssBytes: null }), ACTIONS.KILL);
  });
  test('非 running 状态 + 进程不在 → none（不算 stale）', () => {
    assert.equal(healthAction({ status: 'exited', alive: false, isZombie: false, rssBytes: null }), ACTIONS.NONE);
  });
});

describe('reportOf — pid 缺失分支（health.rs:56）', () => {
  test('无 pid → action none, alive false', () => {
    const r = reportOf({ id: 'x', status: 'running', pid: null });
    assert.equal(r.alive, false);
    assert.equal(r.action, ACTIONS.NONE);
    assert.equal(r.rssBytes, null);
  });
});

describe('RSS 阈值常量', () => {
  test('2 GiB = 2147483648', () => {
    assert.equal(RSS_WARN_BYTES, 2 * GIB);
    assert.equal(RSS_WARN_BYTES, 2147483648);
  });
});
