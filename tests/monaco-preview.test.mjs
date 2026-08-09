// Monaco 编辑器原型模块（src/editor/monaco-preview.js）最小守护测试。
//
// 仅验证模块可导入、结构稳定、示例内容完整——不加载真实 monaco（浏览器依赖），
// 也不触碰终端/会话逻辑。浏览器内加载逻辑由 tauri dev 手工验证。

import { test, describe } from 'node:test';
import assert from 'node:assert/strict';
import {
  MonacoPreviewController,
  SAMPLE_TITLE,
  SAMPLE_LANG,
  SAMPLE_CONTENT,
  LANGS,
} from '../src/editor/monaco-preview.js';

describe('monaco-preview.js 原型模块结构', () => {
  test('导出控制器类（函数）', () => {
    assert.equal(typeof MonacoPreviewController, 'function');
  });

  test('原型控制器暴露核心方法', () => {
    const ctrl = new MonacoPreviewController();
    for (const m of ['init', 'open', 'close', 'toggle', 'destroy']) {
      assert.equal(typeof ctrl[m], 'function', `缺少方法 ${m}`);
    }
  });

  test('内置示例语言为 javascript', () => {
    assert.equal(SAMPLE_LANG, 'javascript');
  });

  test('示例内容非空且含调度台演示片段', () => {
    assert.ok(SAMPLE_CONTENT.length > 200, '示例内容过短');
    assert.ok(SAMPLE_CONTENT.includes('Monaco'), '示例应提及 Monaco');
    assert.ok(SAMPLE_CONTENT.includes('@dev'), '示例应含 AI 员工演示');
  });

  test('语言下拉覆盖常见语言', () => {
    const ids = LANGS.map((l) => l.id);
    for (const lang of ['javascript', 'typescript', 'json', 'markdown', 'rust', 'python']) {
      assert.ok(ids.includes(lang), `缺语言 ${lang}`);
    }
  });
});
