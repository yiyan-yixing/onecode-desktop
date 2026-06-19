# OneCode Desktop

> 原生桌面客户端，一个窗口管理多个 AI 终端。以 Claude Code 为内核，让一人公司开发者在一个窗口内创建、切换、复用多个终端会话。

OneCode Desktop 是 [OneCode](https://github.com/yiyan-yixing/onecode) 的桌面形态。Web 版跑在 Docker 容器里靠浏览器访问，多任务只能开多个浏览器标签——切标签丢现场、标签不持久、无法后台常驻。Desktop 用原生窗口解决：多终端 Tab、托盘常驻、会话配置持久化。

## 为什么是独立仓库

架构评审结论（见 `onecode/.claude/blackboard/desktop-architecture-review.md`）：Desktop 与 Web 版的构建链（Cargo/rustc vs Node/Docker）、CI（macOS runner 打 dmg vs Docker build）、发布节奏完全不同，单仓库耦合会互相干扰。本仓库与 Web 版独立，共享策略为「静态文件直接复制 + 设计文档复用」，不共享 npm/crate。

## 架构

```
┌─────────────────────────────────────────────────────────┐
│                 OneCode Desktop (Tauri v2)               │
│                                                          │
│   Frontend (WebView, Vanilla JS + xterm.js)              │
│     TabManager ── createTab / switchTo / closeTab        │
│        │ onData()  Channel<Vec<u8>> ──► xterm.write()    │
│        │                                                 │
│   ──────┤ invoke (请求/响应)   ├── Channel (流式二进制) ──┤
│        ▼                                                 ▼
│   Rust Backend                                           │
│     commands.rs  ── pty_spawn / kill / write / resize …  │
│     MultiPtyManager (RwLock<HashMap<Uuid, TerminalSlot>>) │
│       ├─ PTY 读取线程 ──► mpsc ──► 16ms 批量合并任务      │
│       └─ RingBuffer (10MB/slot) ──► Tab 切换 replay       │
│     SessionStore (rusqlite) · TrayIcon · 健康检测循环     │
│     CcStatusCache ──► 读 ~/.claude 的 skills/agents/…    │
│                                                          │
│   Frontend (WebView)                                     │
│     CcStatusView (徽章) · MentionController (@弹窗)      │
└──────────────────────────────────────────────────────────┘
```

## 技术栈

| 层 | 选型 | 理由 |
|---|---|---|
| 桌面框架 | **Tauri v2** | ~5-8MB 安装包（Electron ~200MB），WebView 复用 xterm.js |
| 后端语言 | **Rust** | `portable-pty` 跨平台 PTY，无 GC，内存可控 |
| PTY | **portable-pty** | WezTerm 维护，macOS/Linux 一等公民 |
| IPC | **`Channel<Vec<u8>>`** | 原生二进制流式传输，零 base64 编码开销 |
| 并发 | **`RwLock<HashMap>`** | write/resize/list 读锁并发，spawn/kill 写锁低频 |
| 前端 | **Vanilla JS (ES Module)** | 与 Web 版一致，无构建链负担 |
| 终端 | **xterm.js** + fit/web-links/webgl addon | 从 node_modules 同步到 `static/` |
| 持久化 | **rusqlite (bundled)** | 自包含 SQLite，存终端配置（不存内容） |

## 目录结构

```
onecode-desktop/
├── src-tauri/            Rust 后端
│   ├── Cargo.toml        依赖：tauri2 / portable-pty / rusqlite / serde / tokio
│   ├── .cargo/config.toml  Cargo target-dir → ../target（产物统一到项目根目录）
│   ├── tauri.conf.json   窗口 1200×800、打包配置（dmg/appimage/deb/msi/nsis）
│   ├── capabilities/     IPC 权限白名单
│   └── src/
│       ├── lib.rs        入口：Builder.setup + invoke_handler + 启动恢复
│       ├── pty/          🔴 PTY 核心：MultiPtyManager / TerminalSlot / RingBuffer
│       │   ├── mod.rs       RwLock 管理 + snapshot + kill_all_blocking
│       │   ├── slot.rs      TerminalSlot + RingBuffer + SlotStatus
│       │   └── health.rs    僵尸/RSS 健康检测（每 5s 轮询）
│       ├── commands.rs   invoke 命令（Channel<Vec<u8>> + cc_status + health）
│       ├── cc_status.rs  CC Status：读 ~/.claude 的 skills/hooks/agents…
│       ├── session/      SQLite 会话持久化（自动保存 + 启动恢复）
│       ├── tray.rs       系统托盘常驻（新建/显示/退出 + 关窗隐藏）
│       └── config.rs     应用配置
├── src/                  前端 WebView
│   ├── index.html        骨架（标题栏 + Tab 栏 + 终端 + 状态栏徽章）
│   ├── styles.css        Cowork 暖色主题
│   ├── ipc-bridge.js     🔴 Tauri invoke + Channel 封装
│   ├── cc-status.js      状态栏徽章 + agent 数据源
│   └── terminal/         🔴 TabManager / xterm / @mention / 滚动条 / IME
├── static/               运行时同步的 xterm/marked/hljs（.gitignore，生成物）
├── scripts/copy-static.sh  从 node_modules 同步静态资源
└── .github/workflows/    CI（cargo check）+ 发布（dmg/AppImage）
```

## 开发

### 前置依赖

- **Rust** (stable) + `cargo`
- **Node.js** 18+
- **Tauri 系统依赖**：
  - macOS：Xcode Command Line Tools
  - Linux：`webkit2gtk-4.1`、`libgtk-3`、`libayatana-appindicator3` 等（见 [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))
- **Claude Code CLI** 已安装（终端默认启动 `claude --permission-mode bypassPermissions`）

