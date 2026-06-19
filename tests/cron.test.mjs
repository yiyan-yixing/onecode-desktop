// cron_next / expand 逻辑验证。
// 验证对象：tests/lib/cron.mjs（cc_status.rs:349-497 的忠实移植）。
// Oracle：原版 cc-status.js cronNext（从 onecode 提取）。
//
// 关键策略：
// 1) expand 纯函数——穷举手工期望值。
// 2) cronNext——格式/类别/不可能日剪枝/非法输入。
// 3) 交叉比对——在「冻结时刻」下，移植版与 oracle 必须逐位相等（排除当前时间抖动）。
// 4) 已知差异断言——*/0 在 Rust 失败收敛（''），JS 原版视作 *；显式记录。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { expand, cronNext } from './lib/cron.mjs';
import { loadOracleCronNext } from './lib/oracle-cron.mjs';

const setEq = (got, expected) => {
  assert.deepEqual([...got].sort((a, b) => a - b), [...expected].sort((a, b) => a - b));
};

describe('expand — cron 字段展开（cc_status.rs:457）', () => {
  test('* 全域', () => setEq(expand('*', 0, 59), Array.from({ length: 60 }, (_, i) => i)));
  test('单值', () => setEq(expand('5', 0, 59), [5]));
  test('范围 1-5', () => setEq(expand('1-5', 0, 59), [1, 2, 3, 4, 5]));
  test('逗号 1,3,5', () => setEq(expand('1,3,5', 0, 59), [1, 3, 5]));
  test('步长 */15（分）', () => setEq(expand('*/15', 0, 59), [0, 15, 30, 45]));
  test('范围步长 10-30/5', () => setEq(expand('10-30/5', 0, 59), [10, 15, 20, 25, 30]));
  test('单值步长 5/10 → {5}（单值 range，lo=hi=5）', () => setEq(expand('5/10', 0, 59), [5]));
  test('月范围 1-12', () => setEq(expand('*', 1, 12), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]));
  test('小时 0-23 */12', () => setEq(expand('*/12', 0, 23), [0, 12]));
  test('dow 0-6 *', () => setEq(expand('*', 0, 6), [0, 1, 2, 3, 4, 5, 6]));

  test('越界单值 → null（Rust: None，整体失败）', () => {
    assert.equal(expand('60', 0, 59), null);
    assert.equal(expand('99', 1, 31), null);
  });
  test('逗号含一个非法 → null（整体失败）', () => {
    assert.equal(expand('5,99', 0, 59), null);
  });
  test('非数字 → null', () => {
    assert.equal(expand('abc', 0, 59), null);
    assert.equal(expand('', 0, 59), null);
    assert.equal(expand('5-abc', 0, 59), null);
  });
  test('步长 0 → null（Rust: filter(s>=1) 拒绝）— 与 JS 原版 */0 视作 * 不同', () => {
    assert.equal(expand('*/0', 0, 59), null);
    assert.equal(expand('5-30/0', 0, 59), null);
  });
  test('范围越界自动夹断到 [lo,hi]', () => {
    // 50 在分钟域越界 → 单值返回 null；但范围 0-100 夹到 0-59
    setEq(expand('0-100', 0, 59), Array.from({ length: 60 }, (_, i) => i));
  });
});

