# F2: Agents 列表点击后自动 @mention 插入到终端输入

> **类型:** 功能 | **优先级:** 中 | **状态:** 🟡 排期中
> **目标完成:** 2026-07-19 (六)
> **依赖:** F1（需要先有 agents Tab 才能实现点击插入）

## 需求描述

在 F1 的 agents Tab 列表中，点击某个 agent 项后，自动将 `@agent-id ` 插入到当前活跃终端的输入中，等同于用户手动输入 `@` 触发 mention 后选中该 agent。

## 代码定位

- **MentionController:** `src/terminal/mention.js` — 已有 `_select()` 方法：
  - 发 N 个 backspace 清除已输入的 `@prefix`
  - 再发 `@agent-id `
  - 使用 `sendInput` 回调（即 `ipc.ptyWrite(id, s)`）
- **TabManager:** `src/terminal/tab-manager.js`
  - `this.activeId` — 当前活跃终端 ID
  - `this.tabs.get(id)` — 获取终端状态
  - `state.sendInput` 或 `ipc.ptyWrite(id, data)` — 写入 PTY
- **AgentsListController:** F1 中新建的 agents Tab 控制器
- **IPC:** `src/ipc-bridge.js` — `ptyWrite(id, data)` 写入 PTY

## 设计方案

1. Agents 列表项添加点击事件
2. 点击时获取活跃终端 ID（通过 TabManager.activeId）
3. 向活跃 PTY 写入 `@agent-id `（复用 ipc.ptyWrite，与 MentionController._select 逻辑一致）
4. 如果有活跃终端，自动聚焦回终端
5. 如果无活跃终端，显示提示（toast / inline）引导先创建终端

## 子任务

| 编号 | 任务 | 估时 | 负责人 | 状态 | 依赖 |
|------|------|------|--------|------|------|
| F2-1 | Agents 列表项点击事件：在 AgentsListController 中注册 click handler，获取 agent.id 和活跃终端 id | 1h | @dev | TODO | F1-5 |
| F2-2 | @mention 插入逻辑：调用 ipc.ptyWrite(activeId, `@${agentId} `) 写入活跃 PTY，复用 MentionController 的 sendInput 路径 | 2h | @dev | TODO | F2-1 |
| F2-3 | 视觉反馈：点击 agent 后终端获得焦点并短暂高亮，确认插入成功 | 1h | @dev | TODO | F2-2 |
| F2-4 | 边界处理：无活跃终端时 agents 项显示禁用态 / 点击提示先创建终端 | 1h | @dev | TODO | F2-2 |
| F2-5 | 测试：agents 点击 → PTY 输入验证、多终端场景切换、无终端场景提示 | 1h | @qa | TODO | F2-3, F2-4 |

## 估时总计

- 实现: 4h (F2-1 + F2-2 + F2-3 + F2-4)
- 测试: 1h (F2-5)
- **合计: 5h**

## 目标完成日期

- 点击 + 插入逻辑: 2026-07-17 (四)
- 反馈 + 边界处理: 2026-07-18 (五)
- 测试通过: 2026-07-19 (六)

## 验收标准

1. 在 agents Tab 中点击任意 agent，活跃终端输入行出现 `@agent-id `
2. 插入后终端自动获得焦点，可以继续输入
3. 多终端场景：点击 agent 后插入到当前活跃终端（而非固定终端）
4. 无活跃终端时：agents 列表项显示禁用态或点击后显示 toast 提示
5. 连续快速点击不同 agent 不产生重复或错乱
6. 与手动输入 `@` 触发 mention 选中的效果一致（都能正确唤起 agent）
