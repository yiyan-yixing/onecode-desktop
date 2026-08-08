# ADR · onecode-desktop 模型自动切换 — 移植 cc-switch 架构评审

> **日期**：2026-08-08
> **产出主理**：@architect
> **评审对象**：`auto-switch-design.md`（技术方案 draft）+ `auto-switch-interaction-design.md`（交互 v0.1）
> **状态**：✅ **架构方案通过（含 2 项前置裁定 + 6 项董事长签字点）** — S1-S6 已签（2026-08-08），设计链闭环，待 @vp-eng 出 PRD
> **签字记录**：6/6 全部签字（2026-08-08，董事长）——S1 两阶段推进 / S2 代理层触发条件 / S3 axum+reqwest / S4 providers.json / S5 key 安全模型 / S6 JSON 持久化
> **评审方式**：两路独立 subagent deep-read —— ① cc-switch proxy 层深度评审（forwarder/circuit_breaker/model_mapper/provider_router）；② HTTP 栈选型 + 接口边界评审（hyper/axum vs 重启式）。本 ADR 为最终裁定。
> **级联追踪**：cascade-20260808-model-auto-switch

---

## 0. 董事长已拍板（不可改，本 ADR 以此为基准）

| # | 拍板项 | 内容 |
|---|--------|------|
| P1 | 目标方案 | **方案③ 全量**：多供应商切换(Layer A) + 重启式 failover + 代理层 failover(请求级无缝) |
| P2 | 当前阶段 | **设计定稿，不实现**（本 ADR 是设计链一环，不是开工令） |
| P3 | 预置试点 | 两档：DeepSeek-V4-Flash + GLM-5.2 |
| P4 | 外观 | A+B：VS Code 设计语言 + 与 VS Code 双向轻桥接 |
| P5 | 参考 | `reference/cc-switch`（MIT，73M gitignore 本地参考） |

> 本 ADR 不推翻以上任何一项。`§7 签字点` 中提出的「两阶段推进」是**在方案③全量目标内的实施顺序裁定**，不改变 P1 目标本身。

---

## 1. 架构上下文

### 1.1 onecode-desktop 现状（落点）

- **本质**：Claude Code / Codex CLI 的桌面壳。`backend.rs` 把 `api_key/base_url/model` 映射为 `ANTHROPIC_*`/`OPENAI_*` env，注入 spawn 的 CLI 子进程。
- **配置**：`~/.onecode/desktop.json` 单份扁平配置（`AppConfig`），`ConfigManager`（Arc<RwLock> + 2s watcher）支持前端 IPC 热更新 + 外部变更热加载。
- **会话**：`commands.rs pty_spawn` → `pty/mod.rs create_pty`（env 注入）+ `pty.restart`（已存 `SlotConfig` env 快照，指数退避自动重启）。
- **现状单供应商**：改模型 = 改 config + 重启后端会话。
- **零 HTTP 依赖**：当前 `Cargo.toml` 无任何 HTTP server / client 依赖（tokio 已有）。

### 1.2 cc-switch 参考（MIT，`reference/cc-switch`）

- **Layer A 配置切换**：`provider.rs` + `commands/provider.rs` + `database/dao/providers.rs|profiles.rs` + `app_config.rs` + 前端 presets。
- **Layer B 代理 failover**：`proxy/`（~30 文件）— server/handlers/forwarder/circuit_breaker/provider_router/failover_switch/providers 适配器/model_mapper + `commands/failover.rs` + `database/dao/failover.rs`。
- **HTTP 栈**：axum 0.7（server Router）+ hyper 1.0 full + hyper-util（裸 HTTP/1.1 accept loop）+ reqwest 0.12（上游池化客户端）+ rustls + tower。

### 1.3 本次专项缺口补全结论（@ceo 遗留，现已完成）