### 首次运行

```bash
make install          # npm install
make copy-static      # 同步 xterm/marked/hljs 到 static/
# 准备应用图标（首次需要）：
#   放一张 ≥1024×1024 的 PNG，执行：
#   npx @tauri-apps/cli icon ./path/to/app-icon.png
make dev              # cargo tauri dev，热重载
```

### 构建

```bash
make build            # 产出 .dmg (macOS) / .AppImage+.deb (Linux) / .msi+.exe (Windows)
```

产物位于 `target/release/bundle/`。

| 平台 | 安装包格式 | 路径 |
|---|---|---|
| macOS | `.dmg` | `target/release/bundle/dmg/` |
| macOS | `.app` | `target/release/bundle/macos/` |
| Linux | `.AppImage` | `target/release/bundle/appimage/` |
| Linux | `.deb` | `target/release/bundle/deb/` |
| Windows | `.msi` | `target/release/bundle/msi/` |
| Windows | `.exe` (NSIS) | `target/release/bundle/nsis/` |

## 快捷键

| 快捷键 | 功能 |
|---|---|
| `Cmd/Ctrl + T` | 新建终端 |
| `Cmd/Ctrl + W` | 关闭当前终端 |
| `Cmd/Ctrl + 1~9` | 切换到第 N 个终端 |
| `Cmd/Ctrl + Shift + [ / ]` | 上 / 下一个终端 |
| 输入 `@` | 触发 Agent @mention 弹窗（↑↓ 选择、Enter 确认、Esc 取消） |

## 功能矩阵（对照 PRD v0.1）

| 需求 | 状态 | 说明 |
|---|---|---|
| P0-1~6 多终端核心 | ✅ | 创建 / 切换 / 关闭 / 重启 / 自动重启 / Claude Code 集成 |
| P1-1 系统托盘 | ✅ | 关窗隐藏到托盘；菜单：新建/显示/退出；退出时 kill 全部 PTY |
| P1-2 会话持久化 | ✅ | create/close/rename 去抖自动落库；启动按配置恢复终端 |
| P1-3 状态指示 | ✅ | Tab 圆点：运行(绿)/退出(灰)/崩溃(红) |
| P1-4 终端重命名 | ✅ | 双击 Tab 标签重命名 |
| P1-5 快捷键 | ✅ | 见上表 |
| P1-6 Agent @mention | ✅ | 弹窗列出来自 `~/.claude/agents` 的 agent，选中插入 `@id` |
| P1-7 CC Status 徽章 | ✅ | 状态栏显示 skills/hooks/plugins/tasks 计数 |
| 健康检测 | ✅ | 每 5s 轮询 pid RSS/僵尸，状态栏告警 |
| P2 远程/分屏/搜索/分组 | ❌ | 不在 v0.1 范围 |

## 里程碑范围（当前：M1 + P1 全量）

| 里程碑 | 状态 | 范围 |
|---|---|---|
| **M1** | ✅ | 单终端链路 + 多终端 Tab 管理（P0 核心）+ Cowork 暖色主题 |
| **P1 全量** | ✅ 本仓库 | 托盘常驻 + 会话持久化 + 健康检测 + CC Status + @mention |
| **M2** | ⏳ | 拖拽排序 + detach 非活跃终端优化 + 资源告警面板 |
| **M3** | ⏳ | 远程终端（WebSocket）+ 双平台正式分发 |

> **编译验证**：已装 Rust 工具链，用 `verify-rust/` 子 crate（`cargo test`，27 通过）通过 `#[path]`
> 纳入**真实生产源码** `cc_status.rs` / `pty/slot.rs` 编译，并复制 `create_pty` 真 spawn 子进程验证
> `portable-pty 0.8.1` 全链路 API。由此发现并修复 4 处 Rust 编译阻塞 bug（见下）+ 1 处 JS bug。
> 仍未覆盖：`tauri`/`tray`/`rusqlite`/`tokio` 相关代码（需系统 GUI 库，本环境无 root 无法 `cargo tauri build`）。
> 前端 8 个 JS 文件 `node --check` 通过，`tests/` 下 153 个纯逻辑/真实模块用例通过（`npm test`）。

### 本轮验证发现并已修复的 Bug

| # | 文件:行 | 问题 | 修复 |
|---|---|---|---|
| 1 | `terminal/mention.js:128` | `_select` 先 `hide()` 清空 prefix 再算 `n`，backspace 恒为 1 | 先捕获 `n` 再 hide |
| 2 | `pty/slot.rs:184` | 对 `Box<dyn MasterPty>` 直接 `write_all`（trait 不 impl `Write`） | 改 `take_writer()` |
| 3 | `pty/mod.rs:320` | `openpty(size, None)` 多传 `None`（0.8 单参） | 去掉 `None` |
| 4 | `pty/mod.rs:347` | `master.spawn(slave, cb)`（0.8 是 `slave.spawn_command(cb)`） | 改 slave.spawn_command |
| 5 | `cc_status.rs:192` | 对 `String` 调 `unwrap_or_else`（`fm_field` 返 `String` 非 `Option`） | 改 `is_empty` 判空 |

## 设计文档（来源）

本仓库基于以下设计文档实施，**架构冲突一律以评审修订为准**：

- `onecode/.claude/blackboard/desktop-prd.md` — 产品需求
- `onecode/.claude/blackboard/desktop-architecture-review.md` — 架构评审（**修订权威**）
- `onecode/.claude/memory/archival/decisions/desktop-client-architecture.md` — 初版架构（已修订）
- `onecode/.claude/memory/archival/decisions/desktop-code-structure.md` — 逐文件代码蓝图
- `onecode/.claude/blackboard/desktop-prototype.html` — UI 原型（Cowork 暖色）

## License

MIT
