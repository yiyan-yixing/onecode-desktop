// 自动重启退避公式验证。
// 对象：tests/lib/restart.mjs（pty/mod.rs:458-476 忠实移植）。
// 同时与 pty.js:156 的 delay = min(500*2^(count-1),30000) 对照。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { delayMs, shouldGiveUp, isStable, backoffSeries } from './lib/restart.mjs';

describe('delayMs — 指数退避（mod.rs:466）', () => {
  test('序列与 pty.js:156 一致', () => {
    // count(n) = 500 * 2^(n-1)，cap 30000
    const expected = [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000];
    const got = backoffSeries(10);
    assert.deepEqual(got, expected);
  });
  test('首次重启 = 500ms', () => assert.equal(delayMs(1), 500));
  test('第 6 次 = 16s', () => assert.equal(delayMs(6), 16000));
  test('第 7 次 32000 → cap 30000', () => assert.equal(delayMs(7), 30000));
  test('大 count 仍受 30s 上限保护（exp min(..,20)）', () => {
    assert.equal(delayMs(50), 30000);
    assert.equal(delayMs(1000), 30000);
  });
});

describe('shouldGiveUp — 上限放弃（mod.rs:458）', () => {
  test('count 0..9 不放弃', () => {
    for (let c = 0; c < 10; c++) assert.equal(shouldGiveUp(c), false);
  });
  test('count >= 10 放弃（即第 11 次崩溃时停止，共 10 次重启）', () => {
    assert.equal(shouldGiveUp(10), true);
    assert.equal(shouldGiveUp(11), true);
  });
});

describe('isStable — 稳定 5s 重置计数（mod.rs:447）', () => {
  test('< 5s 不稳定', () => {
    assert.equal(isStable(0), false);
    assert.equal(isStable(4999), false);
  });
  test('>= 5s 稳定', () => {
    assert.equal(isStable(5000), true);
    assert.equal(isStable(99999), true);
  });
});

describe('端到端：崩溃序列与 pty.js 行为等价', () => {
  test('10 次重启后第 11 次崩溃放弃', () => {
    let restartCount = 0; // Rust: inc 之前
    let newCount = 0;     // inc 之后
    let crashes = 0;
    let gaveUp = false;
    while (crashes < 15) {
      crashes++;
      if (shouldGiveUp(restartCount)) { gaveUp = true; break; }
      newCount = restartCount + 1;
      restartCount = newCount;
      // 会触发一次 delayMs(newCount) 的重启
    }
    assert.equal(crashes, 11); // 第 11 次崩溃时放弃
    assert.equal(gaveUp, true);
    assert.equal(restartCount, 10); // 完成了 10 次重启
  });
});