两路 subagent deep-read 均未遇配额限制。核心发现（详见 §3/§4/§6）：
1. **proxy 层移植可行性 = 可行**，工作量集中在三处：DB→内存 store 抽象（P0）、`forward()` 的 OAuth/Codex 分支裁剪（P0）、Provider 配置映射（P1）。纯 HTTP/流式/熔断/模型映射四块全部自包含，可脱离 Tauri 作为纯 tokio/axum server 运行。
2. **HTTP 栈**：cc-switch 的 hyper 裸写路径（`preserve_header_case`）是给严格指纹网关用的**非通用需求**，desktop 可先用 axum + reqwest，不引入 hyper-util/hyper-rustls 裸路径。
3. **接口边界**：新增 `providers.json` 独立于 `desktop.json`，ConfigManager / ProviderStore 双单一真相，`commands.rs pty_spawn` env 注入链路**零改动**。

---

## 2. 目标架构图（方案③ 全量）

```
┌────────────────────────────────────────────────────────────────┐
│  onecode-desktop 进程                                             │
│                                                                    │
│  ┌────────────┐   IPC    ┌──────────────────────────────┐          │
│  │  前端 UI     │◄───────►│  commands.rs（Tauri command） │          │
│  │ 模型芯片/面板 │          └──────────────┬───────────────┘          │
│  └────────────┘                     │                            │
│                                     ▼                            │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │               SwitchManager（新增·编排核心）                │    │
│  │  ① 写激活态→AppConfig(desktop.json)  ② 写目录→providers.json │    │
│  └──────────────┬──────────────────────┬─────────────────────┘    │
│                 │                      │                          │
│    ┌────────────▼──────────┐  ┌────────▼────────────────┐         │
│    │ ConfigManager(现有)    │  │ ProviderStore(新增)      │         │
│    │ 激活配置单一真相          │  │ 目录单一真相              │         │
│    │ desktop.json +2s watcher│  │ providers.json          │         │
│    └────────────┬──────────┘  │ Vec<Provider>+failover_queue│      │
│                 │             └────────┬────────────────┘         │
│                 ▼                      │                          │
│  ┌─────────────────────────┐           │                          │
│  │ commands.rs pty_spawn    │◄──────────┤ (读激活 provider creds)   │
│  │ env 注入(backend.rs map) │           │                          │
│  │ —— 零改动 ——             │           │                          │
│  └────────────┬────────────┘           │                          │
│               ▼                        │                          │
│  ┌─────────────────────────────────────▼─────────────────────┐   │
│  │  方案②重启式 failover（Phase 1 落地）                       │   │
│  │  PTY 输出解析→失败特征→切 failover 队列→pty.restart+刷新 env │   │
│  └─────────────────────────────────────┬─────────────────────┘   │
│                                        │                          │
│  ┌─────────────────────────────────────▼─────────────────────┐   │
│  │  方案③代理层 failover（Phase 2，半可逆·需董事长先签）        │   │
│  │  本地 HTTP server(axum 127.0.0.1) + forwarder + 熔断       │   │
│  │  + 流式转发 + model_mapper —— CLI base_url→本地代理          │   │
│  └──────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────┘

数据流（Phase 2 后）：
CLI → 127.0.0.1:port ─(forwarder)─► oneapi-comate 网关 ─► GLM-5.2 / DeepSeek-V4-Flash
      │        ▲                                            
      │        └ 熔断/路由/模型映射/流式(Phase 2)
```

---

## 3. 移植模块清单【提取 / 裁剪 / 新建】

> 依据：cc-switch proxy 深度评审（subagent A）。预估最小模块集约 **20 个文件**，其中 **14 个近乎原样提取**。

### 3.1 提取（直接搬，改动 ≤20%）

