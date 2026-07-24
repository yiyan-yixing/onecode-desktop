# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] — 2026-07-23

### Changed — 产品优化（苹果式 P0 审视）

- **定位定稿**：锚句统一为「一人公司的 AI 员工调度台」，README 首段、托盘 tooltip、tauri.conf.json longDescription 三处对齐；消除「以 Claude Code 为内核」与多内核表述的矛盾，统一为「Claude Code / OpenCode / Codex 等，多内核可切换」。
- **重写空状态**：暖色 orb + 中文价值主张 + 「新建终端」主按钮 + 「⌘K 命令面板」次按钮 + 三个最近项目卡片，替代原先冷感的 `⌘K to begin` 英文提示；点击 orb/按钮触发既有爆裂动画创建终端，消除两条入口的歧义。
- **统一视觉语言**：`theme.js` 注释改为如实描述「暖亮外壳 + 局部深色终端井」范式（移除与实现矛盾的「仅暗色」表述）；终端面板左右边缘新增 28px 暖→深渐变软过渡，消除暖色侧栏与深色终端的硬拼接。
- **文案统一中文**：状态栏、palette、wizard 等 UI chrome 文案统一为中文（API Key / Base URL / Model / backend 名等术语保留英文）；`Ready`→`就绪`、`X terminals`→`X 个终端`、`exited`→`已退出`。
- **状态栏瘦身 + 原生菜单栏**：状态栏移除平铺的 4 组快捷键药丸，只留状态点 / 计数 / 健康告警 / CC 徽章；快捷键可发现性迁移至原生应用菜单栏（终端 / 视图 / 窗口 / 帮助），加速键 ⌘T / ⇧⌘T / ⌘W / ⌘B / ⇧⌘F / ⌘1-9 / ⌘Q 与键盘绑定共用同一套动作。

### Added

- `src-tauri/src/menu.rs` — 原生应用菜单栏模块；托盘「退出」与菜单「退出」(⌘Q) 共用优雅退出流程 `quit_app`。
- 快捷键说明弹窗（帮助菜单入口）。
- 空状态最近项目卡片渲染（复用 `list_projects`）。

### Fixed

- 修复 `palette.js` 引用未定义 CSS 变量 `--aurora-overcast` / `--text-void` 导致图标背景 fallback 透明的问题（在 `:root` 补别名映射）。
- 版本号三处不同步（package.json / tauri.conf.json = 0.1.0，CHANGELOG = 0.2.0）统一为 0.3.0。

## [0.2.0] — 2026-07-04

### Added

- Right sidebar Tab switching between file and agents views
- Agents list panel — shows project and global agents with icon, name, @id, description, scope label
- Auto-refresh agents list when switching terminal/project
- Panel collapse/expand preserves last selected Tab via localStorage
- Empty state prompt when no agents are available
- Warm theme styling for Tab bar and agents list (consistent with left sidebar)

### Changed

- File explorer now renders inside the file Tab container (no behavior change, structural refactor)
- Cmd+Shift+F shortcut opens right panel and activates file Tab

## [0.1.0] — 2025-06-28

### Added

- Multi-kernel AI terminal manager — manage Claude Code, OpenCode, Codex, Crush, Aider, Goose, Hermes terminals in one window
- @Mention popup with cursor-position-based placement (auto-flip on overflow)
- Project card right-click context menu (open terminal, show in Finder, delete)
- Custom confirmation dialog (replaces native `confirm()`)
- IPC disconnection detection banner (3+ consecutive failures)
- Palette settings save feedback (check/cross visual indicators)
- Project list loading state
- Ambient controller — statusbar fades after 8s idle
- Sensitive directory protection in file explorer (~/.ssh, ~/.gnupg, etc.)
- Session DB transaction wrapping (crash-safe persistence)
- Generation counter for PTY restart — eliminates race condition
- `recover_lock!` macro — poisoned mutex recovery instead of cascade crash

### Fixed

- @Mention popup rendered at 0,0 (no positioning)
- Project card right-click menu unreachable (`_showProjectCtxMenu` was dead code)
- CSP disabled in release builds
- DevTools open in production builds
- Session DB non-transactional write — data loss on crash
- restart() 80ms sleep race condition — two wait threads could coexist
- 33 `.expect()` on Mutex — thread panic would cascade-crash the app
- `kill_all_blocking` could skip killing terminals when locks contended
- `run_with_timeout` didn't enforce timeout — wizard could hang
- File explorer could read sensitive directories
- Palette orb icon color values broken (raw CSS var without `var()`)
- WebGL addon loaded but never used (dead weight)
- File explorer auto-refreshed even when sidebar collapsed
- Error tab ID collision (Date.now() same millisecond)
- 11 clippy warnings

### Changed

- Product positioning from "Claude Code shell" to "multi-kernel AI terminal manager"
- tauri.conf.json: CSP enabled, devtools disabled in production, publisher/copyright added
- CI now runs `cargo test` and `npm test`
- Release workflow has code signing + notarization placeholder steps

### Removed

- `console-capture.js` (dev-only utility, never loaded in HTML)
