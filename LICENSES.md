# OneCode Desktop 第三方依赖许可清单

> 用途：记录 onecode-desktop 依赖的第三方软件及其许可证归属，确保对外分发合规。
> 更新：2026-08-09（@vp-eng 审计；npm 侧已从本地 node_modules 核实，Cargo 侧为 crates.io 标准许可）。
> 说明：本文件为工程记录，非法律意见；重大商业化分发前请 @clo 复核。

## 框架

| 组件 | 许可证 | 用途 |
|---|---|---|
| **Tauri v2**（tauri） | MIT OR Apache-2.0 | 桌面应用框架（Rust + Web 前端） |
| **@tauri-apps/cli** | MIT OR Apache-2.0 | 构建/开发 CLI |
| **tauri-plugin-shell** | MIT OR Apache-2.0 | 进程/命令调用 |
| **tauri-plugin-dialog** | MIT OR Apache-2.0 | 原生对话框 |

## npm 依赖（本地 node_modules 已核实）

| 包 | 许可证 | 用途 |
|---|---|---|
| **@xterm/xterm** | MIT | 终端渲染 |
| **@xterm/addon-fit** | MIT | 终端自适应尺寸 |
| **@xterm/addon-web-links** | MIT | 终端链接点击 |
| **@xterm/addon-webgl** | MIT | 终端 WebGL 加速 |
| **marked** | MIT | Markdown 渲染（对话/AI 输出） |
| **highlight.js** | BSD-3-Clause | 代码高亮 |

## Rust / Cargo 依赖（crates.io 标准许可）

| 包 | 许可证 | 用途 |
|---|---|---|
| **portable-pty**（WezTerm 维护） | MIT | 跨平台伪终端 |
| **rusqlite** | MIT | SQLite 会话持久化（bundled） |
| **serde / serde_json** | MIT OR Apache-2.0 | 序列化 / IPC |
| **uuid** | MIT OR Apache-2.0 | 会话 ID |
| **chrono** | MIT OR Apache-2.0 | 时间处理 |
| **tokio** | MIT | 异步运行时 |
| **base64** | MIT OR Apache-2.0 | 文件预览编码 |
| **anyhow / thiserror** | MIT OR Apache-2.0 | 错误处理 |
| **log / env_logger** | MIT OR Apache-2.0 | 日志 |

## 合规注意

- 本仓库为**纯自研外壳**（Tauri + xterm + portable-pty），未基于 VSCode/IntelliJ 任何代码，当前无可合规风险。
- 引入 Monaco 编辑器组件（规划中）时：Monaco 独立 **MIT**，组件级嵌入无附加义务，保留版权声明即可。
- 若复用 VS Code 扩展生态（未来选项），走 Open VSX 类渠道，不接微软官方 Marketplace。