| 模块 | 依赖 | 改动点 |
|---|---|---|
| `proxy/circuit_breaker.rs` | log/tokio/serde + log_codes | 无，独立（自带 5 单测可搬） |
| `proxy/error.rs` + `error_mapper.rs` | axum IntoResponse | 裁 DB 变体、删未用变体 |
| `proxy/session.rs` | axum::http + uuid | 可裁 codex/grokbuild 分支 |
| `proxy/switch_lock.rs` | tokio | 无，纯 tokio mutex map |
| `proxy/sse.rs` | 无 | 无 |
| `proxy/hyper_client.rs` | hyper 1.0 + rustls + httparse | 仅 stub 日志用 `mask_url`（1 处） |
| `proxy/providers/adapter.rs` | Provider 结构 | 无（trait + auth_header_value） |
| `proxy/providers/auth.rs` | 无 | 裁 GoogleOAuth/GitHubCopilot/CodexOAuth/XaiOAuth（保留枚举扩展位） |
| `proxy/providers/models/anthropic.rs` / `openai.rs` | serde | 无，纯数据模型 |
| `proxy/providers/streaming.rs` | sse.rs + bytes/futures | 无（OpenAI→Anthropic SSE 转换，自包含） |
| `proxy/providers/transform.rs` | json_canonical + tool_media + ProxyError | 无 |
| `proxy/model_mapper.rs` | Provider + 常量 | inline 掉 `ONE_M_CONTEXT_MARKER`（自带 19 单测可搬） |
| `proxy/providers/claude.rs`（子集） | Provider + auth | 只留 Anthropic/ClaudeAuth/Bearer 策略；删 Copilot/CodexOAuth/XaiOAuth/Gemini 分支 |
| `proxy/types.rs`（子集） | serde | 留 ProxyConfig/ProxyStatus/AppProxyConfig；删 Gemini/Copilot 专属字段 |
| `proxy/json_canonical.rs` / `tool_media.rs` / `content_encoding.rs` / `body_filter.rs` | — | transform 链路必需，独立 |

### 3.2 裁剪（大幅删减或本地化）

| 模块 | 结论 |
|---|---|
| `proxy/forwarder.rs`（207KB） | **核心移植件**。保留 `forward_with_retry_inner` + `forward()` 的 Claude 透传 + Claude→OpenAI Chat 转换；删 Codex Responses 分支、Copilot/Codex/Xai OAuth 动态取 token、gemini_native、copilot_optimizer。`app_handle` 换 Notifier trait |
| `proxy/handlers.rs`（137KB） | 保留 `handle_messages` + `handle_chat_completions` + `/models`；删 claude-desktop 3P gateway、grokbuild、gemini、usage 日志（依赖 DB 常量） |
| `proxy/server.rs` | 复用 manual hyper accept loop + preserve_header_case 模式；**ProxyState 重建**：去 db/`Option<AppHandle>`/gemini_shadow，换内存 store |
| `proxy/handler_context.rs` | 复用，DB 调用换内存 store |
| `proxy/provider_router.rs` | DB 紧耦合，本地化重建约 150 行（见 §4 裁定） |
| `proxy/failover_switch.rs` | 删 Tauri Emitter/托盘，用 Notifier no-op 桩 |
| `proxy/response_processor.rs` | 留 `strip_hop_by_hop` + passthrough stream；删 usage collector/logger |
| `proxy/thinking_rectifier.rs` | **建议保留**（Claude Code 打非 Anthropic 端点必遇 thinking signature 错误）；thinking_budget + media 可先裁 |
| `proxy/providers/mod.rs` | 裁 ProviderType 到 Claude/ClaudeAuth/OpenRouter/Bearer 子集 |
| `proxy/http_client.rs` | reqwest 全局客户端，可整搬或只搬 `mask_url` stub |

### 3.3 新建（onecode-desktop 侧）

| 模块 | 说明 |
|---|---|
| 内存 `ProviderStore` | 替代 DB：镜像 router 的 7 个 DAO 方法（get_proxy_config_for_app / update_proxy_config_for_app / get_all_providers / get_failover_queue / get_current_provider / get_provider_by_id / update_provider_health_with_threshold）|
| 瘦身 `ProxyState` | config/status RwLock + current_providers HashMap + Arc<ProviderRouter> + Option<Notifier> |
| HTTP 装配层 | axum Router 只挂 `/v1/messages`、`/claude/v1/messages`、`/v1/chat/completions`、`/v1/responses`、`/models`、`/health`；`ProxyServer::start/stop` |
| `Notifier` trait | `provider-switched` → 桌面事件桥；MVP 可为空 |
| Provider 配置映射 | 桌面 env 注入 ↔ cc-switch `Provider.settings_config.env` 双向转换器，含 failover 队列顺序 |

---

