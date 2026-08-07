# onecode-desktop 自动切换模型 — 移植 cc-switch 设计草案

> 状态：**draft**（待 @architect 评审 + 董事长拍板） · 日期：2026-08-08
> 背景：董事长要求「将 ccswitch 的逻辑加到这个产品来」——让 onecode-desktop 具备**多供应商/模型切换 + 自动切换（failover）**能力，落点原型「自动切到 DeepSeek-V4-Flash」。
> 参考源码：`farion1231/cc-switch`（**MIT**，已 clone 到 `/tmp/cc-switch`，可移植，需保留版权声明）。

## ✅ 董事长决策记录（2026-08-08）
- **目标方案 = 方案③（全量）**：多供应商切换 + 重启式 failover + 代理层 failover（请求级无缝）。
- **当前阶段 = 只做产品交互设计，不实现**。实现等交互设计定稿后再走 @architect → @vp-eng → @dev。
- **预置试点 = 两档**：DeepSeek-V4-Flash + GLM-5.2。后续按需扩。
- **外观方向 = A+B**：借 VS Code 设计语言（Monaco 编辑内核 + 色板/主题/codicon + 操作范式）+ 与 VS Code 双向轻桥接（编辑↔打开↔配置同步）。已并入 `auto-switch-interaction-design.md` §2.5。
- 配套产出：`auto-switch-interaction-design.md`（产品交互设计）。

---

## 1. Recon 结论

### 1.1 onecode-desktop 现状（落点）
- 本质：**Claude Code / Codex CLI 的桌面壳**。`backend.rs` 把 `api_key/base_url/model` 映射为 `ANTHROPIC_*` / `OPENAI_*` 环境变量，注入给 spawn 的 CLI（`npm install -g @anthropic-ai/claude-code`）。
- 配置：`~/.onecode/desktop.json` 单份配置（`api_key / base_url / model / default_backend`），`ConfigManager` 支持前端 IPC 热更新，另有外部变更热加载（每 2s 轮询）。
- **现状是「单供应商」**：改模型 = 改 config + 重启后端会话。默认 `api.anthropic.com` + `claude-sonnet-4-6`。
- → **Layer A 的地基已在**（config + env 注入），只缺「多供应商/profile 管理 + 切换 UI」。

### 1.2 onecode-desktop 的网关
- **OneCode IDE gateway（:7681）不是 LLM 网关**——只是 Web IDE 入口（HTML + 反代 filebrowser/code-server + 终端 WebSocket），零模型路由能力。
- → 架构岔路**方案 2（复用 IDE 网关）证伪**，唯一路径 = **desktop 内置自己的切换层**。
- 董事长自己的 Claude Code 侧网关 `oneapi-comate` 有 21 个模型（GLM-5.x / DeepSeek-V4-Flash·Pro / Claude 系 / GPT-5.x / Kimi / MiniMax / grok）。desktop 产品应对接**任意 OpenAI/Anthropic 兼容端点**，并预置这套供应商。

### 1.3 cc-switch 的「切换逻辑」= 两层
| 层 | 能力 | 核心源码（cc-switch, src-tauri/src/） |
|---|---|---|
| **Layer A 配置切换** | 多 provider + profile，一键改写目标工具配置（Claude Code 即改 `~/.claude/settings.json` 的 ANTHROPIC_* / env） | `provider.rs`(1252)、`commands/provider.rs`(1252)、`database/dao/providers.rs`(828)、`database/dao/profiles.rs`(207)、`app_config.rs`、前端 `*ProviderPresets.ts` + `components/providers|profiles/` |
| **Layer B 自动切换 failover** | 本地代理 + 熔断 + 有序故障转移队列（P1→Pn）+ 热切换，失败自动切备用供应商 | `proxy/`（server 405 / handlers / provider_router 523 / circuit_breaker 495 / failover_switch 135 / providers/ 适配器 / model_mapper）、`commands/failover.rs`(182)、`database/dao/failover.rs`(149) |

**Layer B 关键机制（已读源码确认）**：
- 熔断器按 `app_type:provider_id` 独立：默认连续失败 4 次开闸，成功 2 次半开恢复，错误率阈值 0.6。
- `provider_router::select_providers()`：auto_failover 开启时按队列取供应商、跳过熔断开闸的；全部开闸才回退普通路由。
- failover 队列存 `providers.in_failover_queue`；开启开关时若队列空自动把当前供应商作为 P1。
- 切换成功后 `FailoverSwitchManager` 热切换 + 更新托盘 + 发 `provider-switched` 事件。

---

