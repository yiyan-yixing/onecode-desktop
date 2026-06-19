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
│     SessionStore (rusqlite) · TrayManager                │
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
│   ├── tauri.conf.json   窗口 1200×800、打包配置
│   ├── capabilities/     IPC 权限白名单
│   └── src/
│       ├── lib.rs        入口：Builder.setup + invoke_handler
│       ├── pty/          🔴 PTY 核心：MultiPtyManager / TerminalSlot / RingBuffer
│       ├── commands.rs   invoke 命令（Channel<Vec<u8>> 版）
│       ├── session/      SQLite 会话持久化（P1 骨架）
│       ├── tray.rs       系统托盘（P1 骨架）
│       └── config.rs     应用配置
├── src/                  前端 WebView
│   ├── index.html        极简骨架
│   ├── styles.css        Cowork 暖色主题
│   ├── ipc-bridge.js     🔴 Tauri invoke + Channel 封装
│   └── terminal/         🔴 TabManager / xterm / 滚动条 / IME
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
make build            # 产出 .dmg (macOS) / .AppImage (Linux)
```

产物位于 `src-tauri/target/release/bundle/`。

## 快捷键

| 快捷键 | 功能 |
|---|---|
| `Cmd/Ctrl + T` | 新建终端 |
| `Cmd/Ctrl + W` | 关闭当前终端 |
| `Cmd/Ctrl + 1~9` | 切换到第 N 个终端 |
| `Cmd/Ctrl + Shift + [ / ]` | 上 / 下一个终端 |

## 里程碑范围（当前：M1）

| 里程碑 | 状态 | 范围 |
|---|---|---|
| **M1** | ✅ 本仓库 | 单终端链路验证 + **多终端 Tab 管理（P0 核心）** + Cowork 暖色主题 |
| **M2** | ⏳ | 健康监控全量 + 资源告警 + 拖拽排序 |
| **M3** | ⏳ | 系统托盘常驻 + 会话持久化恢复 + 远程终端 + 双平台分发 |

P1 模块（`session/`、`tray.rs`、`health.rs` 完整逻辑）当前为**可编译骨架**，含签名与 `TODO(P1)` 标注，待 M2/M3 填充。

## 设计文档（来源）

本仓库基于以下设计文档实施，**架构冲突一律以评审修订为准**：

- `onecode/.claude/blackboard/desktop-prd.md` — 产品需求
- `onecode/.claude/blackboard/desktop-architecture-review.md` — 架构评审（**修订权威**）
- `onecode/.claude/memory/archival/decisions/desktop-client-architecture.md` — 初版架构（已修订）
- `onecode/.claude/memory/archival/decisions/desktop-code-structure.md` — 逐文件代码蓝图
- `onecode/.claude/blackboard/desktop-prototype.html` — UI 原型（Cowork 暖色）

## License

MIT