## 4. 四个核心文件的独立化裁定（@ceo 专项缺口 #1 结论）

| 文件 | 裁定 | 依据 |
|---|---|---|
| `circuit_breaker.rs` | ✅ **完全独立，直接复用** | 仅依赖 log/tokio/serde；参数经纯结构体注入（默认 4 连败/错误率 0.6/半开 2 成功/min 10 次/超时 60s）；自带 5 单测。per-app 注入点从 DB 改内存 store |
| `model_mapper.rs` | ✅ **独立，可脱离 DB** | 唯一外部依赖是 `Provider` 结构（纯数据）+ 常量（inline 即可）；自带 19 单测 |
| `provider_router.rs` | ⚠️ **逻辑可复用，DB 紧耦合，不能原样独立** | `select_providers` 读 5 个 DAO、`record_result` 写 1 个 DAO。必须抽象成 6-7 方法 store trait，否则本地化重建 ~150 行。**建议：提取 trait + 保留原有 5 个 router 测试作内存实现回归基线** |
| `forwarder.rs` | ⚠️ **核心可移植，需裁剪** | 对 Anthropic+OpenAI 兼容端点最小子集 = `forward_with_retry_inner` + `forward()` 的 Claude 透传 + Claude→OpenAI Chat 两条路径。必须剥离 OAuth 三态查找（换 `AuthProvider` trait）、FailoverSwitch（换 Notifier no-op）。**建议整函数搬入再注释裁剪分支，而非重写**（避免破坏 URL 重写与 header 保真交互） |

---

## 5. 接口边界（@ceo 专项缺口 #2 结论）

### 5.1 三个文件、三个单一真相、互不侵入

| 层 | 文件 | 单一真相 | 管什么 |
|---|---|---|---|
| 运行时激活配置 | `~/.onecode/desktop.json`（现有 `AppConfig`，**加一个字段** `active_provider_id: Option<String>`） | `ConfigManager`（现有 Arc<RwLock> + 2s watcher 不变） | `api_key/base_url/model/default_backend` + 当前激活 provider 指针 |
| 供应商目录 | **新文件 `~/.onecode/providers.json`** | 新 `ProviderStore`（Arc<RwLock<ProviderCatalog>>） | `Vec<Provider>` + `failover_queue: Vec<provider_id>` + `auto_failover_enabled` |
| 静态 profile | `backend.rs`（现有，**不动**） | 静态 `PROFILES` | env_key_map（config 键→env 变量） |

**为什么独立 providers.json，而不是在 desktop.json 里长 providers 列表**：
- `AppConfig` 是扁平单供应商结构，被 `ConfigUpdate.apply_to` / `get_config_schema` / `pty_spawn` / watcher 全链路消费。塞 `Vec<Provider>` 会让它变成「激活态+目录」混合体，所有消费者和前端 schema 跟着改，还让「目录」（静态大量数据）与「激活选择」（少量运行时态）两个生命周期缠在一起。
- 切换层与运行时配置是两个关注点：目录归 ProviderStore 管，激活 creds 归 ConfigManager 管。

### 5.2 激活路径（零迁移，pty_spawn 零改动）

```
用户选 Provider / failover 切档
  → SwitchManager: 该 provider 的 api_key/base_url/model 写进 AppConfig（走现有 save_config 路径）
  → 同时 active_provider_id + failover_queue + enabled 写进 providers.json
  → commands.rs pty_spawn 照旧读 config.api_key/base_url/model → env 注入（一行不改）
```

### 5.3 实现时两个关键坑（subagent B 已标出）

1. **`pty.restart` 复用 slot 旧 env**：切档必须**显式刷新 slot.env**（改 config 后 restart 不会自动带新 env，需在 failover 流程里重取 config 覆盖 env）。
2. **失败检测不能只看退出码**：要解析 PTY 输出里的 403/鉴权/额度特征，才能区分「该切」与「不该切」（如用户主动取消不触发 failover）。

---

## 6. 风险与规避（proxy 层移植）

### P0（阻断级）

