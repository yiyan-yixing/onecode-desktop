// cc_status 加载器决策逻辑验证。
// 对象：cc_status.rs 的 load_tasks / skill_from_content / load_agents 逐项规则
//       （用 frontmatter 移植库 + 内联决策逻辑，对照 JS 原版 cc-status.js 同规则）。
//
// 不做 FS 扫描（那是平台 IO），聚焦「逐项决策」——bug 多藏于此。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { frontmatterBody, fmField } from './lib/frontmatter.mjs';

// ── skill_from_content（cc_status.rs:190）────────────────────────────
// 规则：有 fm → name=fm.name||fallback, desc=fm.description
//      无 fm → name=fallback, desc=首个 # 标题
function skillFromContent(fallbackName, content) {
  const fm = frontmatterBody(content);
  if (fm != null) {
    return { name: fmField(fm, 'name') || fallbackName, description: fmField(fm, 'description') };
  }
  const firstHeading = content.split('\n').find((l) => l.trimStart().startsWith('#'));
  const desc = firstHeading ? firstHeading.replace(/^#+\s*/, '').trim() : '';
  return { name: fallbackName, description: desc };
}

describe('skill_from_content（cc_status.rs:190）', () => {
  test('有 frontmatter：name + description', () => {
    const s = skillFromContent('fallback', '---\nname: Real\ndescription: does X\n---\nbody');
    assert.equal(s.name, 'Real');
    assert.equal(s.description, 'does X');
  });
  test('fm 缺 name → 用 fallback', () => {
    const s = skillFromContent('fb', '---\ndescription: d\n---');
    assert.equal(s.name, 'fb');
  });
  test('无 frontmatter → fallback + 首个标题为 desc', () => {
    const s = skillFromContent('my-skill', '# Heading One\nrest');
    assert.equal(s.name, 'my-skill');
    assert.equal(s.description, 'Heading One');
  });
  test('无 fm 且无标题 → desc 空', () => {
    const s = skillFromContent('plain', 'just text no heading');
    assert.equal(s.name, 'plain');
    assert.equal(s.description, '');
  });
});

// ── load_tasks name 兜底（cc_status.rs:270）─────────────────────────
// Rust: name = t.name || (prompt.chars().take(60)).collect()
//       （取 prompt 前 60 字符；JS 原版同：t.name || t.prompt?.slice(0,60)）
function taskName(t) {
  if (t.name) return t.name;
  if (typeof t.prompt === 'string') return [...t.prompt].slice(0, 60).join('');
  return '';
}
describe('load_tasks name 兜底（cc_status.rs:270）', () => {
  test('有 name 优先', () => assert.equal(taskName({ name: 'Daily', prompt: 'p' }), 'Daily'));
  test('无 name → prompt 前 60 字符', () => {
    const long = 'x'.repeat(100);
    assert.equal(taskName({ prompt: long }), 'x'.repeat(60));
  });
  test('prompt 正好 60 不截断', () => {
    const exact = 'y'.repeat(60);
    assert.equal(taskName({ prompt: exact }), exact);
  });
  test('prompt 短于 60 全保留', () => {
    assert.equal(taskName({ prompt: 'short' }), 'short');
  });
  test('按字符（非字节）截断——CJK', () => {
    const cjk = '中'.repeat(70);
    assert.equal([...taskName({ prompt: cjk })].length, 60);
  });
  test('全空 → 空串', () => assert.equal(taskName({}), ''));
});

// ── load_agents 跳过规则（cc_status.rs:286）─────────────────────────
// 规则：跳过非 .md；无 frontmatter → 跳过；fm 无 name → 跳过
function shouldKeepAgent(fileName, content) {
  if (fileName.startsWith('.') || !fileName.endsWith('.md')) return false;
  const fm = frontmatterBody(content);
  if (fm == null) return false;
  return fmField(fm, 'name').length > 0;
}
describe('load_agents 跳过规则（cc_status.rs:286）', () => {
  test('正常 agent 保留', () => {
    assert.equal(shouldKeepAgent('arch.md', '---\nname: Arch\ndescription: d\n---'), true);
  });
  test('隐藏文件跳过', () => assert.equal(shouldKeepAgent('.hidden.md', '---\nname: X\n---'), false));
  test('非 md 跳过', () => assert.equal(shouldKeepAgent('arch.txt', '---\nname: X\n---'), false));
  test('无 frontmatter 跳过', () => assert.equal(shouldKeepAgent('arch.md', '# no fm'), false));
  test('fm 缺 name 跳过', () => assert.equal(shouldKeepAgent('arch.md', '---\ndescription: d\n---'), false));
});

// ── 已知差异：Rust load_tasks 无去重 ────────────────────────────────
describe('已知差异：Rust load_tasks 缺少 seenIds 去重', () => {
  // JS 原版 cc-status.js:259 用 seenIds 按 id（t.id || cron+prompt+name）去重；
  // Rust cc_status.rs:261 的 load_tasks 未见去重——同一任务出现在
  // scheduled_tasks.json 与 cron-session.json 会被 emit 两次。记录为待修。
  test('文档化：JS 原版去重 vs Rust 不去重', () => {
    // 仅断言「两个文件含同 id 时，朴素双遍加载会产出 2 条」这一事实，
    // 提示 Rust 端需要补去重。
    const fileA = [{ id: 't1', name: 'A', prompt: 'p', cron: '0 * * * *' }];
    const fileB = [{ id: 't1', name: 'A', prompt: 'p', cron: '0 * * * *' }];
    const naive = [...fileA, ...fileB]; // Rust 当前行为
    const deduped = [...new Map(naive.map((t) => [t.id || (t.cron + t.prompt + t.name), t])).values()];
    assert.equal(naive.length, 2);
    assert.equal(deduped.length, 1, '正确去重应只剩 1 条；Rust 当前会留 2 条');
  });
});
