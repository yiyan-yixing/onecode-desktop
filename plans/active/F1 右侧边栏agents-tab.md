# F1: 右侧边栏增加 Tab 切换 — [ file | agents ]

> **类型:** 功能 | **优先级:** 中 | **状态:** 🟡 排期中
> **目标完成:** 2026-07-15 (二)

## 需求描述

右侧边栏（当前只有文件浏览器）增加 Tab 切换条：
- `[ file | agents ]` 两个 Tab
- `file` Tab 为原有文件浏览器（保持现有功能不变）
- `agents` Tab 读取 `.claude/agents/` 中的 agent 定义并以列表展示

## 代码定位

- **右侧面板:** `src/index.html` line 57 — `<aside class="sidebar sidebar-right collapsed" id="filePanel">`
- **文件浏览器:** `src/file-explorer.js` — `FileExplorerController`
- **Agents 数据源:** `src/cc-status.js` — `CcStatusView` 已有 `agents` 列表
- **Rust agents 加载:** `src-tauri/src/cc_status.rs` — `load_agents()` 从 `.claude/agents/*.md` 读取
- **面板切换:** `src/main.js` line 148-162 — `rightPanelToggle` 事件处理
- **样式:** `src/styles.css` — `.sidebar` / `.sidebar.collapsed`

## 设计方案

1. 在 `#filePanel` 内顶部添加 Tab 栏：`<div class="fe-tabs"><button data-tab="file">file</button><button data-tab="agents">agents</button></div>`
2. `file` Tab 内容：保留现有 `FileExplorerController` 的 DOM（toolbar + tree + preview）
3. `agents` Tab 内容：新建 `AgentsListController`，渲染 agents 列表
4. Tab 切换时隐藏/显示对应面板，不销毁 DOM（保持状态）
5. 复用 `CcStatusView` 的 `agents` 数据（已通过 `onAgents` 回调提供给 `TabManager.agentProvider`）

## 子任务

| 编号 | 任务 | 估时 | 负责人 | 状态 | 依赖 |
|------|------|------|--------|------|------|
| F1-1 | 右侧边栏 Tab 栏 UI：设计并实现 [ file \| agents ] Tab 切换条（HTML + CSS） | 3h | @dev | TODO | — |
| F1-2 | Tab 切换逻辑：点击 Tab 切换显示对应面板内容，切换时保持各 Tab 状态 | 2h | @dev | TODO | F1-1 |
| F1-3 | File Tab 适配：将现有 FileExplorerController 的 DOM 包裹进 file Tab 容器，确保功能不变 | 2h | @dev | TODO | F1-2 |
| F1-4 | Agents Tab 数据层：复用 CcStatusView 的 agents 数据，确保切换项目时自动刷新 | 2h | @dev | TODO | F1-2 |
| F1-5 | Agents Tab UI 列表：渲染 agents 列表（icon + name + description + scope 标签），新建 AgentsListController | 3h | @dev | TODO | F1-4 |
| F1-6 | Tab 状态持久化：记忆上次选中的 Tab，下次打开面板时恢复 | 1h | @dev | TODO | F1-3, F1-5 |
| F1-7 | 样式适配：Tab 栏 + Agents 列表适配 warm 主题（与左侧 sidebar 风格一致） | 1h | @dev | TODO | F1-5 |
| F1-8 | 测试：Tab 切换正常、Agents 列表正确渲染、面板折叠/展开不影响 Tab 状态 | 1h | @qa | TODO | F1-6, F1-7 |

## 估时总计

- UI 实现: 5h (F1-1 + F1-5 + F1-7)
- 逻辑实现: 7h (F1-2 + F1-3 + F1-4 + F1-6)
- 测试: 1h (F1-8)
- **合计: 13h**

## 目标完成日期

- Tab UI + 切换逻辑: 2026-07-11 (五)
- File Tab 适配 + Agents 数据层: 2026-07-13 (日)
- Agents UI + 样式: 2026-07-15 (二)
- 测试通过: 2026-07-15 (二)

## 验收标准

1. 右侧面板打开后，顶部显示 Tab 栏：`[ file | agents ]`
2. 点击 `file` Tab 显示文件浏览器，功能与改动前完全一致
3. 点击 `agents` Tab 显示当前项目 + 全局的 agents 列表
4. 每个 agent 项显示：图标（icon 或首字母）、名称、@id、描述、scope 标签
5. 切换终端/项目时，agents 列表自动刷新为新项目的 agents
6. 面板折叠再展开时，保持上次选中的 Tab 和对应内容
7. 无 agents 时显示空状态提示
8. Tab 栏和列表风格与左侧 sidebar warm 主题一致