| 风险 | 规避 |
|---|---|
| **DB→内存 Store 抽象失真**：ProviderRouter/handler_context/handlers 直接调 DAO。failover 队列排序 + per-app 熔断参数 + `update_provider_health_with_threshold` 是熔断/切换语义核心，抽象错了会静默破坏故障转移 | 定义最小 trait 严格镜像 7 个 DAO 方法；把 cc-switch 自带 router 测试（5 个）搬到内存实现上回归 |
| **OAuth 分支与 auth 头注入交织**：`forward()` 里 Copilot/CodexOAuth/XaiOAuth token 获取深嵌转发主流程（forwarder.rs:1309/1626/1675/2585） | 移植时把「认证获取」抽成 `AuthProvider` trait（API-key 实现 + 预留 OAuth 实现位），`forward()` 只调 trait |

### P1（高影响，可绕）

| 风险 | 规避 |
|---|---|
| **Header-case 保真链路**：server.rs TCP peek + OriginalHeaderCases + hyper_client raw write 是 wire 级保真 | `hyper_client.rs` + server.rs accept loop **原样搬运**（自包含），不裁剪 |
| **OpenAI→Anthropic SSE 转换边界 case**：message_delta 去重、pending usage 延迟、tool_use delta 拼接、无限空白中止 | `streaming.rs` + `sse.rs` **一字不改提取**，连带依赖测试 |
| **Provider 配置模型错配**：桌面 env 注入 vs cc-switch `settings_config.env` 对象 | 定义明确双向映射，复用 `ClaudeAdapter::extract_auth` 推断逻辑而非重写 |

### P2（低影响，可暂缓）

- failover_switch/tray/事件：Notifier no-op 桩先上，后续接线。
- thinking_budget/media 裁剪：至少保留 `thinking_rectifier.rs`，或文档标注已知限制。
- usage/计费日志：依赖 DB 常量，MVP 裁掉，成本归因缺失需明示。

---

## 7. 可逆性分级（本 ADR 核心裁定）

> 分级标准：**可逆** = 1 天内拆除、零残留、无架构依赖；**半可逆** = 可拆除但有代价/有单向门特征；**不可逆** = 拆除等于产品推倒重来。

| 架构块 | 分级 | 依据 | 拆除成本 |
|---|---|---|---|
| **Layer A：多供应商切换 + providers.json** | 🟢 **可逆** | 纯增量：config.rs 只加 `active_provider_id` 一个字段，providers.json 是新增文件，env 注入链路零改动 | 1 天：删 providers.json + 移除 active_provider_id，回退现状 |
| **重启式 failover（方案②）** | 🟢 **可逆** | 小型状态机复用现有 PTY 重启基建；`auto_failover_enabled=false` 即回单供应商 | 1 天：关开关即回退，删状态模块即可 |
| **代理层 failover（方案③）** | 🟡 **半可逆（半单向门）** | 见下 | 1 周内可拆，越拖越重 |

### 7.1 代理层「半可逆」拆解

**为什么是半单向门**：
1. **base_url 永久指向本地代理** → 所有 CLI 流量改走新层。拆 = 重新指向真实网关 + 移除新依赖/模块。代理有 bug 则所有 CLI 流量全断。
2. **引入 HTTP 栈**（axum + reqwest + rustls）→ 新增供应链面 + 编译时长 + 本机网络攻击面（任何本机进程可打 127.0.0.1:port）。
3. **功能沉淀加速不可逆**：一旦用量/成本计量、OAuth、格式转换边界 case 在代理层累积，产品形态开始依赖它 → 「重新直连网关」从技术回退变成**产品回退**。

**保留的逃生舱（使其维持「半可逆」而非滑向「不可逆」）**：
- `providers.json` 保持纯 JSON、不引入 SQLite → 数据可移植。
- 激活路径保持「写 desktop.json + env 注入」不变 → base_url 可随时改回真实网关。
- 代理层作为**独立可开关模块**（`ProxyServer::start/stop`），开关关闭即回直连。

**滑向不可逆的触发点（红线）**：
- 代理成为唯一流量路径且用量计费/成本归因**只**依赖代理遥测；
- 或 OAuth 接入后 key 管理彻底迁移出 env 体系。