describe('cronNext — 格式与边界（cc_status.rs:349）', () => {
  test('字段数 != 5 → 空串', () => {
    assert.equal(cronNext('* * *'), '');
    assert.equal(cronNext(''), '');
    assert.equal(cronNext('* * * * * *'), '');
  });
  test('非法字段 → 空串', () => {
    assert.equal(cronNext('60 0 * * *'), '');   // 分钟越界
    assert.equal(cronNext('0 24 * * *'), '');   // 小时越界
    assert.equal(cronNext('0 0 0 * *'), '');    // 日 0 越界
    assert.equal(cronNext('* * * * 7'), '');    // dow 7 越界
    assert.equal(cronNext('a b c d e'), '');
  });
  test('不可能日提前剪枝（maxDayByMonth）', () => {
    assert.equal(cronNext('0 0 31 2 *'), '');   // 2 月无 31 日
    assert.equal(cronNext('0 0 30 2 *'), '');   // 2 月最 29 < 30
    assert.equal(cronNext('0 0 31 4 *'), '');   // 4 月无 31 日
  });
  test('可能日不被误剪（剪枝仅删真正不可能的组合）', () => {
    // 31 日：存在 31 天的月份 → 未被剪枝 → 366 天内必有 31 号 → 非空
    assert.ok(cronNext('0 0 31 * *').length > 0);
    // 30 日同理
    assert.ok(cronNext('0 0 30 * *').length > 0);
    // 29 日：剪枝检查 dom_min=29，2 月 maxDay=29>=29 → 不剪枝；
    // 但从当前日期出发下一个 2/29 可能超出 366 天窗口 → 结果为空也属正常（与 oracle 一致）。
    // 此处只断言「未被提前剪枝返回 '' 而与 oracle 不同」——交给交叉比对覆盖。
  });
  test('每分钟 → "< 1m"', () => {
    assert.equal(cronNext('* * * * *'), '< 1m');
  });
  test('每日凌晨 → 输出形如 Ndh 或 Nh Nm（当天或次日）', () => {
    const out = cronNext('0 0 * * *');
    assert.match(out, /^(\d+d \d+h|\d+h \d+m)$/);
  });
  test('输出归一化为纯 ASCII 单位串', () => {
    for (const expr of ['0 3 * * *', '*/30 * * * *', '0 0 1 * *']) {
      const out = cronNext(expr);
      assert.match(out, /^< 1m$|^\d+m$|^\d+h \d+m$|^\d+d \d+h$/);
    }
  });
});

// ── 冻结时刻的确定性交叉比对 ────────────────────────────────────────
// 临时把 globalThis.Date 替换为「无参构造返回固定时刻」的 Proxy，
// 让移植版 cronNext 与 oracle cronNext 看到同一个 now，从而逐字相等。
function withFrozenDate(ms, fn) {
  const real = globalThis.Date;
  globalThis.Date = new Proxy(real, {
    construct(t, args) { return args.length === 0 ? new real(ms) : new t(...args); },
    apply(t, thisArg, args) { return args.length === 0 ? new real(ms) : t.apply(thisArg, args); },
    get(t, p) { const v = t[p]; return typeof v === 'function' ? v.bind(t) : v; },
  });
  try { return fn(); } finally { globalThis.Date = real; }
}

describe('cronNext × oracle 交叉比对（冻结时刻，逐字相等）', () => {
  // 选若干时刻覆盖跨日/跨月/跨年场景。
  const FROZEN = [
    Date.UTC(2026, 5, 20, 9, 15, 30),   // 2026-06-20 09:15:30 UTC（Local 视角不影响「相对差」逻辑）
    Date.UTC(2026, 0, 1, 0, 0, 5),      // 年初
    Date.UTC(2026, 11, 31, 23, 59, 0),  // 年末
    Date.UTC(2026, 2, 15, 12, 30, 0),   // 月中
  ];
  const EXPRS = [
    '* * * * *',
    '*/5 * * * *',
    '0 * * * *',
    '0 3 * * *',
    '30 9 * * 1-5',
    '0 0 * * *',
    '0 0 1 * *',
    '0 0 1 1 *',
    '0 0 15 * *',
    '0 0 29 2 *',
    '15,45 8-18 * * 1-5',
    '*/30 9-17 * * 1-5',
  ];

  for (const now of FROZEN) {
    for (const expr of EXPRS) {
      test(`now=${new Date(now).toISOString()} expr="${expr}"`, () => {
        withFrozenDate(now, () => {
          const port = cronNext(expr);
          const oracle = loadOracleCronNext()(expr);
          // 移植版与原版必须逐字相等（* 在秒=0 的冻结时刻为 "1m" 而非 "< 1m"，故不硬编码）
          assert.equal(port, oracle, `expr="${expr}" port="${port}" oracle="${oracle}"`);
        });
      });
    }
  }
});

describe('已知差异：*/0 步长', () => {
  test('Rust 移植：*/0 视为非法 → 空串', () => {
    assert.equal(cronNext('*/0 * * * *'), '');
  });
  test('JS 原版 oracle：*/0 视作 * → 非空（每分钟）', () => {
    // 这是与原版的故意分歧，Rust 失败收敛更安全；记录不视为 bug。
    assert.notEqual(loadOracleCronNext()('*/0 * * * *'), '');
  });
});
