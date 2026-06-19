// frontmatter_body / fm_field 逻辑验证。
// 对象：tests/lib/frontmatter.mjs（cc_status.rs:320-345 忠实移植）。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import { frontmatterBody, fmField } from './lib/frontmatter.mjs';

describe('frontmatterBody（cc_status.rs:320）', () => {
  test('正常 frontmatter', () => {
    const c = '---\nname: foo\ndescription: bar\n---\n# body';
    assert.equal(frontmatterBody(c), 'name: foo\ndescription: bar');
  });
  test('首行非 --- → null', () => {
    assert.equal(frontmatterBody('# title\n---\nx'), null);
    assert.equal(frontmatterBody('plain text'), null);
  });
  test('首行 --- 带尾随空格仍识别', () => {
    assert.equal(frontmatterBody('---   \nname: x\n---'), 'name: x');
  });
  test('无闭合 --- → null', () => {
    assert.equal(frontmatterBody('---\nname: x'), null);
  });
  test('空 frontmatter → 空串', () => {
    assert.equal(frontmatterBody('---\n---\nbody'), '');
  });
  test('多行值（非 list）整行保留', () => {
    const fm = frontmatterBody('---\ndescription: a b c\n---');
    assert.equal(fm, 'description: a b c');
  });
});

describe('fmField（cc_status.rs:337）', () => {
  const fm = 'name: Arch\ndescription: builds things\ntools: Read, Write\nmodel: opus';
  test('取存在字段', () => {
    assert.equal(fmField(fm, 'name'), 'Arch');
    assert.equal(fmField(fm, 'model'), 'opus');
  });
  test('缺失字段 → 空串', () => {
    assert.equal(fmField(fm, 'color'), '');
    assert.equal(fmField('', 'name'), '');
  });
  test('值带前后空格被 trim', () => {
    assert.equal(fmField('name:   spaced   \n', 'name'), 'spaced');
  });
  test('行首缩进仍命中（Rust: trim_start 后匹配前缀）', () => {
    assert.equal(fmField('  name: indented', 'name'), 'indented');
  });
  test('前缀精确：name 不误匹配 names', () => {
    assert.equal(fmField('names: x\nname: y', 'name'), 'y');
  });
  test('取首个匹配行', () => {
    assert.equal(fmField('model: a\nmodel: b', 'model'), 'a');
  });
});