> 这两条红线发生前，代理层必须在 1 周内可拆。建议把「可拆性」写入 Phase 2 的验收标准。

### 7.2 半不可逆的次要点（本 ADR 范围外）

- **Monaco 编辑内核 + VS Code 桥接（外观 A+B，P4 已拍板）**：Monaco 是大体积 JS 依赖，属另一 ADR 范围。本 ADR 仅标注：auto-switch 的模型芯片/F3/F4 视觉按 VS Code token 实现，与 Monaco 无架构耦合，可独立推进。

---

## 8. 需董事长签字的架构决策点（6 项）

| # | 决策点 | 选项 | 架构师建议 | 影响 |
|---|--------|------|-----------|------|
| **S1** | **实施顺序**：方案③全量内的推进节奏 | A) Phase1(LayerA+重启式)先行，代理层 Phase2 后置；B) 直接一次性全量上代理层 | **A** | 试点先验证「切换能力成立」，再上「无缝」；失败爆炸半径小一个量级 |
| **S2** | **代理层触发条件**：何时引入代理层 | A) 「对话不打断」成为硬验收标准时才上；B) 无条件尽快上 | **A** | 避免半单向门（§7.1）提前触发；试点场景（网关整断）重启式已 100% 解决 |
| **S3** | **HTTP 栈**（Phase 2 若上） | A) axum 0.7 + reqwest 0.12（rustls），不引入 hyper 裸路径/header case 保真；B) 全量照搬 cc-switch hyper 栈 | **A** | 引入 3 个新依赖 + 本机 HTTP server 安全面 + 编译时长上升；header case 保真只在严格指纹网关需要 |
| **S4** | **配置边界** | A) 新增 `~/.onecode/providers.json` 独立文件，ConfigManager/ProviderStore 双单一真相；B) 在 desktop.json 里长 providers 列表 | **A** | 决定用户数据位置 + 前端 schema 改动面 |
| **S5** | **API key 安全模型**（Phase 2 若上） | A) key 移至 ProviderStore 内存 + providers.json 持久化，CLI env 变占位符；B) 维持 key 走子进程 env | **A** | key 泄漏面变小（env 对同用户可见），但新增本机网络面；需绑 127.0.0.1 + 共享 bearer 校验 |
| **S6** | **持久化** | A) JSON 文件足够，不引入 SQLite；熔断健康 v1 内存态、v2 落盘；B) 直接上 SQLite | **A** | 技术债容忍度确认：SQLite 是 0→1 阶段过早抽象 |

### ✅ 董事长签字记录（2026-08-08）

> **S1-S6 全部按 @architect 建议签字（6/6）**。签字后生效：
> - **Phase 1 = Layer A 多供应商 + 重启式 failover** 先行 → 交 @vp-eng 出 PRD → @dev 实现。
> - **Phase 2 = 代理层** 后置，触发条件 = 「对话不打断」成为硬验收标准（S2），上代理层前需再签（半可逆，红线前 1 周内可拆）。

---

## 9. 一句话结论（转董事长）

> **方案③全量架构可行、可逆性有底线（代理层=半可逆，红线前 1 周内可拆），建议按「Phase 1 = Layer A 多供应商 + 重启式 failover 先行，Phase 2 = 代理层后置」推进；6 项签字点（S1-S6）批准后即闭环设计链。**

---

## 10. 后续动作

1. ~~S1-S6 待董事长签字~~ → **✅ 已签（2026-08-08）** → 交 @vp-eng 出 PRD → @dev 实现 Phase 1。
2. **合规**：移植文件头保留 MIT 版权声明 + 附 LICENSE 副本，@clo 复核 attribution 写法。
3. **文档落盘**：本 ADR 同步写入 `.claude/memory/core/architecture.md`（D033 行）+ `.claude/memory/archival/decisions/decisions.md` + `.claude/blackboard/decisions-log.md`。

---

*附：独立评审记录 = ① cc-switch proxy 深度评审（subagent a238bd96）；② HTTP 栈+接口边界评审（subagent aa41d4ce）。两者结论均基于源码逐文件阅读，未遇配额限制。*