## 2. 三层移植方案（按成本递增）

### 方案 1 — Layer A：多供应商/模型切换（MVP，推荐先做）
- **做什么**：在 desktop 加「供应商管理 + profile 预设 + 一键切换」。
  - 数据层：Provider/Profile 表（JSON 文件即可，不必上 SQLite——desktop 目前是单 JSON）。
  - 切换逻辑：切 = 改写 `desktop.json`（api_key/base_url/model）+ 重启当前后端会话，复用现有 ConfigManager/热加载。
  - 预置：内置董事长网关的 21 模型供应商预设（GLM-5.2 / DeepSeek-V4-Flash / DeepSeek-V4-Pro / Claude 系 / GPT 系…）。
  - UI：设置面板加「供应商/模型」下拉 + 管理入口。
- **移植来源**：cc-switch `database/dao/providers.rs`（简化为 JSON）、`provider.rs` 的 env 构造、前端 provider presets。
- **成本**：中低。高复用 desktop 现有 config/backend。约 3–5 个后端文件 + 1 个 UI 面板。
- **交付价值**：用户在 app 内一键切 GLM-5.2 ↔ DeepSeek-V4-Flash ↔ Claude，**直接实现「切到 deepseek v4 flash」**。无自动 failover。

### 方案 2 — Layer A + 重启式自动切换（v1「自动切换」，成本/复杂度折中）
- **做什么**：在方案 1 之上，加「**故障自动换供应商**」——但不引入代理，而是**监听后端进程失败**（退出码/错误特征），自动把 config 切到 failover 队列下一个供应商、**重启后端会话**、重挂 PTY。
- **为什么可行**：desktop 架构是「spawn CLI + env 注入 + 会话/PTY 管理」，重启式 failover 契合现状，无需代理层。
- **移植来源**：cc-switch `commands/failover.rs`（队列/开关逻辑，去掉 proxy 依赖）+ `database/dao/failover.rs`。
- **成本**：中。主要是会话重启 + PTY 重挂（desktop 已有 session/pty 模块）+ 失败检测。
- **局限**：请求级 failover 做不了（失败请求会断，需重启会话）；但作为「自动切换」v1 足够。

### 方案 3 — Layer A + 完整 failover proxy（v2 全量，最重）
- **做什么**：desktop 内跑本地代理（127.0.0.1），后端 CLI 的 `base_url` 指向本地代理，代理做**请求级路由 + 熔断 + 流式转发 + 模型映射**，失败请求无缝切下一个供应商。
- **移植来源**：cc-switch `proxy/` 模块（~30 文件，含 claude/codex/gemini 等适配器、streaming、model_mapper、usage 计量）+ circuit_breaker + provider_router + failover_switch。
- **成本**：高。全新 Rust 模块，约 8–15 个文件迁入 + 大量适配调优。是 cc-switch 最复杂、也是「产品护城河」最强的一块。
- **价值**：请求级无缝 failover（不打断对话）+ 多供应商聚合 + 后续可做用量/成本计量。

---

## 3. 推荐路线
1. **MVP = 方案 1（Layer A 多供应商切换）** —— 先让用户在 app 内能切到 DeepSeek-V4-Flash / GLM-5.2 / 任意供应商。地基建好，也是方案 2/3 的必然前提。
2. **v1 = 方案 2（重启式自动 failover）** —— 满足「自动切换」字面需求，成本可控。
3. **v2 = 方案 3（完整 proxy failover）** —— 产品护城河，按需/按资源再上。
4. 全程 **@architect 把关架构**（新增切换层属架构决策，董事长签字）→ @vp-eng PRD → @dev 实现 → @qa 验证。

## 4. 合规
- cc-switch **MIT**：可移植/改造，需保留版权声明（在每个移植文件头注明来源 + 附 LICENSE 副本），@clo 复核。
- 本 repo 内不直接全量搬 proxy 之前，@clo 确认 attribution 写法。

## 5. 待董事长拍板项
1. **MVP 范围**：只做方案 1，还是 1+2，或直接 1+2+3？
2. **默认接入**：desktop 预置供应商是否就按「董事长网关 oneapi-comate 的 21 模型」为默认，还是只预置 DeepSeek-V4-Flash + GLM 两档试点？
3. **切换生效方式**：改 config + 重启后端会话（简单）可接受吗？还是必须请求级无缝（即方案 3）？
4. **本轮配额事件**（网关 403 月额度用完导致全公司 agent 停摆）是否顺带推动「desktop 多供应商」作为风险对冲——即是否在本期就列入 1 的验收标准。
