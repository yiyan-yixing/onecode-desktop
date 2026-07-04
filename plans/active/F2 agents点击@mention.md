# F2: Agents 列表点击后自动 @mention 插入到终端输入

> **类型:** 功能 | **优先级:** 中 | **状态:** ✅ 已完成
> **完成日期:** 2026-07-04
> **依赖:** F1（需要先有 agents Tab 才能实现点击插入）

## 需求描述

在 F1 的 agents Tab 列表中，点击某个 agent 项后，自动将 `@agent-id ` 插入到当前活跃终端的输入中，等同于用户手动输入 `@` 触发 mention 后选中该 agent。

## 设计方案

1. Agents 列表项添加 click/keydown 事件 → `_onAgentClick(agentId, itemEl)`
2. 点击时获取活跃终端 ID（通过 TabManager.activeId）
3. 向活跃 PTY 写入 `@agent-id `（复用 MentionController.sendInput 路径）
4. 如果有活跃终端，自动聚焦回终端
5. 如果无活跃终端，显示 toast 提示引导先创建终端
6. 300ms 防抖防止快速连续点击产生重复插入
7. mention 弹窗活跃时先 hide() 再 sendInput，与 MentionController._select 行为一致

## 实现变更

| 文件 | 变更 |
|------|------|
| `src/agents-list.js` | 添加 `setTabManager()`, `setPtyWrite()`, `markTerminalStateChanged()`, `_onAgentClick()`, `_flashAndFocus()`, `_showNoTerminalToast()`, 防抖, disabled 状态渲染, 键盘可访问性, mention.hide() |
| `src/main.js` | 添加 `setTabManager`, `setPtyWrite`, `markTerminalStateChanged` 注入 |
| `src/styles.css` | 添加 `.al-disabled`, `.al-mention-flash`, `.al-toast` 样式 |
| `tests/agents-list.test.mjs` | 新建 23 个测试覆盖点击、防抖、兜底路径、边界、mention.hide、terminalStateChanged |

## 子任务

| 编号 | 任务 | 状态 |
|------|------|------|
| F2-1 | Agents 列表项点击事件 | ✅ DONE |
| F2-2 | @mention 插入逻辑 | ✅ DONE |
| F2-3 | 视觉反馈 + 键盘可访问性 | ✅ DONE |
| F2-4 | 边界处理（disabled/toast） | ✅ DONE |
| F2-5 | 代码审查修复（P1-1 disabled 状态失效, mention.hide, 键盘可访问, CSS !important 移除） | ✅ DONE |

## 测试结果

- `npm test`: 230 assertions, 50 suites, 0 failures
- 新增 23 个测试（agents-list.test.mjs）覆盖：
  - 主路径：mention.sendInput 写入 @agent-id
  - mention 弹窗活跃时先 hide() 再 sendInput
  - 兜底路径：mention 不可用时走 ptyWrite
  - 防抖：300ms 内重复点击忽略
  - 边界：无 TabManager / 无终端 / error tab → toast
  - 视觉反馈：al-mention-flash 类 + term.focus()
  - markTerminalStateChanged：终端状态变更后刷新正确

## 验收标准

1. ✅ 在 agents Tab 中点击任意 agent，活跃终端输入行出现 `@agent-id `
2. ✅ 插入后终端自动获得焦点，可以继续输入
3. ✅ 多终端场景：点击 agent 后插入到当前活跃终端（通过 TabManager.activeId）
4. ✅ 无活跃终端时：agents 列表项显示 disabled 态 + 点击后显示 toast 提示
5. ✅ 连续快速点击不同 agent 不产生重复或错乱（300ms 防抖）
6. ✅ 与手动输入 `@` 触发 mention 选中的效果一致（相同 sendInput 路径 + hide() 行为）
