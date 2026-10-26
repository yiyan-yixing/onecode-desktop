# AI 员工调度台 — 产品定义（按董事长的目标拆）

> 日期：2026-10-14 ｜ 作者：@dev
> 目标原文（董事长）：
> **「整合各家终端的优点 → 并能自选模型 → 然后 管理员工 / 管理产出 / 管理工作」**
>
> 配套：架构与证据见 [2026-10-14-dsh-fusion-agent-os-design.md](./2026-10-14-dsh-fusion-agent-os-design.md)（DSH 288 包源码审计 + OneCode 资产/安全审计）
> 证据等级：**[实测]** 直读源码/实跑 ｜ **[报道]** 外部公开信息 ｜ **[推断]** 我的判断

---

## 0. 先读懂目标里的一个顺序

> 整合各家终端的优点 → **并能**自选模型 → **然后** 管理员工 / 管理产出 / 管理工作

这个顺序不是罗列，是**架构判断**：

**前两件是"地基"，后三件是"楼"。** 如果引擎抽象和模型抽象没做好，三个管理面一定长成假的：

- 引擎抽象没做好 → "管理员工"退化成**管一堆终端窗口**（产品自己的审视文档已经判定过：那不是购买理由）。
- 模型抽象没做好 → "自选模型"退化成**一个下拉框**：换了模型，上下文窗口算错，autoCompact 阈值错位 → 要么 400、要么白白压缩。**换模型会变危险。**

所以本文的组织顺序 = 目标原文的顺序，且**地基两节先写"判据"再写"做法"**。

### 0.1 一句判据：什么算"整合成功"

> **同一个员工，换引擎不换账本；同一个任务，换模型不换验收；同一个产出，换人接手不换证据。**

三条都能做到，才叫整合。做不到，就只是"装了几个 CLI"。

---

## 1. 整合各家终端的优点

### 1.1 一条纪律，防止"整合"变成"堆功能"

**只抄机制，不抄功能表。** 每抄一件，必须能回答两个问题：
1. 它落到五个单元（Playbook / 员工-Slot / Team / ReviewItem / 账本）里的哪一个？
2. 没有它，上面那条判据的哪一句会破？

答不上来的，就是"看起来很酷但会变成维护债"的东西 —— 比如 vendor 进仓库的 14MB Monaco 就属于这类（无已交付表面）。

### 1.2 优点清单 → 归谁实现 → 落到哪层

| 来源 | 最值得抄的**一件** | 落到哪 | 我们已有的条件 |
|---|---|---|---|
| **DSH** | **事件溯源 Session**：一切模型可见的东西必须能从日志重建（可 fork、可审计、可导出、可计量） | L1 账本 | 引擎免费，直接用 **[实测]** |
| **DSH** | **上下文经济学的成本顺序**：先免费（按字节保留 → spill 给真实路径 → 无模型裁剪），再付费（LLM 摘要） | L2 策略 | 包直接用，调默认值 **[实测]** |
| **DSH** | **能力组合式 preset**：角色 = 装了哪些工具/技能/权限，不是一段提示词 | L2 Playbook | 直接用 **[实测]** |
| **DSH** | **窄≠窄的教训**：它的审批只有 `allowed-once`、plan mode 无牙 —— **反面教材，正好用来设计我们的门** | L4 反例 | 见 §6 铁律 1/2 **[实测]** |
| **Claude Code** | **`SKILL.md` + `references/` + `scripts/` 生态**（三家已收敛，Apple 也导出了 10 个） | L2 可移植扩展面 | DSH 的 `customSkillDirs` 直接吃 **[实测]** |
| **Claude Code** | **NDJSON 双工会话**：`claude -p --input-format stream-json --output-format stream-json` + `--resume/--fork-session/--session-id` → **可按轮寻址的 Slot，不用刮 PTY** | L0 驱动协议 | 比 PTY 高一级（轮次可寻址、可续、可 fork）**[研究]** |
| **Claude Code** | **持久化"总是允许"**：把作用域规则（如 `Bash(npm test *)`）写进**仓库根** `.claude/settings.local.json` | L4 审批记忆 | 正好补 DSH"只有 `allowed-once`"的短板 **[研究]** |
| **Claude Code** | 转录是可解析的 JSONL 会话树：`~/.claude/projects/<项目>/<sessionId>.jsonl`（`parentUuid` / `isSidechain`） | L1 账本数据源 | 本机实测 52 个项目目录，`cc_sessions.rs` 已在读。**⚠️ 但官方明确说行格式是内部实现、随时可能变 → 见 §9.1** |
| **Claude Code / Codex** | **hooks 兼容层**：让用户已有的 guardrails 继续生效 | L4 迁移楔子 | **计数修正：Claude Code 是 33 个事件（不是 30），DSH 只桥 7 个；Codex 是 12 个且已用 camelCase 镜像 Claude 的词汇 → 以 Codex 的 12 个为规范集最省事** **[研究]** |
| **Codex** | **`app-server` 全套 JSON Schema 在仓库里**（~100 方法 / ~70 通知 / 10 个服务端→客户端请求）+ 4 种传输（`stdio://`/`unix://`/`ws://`/`off`）；`thread/start\|resume\|fork\|read\|list`、`turn/start\|steer\|interrupt`、`model/list` | **L0 最强引擎协议** | 有 schema 就能生成客户端，不用猜 **[研究]** |
| **Codex** | **`item/tool/call` 可倒转**：服务端请**宿主**执行"宿主自己的工具" | **L2 把我们的能力注入任意线程**（调度/成本计量/跨 Slot 消息），不需要 MCP | 这是"控制台把能力借给引擎"的机制 **[研究]** |
| **Codex** | `ApprovalsReviewer: user\|auto_review\|guardian_subagent`（审批交给 LLM 子代理）+ `AskForApproval` 的 `granular` 分类 | L4 自动初审 | 与 DSH auto-review 互相印证 → 方向确认 **[研究]** |
| **Gemini CLI** | **TOML 策略引擎**：按 `toolName`/`commandPrefix`/参数匹配，决策 `allow\|deny\|ask_user`，带优先级与 Default/User/Admin 三层 | **L4 唯一"策略即语言"的权限模型** —— 最接近 Xcode 27 的窄门 | 抄它的规则文件形态（比 UI 开关可审计）**[研究]** |
| **OpenCode** | **"总是允许"是协议字段**（`remember?`）而非 UI 装饰；MIT；`opencode serve` 暴露 OpenAPI 3.1 + SSE `/event` | L4 审批记忆 + L0 可选协议 | 抄它的**回复形状** **[研究]** |
| **models.dev** | **228 个 provider 的模型目录**（`limit.context` / `limit.output` / `cost` / `reasoning` / `tool_call` / `modalities` / `open_weights`），OpenCode 原生消费 | **L4 窗口表 + 账本价目表的现成数据源** | **把 §6.5 的"窗口表"从手工维护变成可生成** **[研究]** |
| **Aider** | 编辑前先提交、每次改动一次提交（LLM 写 Conventional Commits）→ **git 就是会话存储与撤销栈** | L1 撤销/审计 | ⚠️ **最后推送 2026-05-22，疑似停更**，且**没有任何流式协议**（只能 PTY） **[研究]** |
| **Xcode 27** | **窄而实在的审批**：build/test/预览**不问**，只有 shell/CLI 逐条问 | **L4 审查队列的核心规则** | 直接抄（这是全案最该抄的一条） |
| **Xcode 27** | **计划是可编辑的 Markdown 工件**，在对话旁审阅/批注/批准；跑动中可排队发消息 | L3 交互 | 抄交互，但**不把计划当安全边界** |
| **Xcode 27** | **自验证闭环**：预览/模拟器/截图/Instruments 让 agent 真的用上 IDE，且这闭环能**通过 MCP 外借** | L2 工具包 | 借，不自建（`xcrun mcpbridge`） |
| **Xcode 27** | **对话即编辑器**（tab / 分屏 / 停靠） | L3 Studio | desktop 已有槽模型，扩展成视图 |
| **Claude Cowork** | **删除永远问**（任何模式下都确认） | **L4 铁律** | 直接抄 |
| **Claude Cowork** | **产物是真文件**（带公式的 xlsx / pptx / docx），可边看边改 | L2→L4 产出 | `dsh-skill-office` + `dsh-office-to-pdf` 已有 |
| **Claude Cowork** | **真隔离 + 云端无人跑**（设备关机也行） | L1 隔离 + L2 触发器 | 容器可替代 VM；云端要自建 |
| **经典终端**（Ghostty / WezTerm / iTerm2） | 多 tab/分屏、滚动搜索、键位可重绑、真色彩字体、低延迟 | L3 Studio 工艺 | 移动端 IME/惯性滚动经验可复用 |
| **OneCode 自己** | **多槽 PTY + generation counter + 崩溃存活**；多 provider 切换；**上下文窗口推导** | L1 核心 | 已有（desktop `pty/{mod,slot}.rs`、`providers.rs`、`model_context.rs`）**[实测]** |
| **OneCode 自己** | 安装器 UX、容器生命周期 CLI、code-server 补丁台账、移动端 IME | L1 分发 / L3 | 已有 **[实测]** |

> ⚠️ 注意表里 DSH 出现了 4 次，其中一次是**反面教材**。整合不是"谁最好就用谁" —— 是**把每家最锋利的那一件拿走，同时不做它最钝的那一件**。

> **技能目录的坑**：`<root>/<name>/SKILL.md` 的最小公分母根是 **`.agents/skills/`** —— Codex / Gemini CLI / OpenCode 都读它，**但 Claude Code 不读**（它只认 `~/.claude/skills` 与 `.claude/skills`）。→ **Playbook 的技能源放 `.agents/skills/`，安装时镜像/软链到 `.claude/skills/`**，否则 Claude Code 这条腿看不见技能。**[研究]**

### 1.3 接入门槛（防止"什么都能接，什么都不好用"）

一个引擎只有**同时**满足三条，才允许作为**一等引擎**接入（进花名册、进账本、进审查）：

| # | 条件 | 为什么 |
|---|---|---|
| a | **可程序化驱动**（有协议，或至少可被 PTY 稳定托管） | 否则"派活"只能靠打字 |
| b | **会话可落盘、可恢复**（有可解析的 transcript） | 否则账本断了，"换引擎不换账本"破 |
| c | **有审批接缝**（能拦住不可逆动作，或能被我们拦） | 否则它成为整条治理链的漏洞 |

不满足的，**只作为"外部终端"**（在 Human Terminal 里手工跑），不进花名册、不进账本。
**这条门槛是"整合"和"堆功能"的分界线。**

**协议优先级**（依据见 §2.4）：**ACP 优先**（一个连接多会话 + 会话级模型选择 + 权限请求可自动应答）→ 各家私有协议（如 Codex `app-server`、Claude Code stream-json）→ **PTY 兜底**。PTY 兜底的代价要说清楚：**派活只能靠打字、审批拦不住、转录不可解析** —— 也就是"能看见但管不住"。

---

## 2. 自选模型

### 2.1 先说清楚：自选模型**不是为了省钱**

ACE 用本机 50,564 次真实调用已经测出：**模型分层路由的省钱天花板 < 2%** **[实测 ace/docs/03]**。所以如果"自选模型"的卖点是省钱，这个功能一开始就失败了。

**真正的五个理由（按重要性）**：

| # | 理由 | 例子 |
|---|---|---|
| 1 | **能力匹配** | 长上下文任务 / 视觉任务 / 需要推理强度 / 需要工具调用可靠性 |
| 2 | **隐私与合规** | 敏感项目只在本地模型上跑；不出境 |
| 3 | **可用性** | 限流、故障切换 —— 一家挂了不能停摆 |
| 4 | **速度** | 交互式要快，批处理可以慢而便宜 |
| 5 | 成本 | 排最后，而且省在**上下文瘦身与调用次数**，不在路由 |

### 2.2 三层，不是一个下拉框

| 层 | 谁定 | 存在哪 | 例子 |
|---|---|---|---|
| **员工级** | 招聘时定 = "它是谁" | Slot 的 schema | `@reviewer` 固定用便宜快模型；`@architect` 固定用最强推理 |
| **任务级** | Playbook 声明 = "这类活用什么档" | `playbook.yml` | 写公众号用 A，跑回测用 B |
| **步骤级** | 预算与计量驱动 = "这一步要不要升降" | 运行时决策 | 剩余预算少 → 降档；连续失败 → 升档 |

### 2.3 前置条件与开工顺序（不做这个，自选模型会变危险）

> **换模型的正确性，依赖"上下文窗口"的正确性。**

上下文窗口算错的代价：窗口填小→过度压缩（浪费钱、丢信息）；填大→400（浪费一轮、可能丢一次工具结果）。OneCode 已经踩过两次生产 400 并从中学出 `model_context.rs`（阈值 = 窗口 − 32k 补全 − 25% 突发 − 5% 余量）**[实测，desktop 最后一条 commit 就是修这个]**。

所以第二节的开工顺序必须是：
1. **修 `_switching` 无 watchdog**（否则切换可永久锁死，要重启 app）**[实测，M1 遗留 P1]**
2. **修 `perform_switch` 双写非原子**（否则 split-brain，静默回滚）**[实测，M1 遗留 P1]**
3. **把 provider/model 从"全局单例"改成"每槽"** —— 见下面的架构缺口
4. **窗口表一般化 + 社区可提交 + "未识别模型（保守估计）"标记** —— **不必从零手搓**：**models.dev/api.json 已给出 228 个 provider 的 `limit.context`/`limit.output`**，Codex 的 `model/list` 也给结构化目录。→ 窗口表改为**从这两个源生成 + 本地覆盖 + 未识别时标"保守估计"**。**[研究]**
5. **模型目录机器可读**：`{contextWindow, vision, toolCalling, price, locality, rateLimits}` —— 由 models.dev 供大部分字段，`rateLimits`/`locality` 本地补。
6. **故障切换**：**只有 Claude Code 有真正的 fallback**（`--fallback-model a,b`，仅在 overloaded/unavailable/非可重试时触发，**从不在 auth/billing/限流/传输错误上触发**，去重后上限 3 个，只作用于一轮，也覆盖子代理）；Codex 有 `model/rerouted` + 可配 `model_catalog_url`；OpenCode / Gemini / Aider 无文档化链路。
7. **本地模型**：**只有 Codex 是一等公民**（`oss_provider = "lmstudio" | "ollama"`）；OpenCode/Aider 走 OpenAI 兼容层；**Claude Code 原生不支持**。→ 本地档要么挂 Codex，要么挂 OpenCode，别指望 Claude Code。
8. 然后才做三层选择 UI

**⚠️ 架构缺口（本机实测）：现在的"自选模型"是全局的，不是每员工的。**

| 事实 | 证据 |
|---|---|
| `PersistentSlot` **没有** provider/model 字段，只有 `cmd / args / env / backend` | `session/mod.rs:19-30` **[实测]** |
| `terminals` 表也**没有** provider/model 列 | `session/schema.rs` **[实测]** |
| 切换 provider 时，把激活供应商的 creds 写进**同一个**隔离配置目录 `~/.onecode/cc-config/settings.json` | `providers.rs:458,478-485`（注释："全局隔离：切档后同步到 onecode 专属的隔离 CC 配置"）**[实测]** |

**这意味着**：现在全机同时只有**一个**激活模型；要让 `@reviewer` 用便宜快模型、`@architect` 用最强推理**并行跑**，做不到。

**两条实现路径**：

| 路径 | 做法 | 成本 |
|---|---|---|
| **A. 扩展现有隔离模式（渐进）** | 把 `cc-config/settings.json` 从"一个全局目录"扩成 `cc-config/<slotId>/settings.json`，每槽独立 creds/env；`PersistentSlot` 加 `provider_id` + `model` | 改动集中在 `providers.rs` + `session/`，**复用已有的隔离机制**（它本来就是为了不污染用户全局 `~/.claude/settings.json` 而写的） |
| **B. 走协议层（一步到位）** | 用 ACP（或各家私有协议）把模型作为**会话级 option** —— ACP 原生支持且**在一个 turn 内 pin 住**（§2.4） | 需要先有 ACP 客户端；但换来"换引擎不换账本" |

> **建议**：**A 先做**（能立刻把 `@name` 派发与员工级模型接上，且改动面小），**B 作为一等引擎的目标形态**。

### 2.4 驱动协议：ACP 已经把"自选模型 + 多员工"做成了协议原语 [实测]

这是本轮最有用的一条发现。`dsh-acp` 的定位**就是我们的场景**，原文：

> "Choose it for **out-of-process subagents**, test runners, and scripted controllers"
> "One connection can run **several sessions at once, each independent**"

| ACP 能力 | 对应用户目标 | 原文依据 |
|---|---|---|
| 一个连接跑多个独立会话 | **管理员工**（多 Slot ↔ 多 ACP session） | "One connection can run several sessions at once, each independent" |
| `session/new` / `list` / `resume` / `close` | 招聘 / 看病史 / 暂停 / 解雇 | 方法清单 |
| **会话级 opaque provider/model 选择 + `reasoning_effort`** | **自选模型** | "Session configuration offers opaque provider/model choices from the **live LLM service catalog**" |
| **一次 prompt 会 snapshot 该选择，并在该 turn 的每一步 pin 住**；并发改动只影响下一 turn | **步骤级/任务级模型隔离**（防止跑到一半换模型导致上下文错位） | "A prompt snapshots that selection … and **pins it across every model step in that turn**; a concurrent option change applies to the next turn" |
| `session/request_permission`，**一次性 allow/reject，客户端可自动应答** | **审查队列的插口** | "A permission prompt with one-shot allow/reject choices; **your client can answer automatically**" |
| `session/prompt` / `session/cancel` | 派活 / 打断 | 方法清单 |
| stdio MCP 条目授权其绝对命令与环境；HTTP 条目授权其绝对 URL 与头 | 权限模型的边界 | "ACP clients are trusted controllers: …" |

**它刻意不做的**（这决定了我们的账本必须是自己建的）：`session/load`（v1 有 `session/load` 之争，但**无文档化的 `session/fork`**）、删除、**fork**、额外目录、modes、commands、**plans**、**terminals**、客户端 fs 操作、elicitations **[实测]**。

**★ 它已经有官方 agent 注册表（这是"整合"最省力的地方）**：ACP 官方生态里已有 **Claude Agent（Zed 的 `claude-agent-acp` 适配器）、Codex CLI（`codex-acp`）、Cursor、Gemini CLI、OpenCode、Goose、GitHub Copilot（公开预览）、Cline、Qwen Code、Kimi CLI、Kiro、Junie/JetBrains、OpenHands、Factory Droid、Mistral Vibe、Augment** 等约 25+ 个。**写一个 Rust ACP 客户端，就把大部分生态变成同构 Slot** **[研究]**。
> 原生协议更丰富的地方（Codex `app-server`、OpenCode 的 OpenAPI 服务端）用原生，但**在 ACP 形状的门面后面**——这样引擎可换、账本不用换。

> **结论**：
> 1. **首选 ACP 作为一等引擎的驱动协议**，PTY 作为不满足门槛时的兜底；原生协议更强者走"原生 + ACP 门面"。
> 2. **模型目录不必自己发明** —— ACP 从**活的 LLM service catalog** 给出 provider/model 选项；外部再叠 models.dev 补窗口与价格。
> 3. **因为 ACP 自己不给 fork / plans / terminals / 转录回放，所以"账本必须是我们自己的"** —— 这是"换引擎不换账本"的技术理由，不是口号。
>
> ⚠️ 一处产物缺陷（同 `.d.ts` 那类）：DSH 的 README 反复引用仓库客户端 **`dsh-subagent-acp`**，但**本机这 288 个包里没有它** **[实测]**。所以"用 ACP 做进程外委派"这条路，**服务端有，编排端要我们自己写** —— 而这正是 ACP 客户端该干的活。

### 2.5 验收判据

- 给同一个任务换 3 个不同窗口大小的模型，**autoCompact 阈值每次都正确**（不出现 400，也不出现过度压缩）。
- 一个不在窗口表里的模型，**显示"保守估计"而不是假装知道**。
- 一家 provider 挂掉（拔网线模拟），**在跑的员工自动切换且账本不中断**（`seq` 连续）。
- 切换过程中**杀掉 app 再拉起**，不出现 split-brain（配置与运行态一致）。
- ACP 引擎：一次 prompt 中途改模型，**本 turn 仍用旧模型跑到结束**（验证 turn 级 pin 生效）。

---

## 3. 管理员工（Roster）

### 3.1 一个"员工"是什么（字段即定义）

一个员工 = 一个 **Slot**：

| 字段 | 含义 | 现状 |
|---|---|---|
| `name` | 唯一名字，永不复用（DSH Teams 的做法：连创建失败的也占名） | DSH 有，可借 **[实测]** |
| `engine` | 它跑在哪个引擎上（DSH 原生 / Claude Code / Codex / ACP / 本地） | 要新建（§1.3 门槛） |
| `playbook` | **能力组合**（工具 + 技能 + 委派策略 + UI 槽），不是提示词 | DSH preset 直接吃 **[实测]** |
| `model` | 员工级默认模型 | desktop `providers.rs` 有 CRUD，**但模型是全局单例**（`PersistentSlot` 无此字段）→ 必须先按 §2.3 改成每槽 **[实测]** |
| `posture` | **权限姿态**：沙箱档 + 审批策略 + 出境策略 | DSH 三档 + fail-closed **[实测]**；desktop 现在**没有**（全程 `bypassPermissions`） |
| `cwd` | 工作目录（一个员工一个项目） | 有 |
| `budget` | 预算上限（token/钱/时长），**硬闸** | **全生态都缺**（DSH grep 预算 = 0）**[实测]** |
| `health` | 状态 + 上次动作 + 是否卡住 | desktop 有 health loop，**但现在只 warn** |
| `transcript` | 病史：完整事件日志 | desktop **只存配置不存内容** **[实测]** |

### 3.2 六个动作（这就是"管理"的全部）

| 动作 | 键位/入口 | 等价现实隐喻 |
|---|---|---|
| **招聘** | `@name` 新建 或 从 Playbook 装 | 招人 |
| **派活** | 把一条任务指给某员工 | 派工单 |
| **开会** | 多员工并行（同一个 Lead + 若干 teammate） | 项目组 |
| **暂停/打断** | interrupt（保留待办，不删消息） | 打断一下 |
| **解雇** | dispose（有界清理，超时可见失败） | 辞退交接 |
| **看病史** | 打开 transcript / trajectory | 查工作记录 |

> **最便宜、最高杠杆的第一步**：让 `@name` **真的派发**。现在 desktop 的侧栏 agent 列表**不派发**——它只是把 `@dev ` 打进同一个 PTY **[实测]**。改成"以该 agent 为身份开一个新 Slot（自己的槽 / system prompt / transcript / provider / 权限姿态）"，**是 `pty_spawn` 的参数变更，不是重写**。这一步把"终端"变成"调度台"。

### 3.3 验收判据

- `@architect` 与 `@dev` 两个员工：**transcript 互不可见、system prompt 不同、权限档不同、模型可不同**。
- 关掉 app 再开，**员工的 transcript 还在**（现在会丢 —— SQLite 只存配置）。
- 一个员工卡住（模拟死循环），**健康检查能自动处置**（不只是 warn 日志）。
- 员工超预算 → **被中止**，账本记录原因（不是只报警）。

---

## 4. 管理产出（Deliverables）

### 4.1 产出的定义（这一节是三条铁律里最容易做错的）

> **产出 = 真文件 + 证据 + 验收状态。**

| 要素 | 反例（不算产出） | 正例 |
|---|---|---|
| 真文件 | 聊天里的代码块 | 工作区里带公式的 `.xlsx`、能打开的 `.pptx`、可跑的 `.py` |
| 证据 | "我已经完成了" | `node tools/check-layout.js out.html` → exit 0；diff；校验和；成本 |
| 验收状态 | 没人签字 | 有状态机：`草稿 → 待审 → 已验收 → 已归档` |

### 4.2 状态机与证据类型

```
草稿 ──提交──> 待审 ──人批准──> 已验收 ──> 已归档（进知识库）
                 │
                 └──驳回（结构化理由回灌给员工）
```

**证据类型（按可信度排序）**：

| 证据 | 例子 | 可信度 |
|---|---|---|
| 可执行检查通过 | 测试、lint、eval 脚本、schema 校验 | ⭐⭐⭐ |
| 可测量的差异 | diff、文件校验和、字节数、帧数 | ⭐⭐⭐ |
| 外部事实 | 抓回来的 HTTP 200、git commit、发布成功 | ⭐⭐ |
| 截图/渲染 | 页面截图、PDF 首页 | ⭐⭐ |
| 成本/用量 | token 分项、¥ | ⭐（不是质量证据） |
| **模型自述** | "我认为可以了" | ❌ **不作为证据** |

> **最后一行是硬规矩，不是偏好。** ACE 已实测：本地判官**假通过率 100%**；董事长铁律 7 也是同一句话（"拿一个可测量的差异证明它真的生效了"）。

### 4.3 可直接复用的现成件 **[实测]**

- **DSH**：`present` 工具（声明交付物，≤4 文件/次）、`workspace-changes`（git 快照做 turn 级改动对比）、`ui-deliverables`（改动卡 + 右栏 diff 审查 tab + 悬停 500ms 浮层 + 内联路径可点）。
- **Cowork 的启发**：产物是**真 Office 文件**。DSH 有 `dsh-skill-office` + `dsh-office-to-pdf`，可直接支撑 docx/xlsx/pptx 的产出与预览。

### 4.4 验收判据

- 每个"已验收"的产出，**都能回答"依据什么证据"**，且证据是可重跑的。
- 把产出目录拷到另一台机器，**文件能打开、脚本能跑**（不依赖聊天记录）。
- **没有任何产出仅凭模型自述进入"已验收"。**

---

## 5. 管理工作（Board）

### 5.1 任务的定义

一条任务 = `{标题, 详情, 归属员工, 依赖, 触碰范围, 预算, 验收标准, 状态}`。

DSH 的 Teams 已经把大部分形状给对了 **[实测]**：CAS 版本号（`expectedRevision`，过期即拒）、依赖 DAG + 环检测、`writeScopes` 重叠**告警**、状态 `pending/in_progress/completed/deleted`。

**但它的四个短板必须补**（否则"管理工作"会半途塌）：

| 短板 | 后果 | 我们的处置 |
|---|---|---|
| `writeScopes` **只是告警**，不阻塞 | 两个员工同时改一个文件 → 互相覆盖 | 上加**真锁**或 worktree 隔离（DSH 自认无 worktree/merge/lock） |
| **无 ownership 自动释放** | 员工挂了，任务永远锁在他名下 | 超时/进程退出/失败 → 自动回收 |
| **单进程、非跨进程 exactly-once** | 多机/多进程协作不支持 | 明确不做跨进程，或自建 |
| 上限 16 成员 / 256 任务 | 规模天花板低 | 按需调；Fleet 阶段要重做 |

### 5.2 三种触发器（"无人值守"能不能成立取决于这里）

| 触发器 | DSH 现状 **[实测]** | 我们必须做的 |
|---|---|---|
| **人** | 有 | 直接可用 |
| **定时** | 一条 Host 定时器，**关掉 app 就不触发**；错过只补最近一次 | **自建持久调度**（独立 daemon 或系统级调度） |
| **事件** | 签名 webhook，但**无队列/无重放/无去重**；崩溃丢任务 | **自建队列 + 幂等键** |

> **重要**：董事长自己的运作方式是"**夜间 = AI 自治（人类睡觉，AI 干活）**"（`CLAUDE.md`）。而 DSH 的自治底座**不耐久**（`jobs` 纯内存、`schedule` 关 app 就停、workflow 无 journaling、webhook 无队列）**[实测]**。
> → **所以"管理工作"里最硬的一块，是让调度与作业真正持久化。** 在这件事做完之前，**不要把"夜间自治"当已交付能力**。

### 5.3 任务与审查队列的接线

```
任务板 ──派活──> 员工(Slot) ──产出──> 产出物 + 证据
   ↑                                        │
   │                                        ▼
   └──关闭任务 <──已验收<──人批准<──审查队列（不可逆动作 + 待验收产出）
```

**一张板，一条队列，一个账本。** 人只需要看两样东西：**队列里等我拍板的**、**板子上卡住的**。

### 5.4 验收判据

- 一条任务从创建到关闭，**全程可在时间线上回放**（谁派的、谁做的、花了多少、谁批的）。
- 两个员工抢同一个文件 → **真的被拦住**（不是只打一条 warning）。
- 一个员工进程被杀掉 → 它名下的任务**自动回到待领**。
- **关掉 app 一晚上**：调度与作业记录**不丢**（这是与 DSH 现有能力的硬分界）。
- 早上打开，屏幕上只有两个列表：**等我批的**（几条）+ **卡住的**（几条）。超过 5+5 条就是设计失败（说明门槛没设对）。

---

## 6. 三条铁律（从三家抄来的，不能妥协）

| # | 铁律 | 来源 | 反例（谁做错了） |
|---|---|---|---|
| 1 | **窄而实在的审批**：build / test / 预览**不问**；shell 与不可逆动作**逐条问**；**删除永远问** | Xcode 27（窄）+ Cowork（删除永远问） | DSH **全都要问** → 批准疲劳；DSH 的 seam 里甚至**没有"总是允许"** **[实测]**；Xcode 的"build/test 不问但 shell 逐条问"被董事长自己批评为"节奏割裂" —— 我们要的是**按可逆性分**，不是按工具类型分 |
| 2 | **计划是可编辑的工件**，在对话旁审阅/批注/批准；但**计划不构成安全边界** | Xcode 27（工件化） | DSH 的 plan mode **"every tool stays callable"**，纯文字约束 **[实测]** —— 所以强制必须落在沙箱 + 审批 |
| 3 | **产物是真文件 + 真证据** | Cowork（真文件）+ ACE（证据） | 通用聊天框里的代码块；以及"模型说完成了" —— ACE 实测本地判官**假通过率 100%** |

**把第 1 条说细，因为它决定日常手感**：

| 动作类型 | 问不问 | 理由 |
|---|---|---|
| 读文件、搜索、看 diff、跑测试、构建、预览 | **不问** | 可逆 / 只读 / 不产生外部影响 |
| 改工作区内的文件 | **不问**（但进产出与 diff） | 可逆（有 git） |
| 写工作区外、发外部请求、付钱、发布 | **问** | 不可逆 / 越界 |
| **删除任何东西** | **永远问** | 不可逆，且是 Cowork 唯一无例外的规则 |
| 装 Playbook / 装插件 | **问** | **Host 代码跑在沙箱外 + 会执行第三方构建脚本** **[实测]** |

---

## 7. 落地顺序（把地基与楼分开）

| 步 | 做什么 | 判据 | 为什么在这个位置 |
|---|---|---|---|
| **0** | **断安全链**：token 强制 + `docker.sock` 改 opt-in + `Origin`/CSRF + 去 `bypassPermissions`；关掉 DSH 的默认 transcript 出境 | 无 token 访问返回 401；日志证明无 transcript 出境 | 不修这个，"多员工"= 多个提权通道 |
| **1** | **`@name` 真派发** + 终端拆两种（Human / Agent） | 两个员工 transcript 互不可见、权限档不同 | **地基第一块**：员工有了物理形态 |
| **2** | **模型从全局改每槽**（`cc-config/<slotId>/` + `PersistentSlot.provider_id/model`）+ 窗口表 + 修两个切换债 | 两个员工**同时**跑不同模型；换 3 个窗口不同的模型 autoCompact 都正确；断网自动切换且账本连续 | **地基第二块**：自选模型能用且不危险 |
| **3** | **产出与审查队列**（真文件 + 可执行证据 + 状态机 + 窄门审批） | 无 eval 的 Playbook 进不了注册表；无产出仅凭自述进"已验收" | 楼的第一层：**产出可验收** |
| **4** | **任务板 + 持久调度/作业** | 杀掉进程不丢任务；早上只有两列表；抢文件被真拦住 | 楼的第二层：**工作可编排** |
| **5** | 多租户 / Fleet / 市场 | 越权测试；第三方 Playbook 收入 > 自研 | 楼顶 |

> **注意第 1 步和第 2 步都是"地基"，但可以并行**：一个改 Rust 的 `pty_spawn` + Slot schema，一个改 `model_context` + `providers` 的切换可靠性。**它们不重叠。**

---

## 8. 明确不做（防走偏）

1. 不做第 X 个 harness / 引擎（引擎已免费）。
2. 不做"更好的终端"、不与 iTerm2 在"多 tab"上竞争（产品自己的审视文档已判定那不是购买理由）。
3. 不与 Cursor / Claude Code 在"补全与编辑体验"上竞争。
4. 不做模型训练 / 微调。
5. **不用 LLM 当验收裁判**（ACE 实测假通过率 100%）。
6. **不把"计划已批准"当安全边界**（DSH plan mode 无牙的教训）。
7. **不在调度持久化做完之前宣称"夜间无人自治"**。
8. **不再同时维护 web 端与 desktop 端两套并行实现**（web 端已停更，是零件库不是平台）。

---

## 9. 一页速查

**目标 → 五件事 → 一句话做法**

| 目标 | 一句话做法 | 第一步 |
|---|---|---|
| 整合各家终端优点 | **只抄机制，每件必须落到五个单元之一**；接入门槛三条件（可驱动/可落盘/有审批） | 用 DSH 做原生引擎，PTY 托管异种 CLI |
| 自选模型 | **不是为了省钱**（路由省 <2%）；三层（员工/任务/步骤）+ **窗口表是前置条件** | 修两个切换债 + 窗口表 |
| 管理员工 | 员工 = Slot = {能力组合, 模型, 权限, 目录, 预算, 病史} | **让 `@name` 真派发**（一个 `pty_spawn` 参数） |
| 管理产出 | 产出 = **真文件 + 证据 + 验收状态** | 接 DSH 的 `present` + `workspace-changes` |
| 管理工作 | 一张板 + 一条队列 + 一个账本；**触发要持久** | 自建持久调度与作业 |

**三句判据**：
> 换引擎不换账本。换模型不换验收。换人不换证据。

**三句底线**：
> 证据要可执行，不要模型裁判。
> 删除永远问；build/test 不问。
> 没有预算的自治不是能力，是账单。

---

## 10. 实现状态

### 10.1 第一刀：`@name` 真派发 v0（2026-10-14 已落地）

对应 §3.2 的"最便宜、最高杠杆的第一步"。**已实现并有两套测试保护。**

### 做了什么

| 层 | 改动 | 文件 |
|---|---|---|
| Schema | `slot.agent`（内存 / SQLite / 持久化）贯通，含旧库 `ALTER TABLE` 迁移 | `pty/slot.rs` `pty/mod.rs` `session/{mod,schema}.rs` |
| 解析 | 新增 `agents` 模块：按 **项目 → onecode 隔离全局 → 用户全局** 解析 `.claude/agents/<id>.md`；id 白名单防路径穿越 | `agents.rs`（新） |
| 注入 | 身份写成 system prompt **文件**（`~/.onecode/cc-config/agent-prompts/<id>.md`），用后端声明的 `--append-system-prompt-file` 传入 —— **不进 argv**（长文本不被截断，内容不出现在 `ps`） | `commands.rs` `backend.rs` |
| 诚实回报 | `pty_spawn` 返回 `agent_injected`；**后端不支持 / 定义没找到 / 写文件失败 → false**，前端在终端里明确打印"身份未注入"，不假装 | `commands.rs` `tab-manager.js` |
| 交互 | **点击 = 派发新员工 Slot**；**⌥/Ctrl/Cmd + 点击 = 同会话点名**（保留 `@id` 插入） | `agents-list.js` |

### 顺手修掉的一个真 bug

`~/.claude/agents/dev.md` 开头多了一个空的 `---/---` frontmatter 块，旧解析器据此得到空 frontmatter → **该 agent 在应用里一直是隐形的**。现在 `frontmatter_split` 会跳过空块并补回分隔符重解析（有单测 + 一个针对真实文件的门控测试，实测解析到 1 个文件）。

### 验证状况

| 检查 | 结果 |
|---|---|
| `cargo test`（含新增 11 个：解析/注入/守卫/持久化往返/旧库迁移/真实文件） | **56 passed / 0 failed** |
| `npm test`（agents-list 用例已改到新语义） | **268 passed / 0 failed** |
| `cargo check` | **无告警** |

### ⚠️ 唯一未验证的环节（必须说清楚）

**`--append-system-prompt-file` 对你这台机器的 `claude` 是否真的生效，我没能验证。**

- 依据：该 flag 出现在**本机随发布的官方 Claude Code bundle** 帮助文本里（`--append-system-prompt-file <file>`，与 `--append-system-prompt` 互斥："Please use only one."），覆盖 2.1.63.1 与更早版本。
- 但 `which claude` → `/Users/zhanglei/.baidu-cc/baidu-cc/bin/claude` 是一个 **Go 启动器**（baidu-cc），启动时会去 `baidu-cc-client.bj.bcebos.com` 取最新版本；**在本环境该请求超时，进程直接挂住**，所以我无法用运行期行为确认 flag 被转发（`strings` 也被剥了符号）。
- **请你手工确认一次**：在一个真实项目里点某个 agent → 看新 Slot 里终端是否报了"身份未注入"。
  - 没报 → 生效（`agent_injected=true`）。
  - 报了 → 说明该启动器不转发这个 flag；那就把 `backend.rs` 里 claude-code 的 `system_prompt_file_flag` 改成 `None`，我们改走"首条消息注入"的降级路径（届时身份是 user 消息而非 system prompt，必须如实标注）。

### 明确还没做（下一刀）

1. **模型仍是全局单例**（§2.3 的架构缺口）→ 两个员工**还不能同时跑不同模型**。
2. **`agent.tools`（frontmatter 的 `tools:`）没有落到权限** —— 目前只注入身份，不限制工具。
3. **agent 的 `model:` 字段没被使用** —— 员工级模型要等第 1 项。
4. 派发需要"当前项目"上下文：无活跃终端时只提示，不会自动建项目。

---

### 10.2 第二刀：每槽 provider/model（目标 ② 的地基，2026-10-14 已落地）

**关键发现：这条路已经被你自己打通了。** `pty/mod.rs` 声明了
`CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1` —— **让 Claude Code 忽略所有 settings 文件的
provider 变量、以进程 env 为准**。所以每槽路由**只需要 per-slot env**，
**不需要给每槽独立 `CLAUDE_CONFIG_DIR`**（那会把 `cc_status` / `cc_sessions` 两个读取器
全部打散，爆炸半径大得多）。这把本刀的改动面从"重构配置隔离"降到"一条 env 装配链"。

| 层 | 改动 |
|---|---|
| 纯函数 | `providers::resolve_route(catalog, global, provider_id, model)` —— 自选模型的**唯一判断实现**（命中供应商 / 未命中回退并标记 / 模型覆盖 / 空白视为未指定） |
| 纯函数 | `providers::apply_route_env(env_key_map, route, env)` —— **先清后写**：不先清掉调用方（含持久化下来的上一轮）env，就会出现"以为换了供应商其实没换"的静默失效 |
| Spawn | `pty_spawn(provider_id, model)` + `State<ProviderStore>`；返回 `effective_provider` / `effective_model` / `provider_fallback` |
| Schema | `slot.provider_id` / `slot.model` 贯通内存 / SQLite / 持久化（含旧库迁移）；重启后仍走同一供应商 |
| 前端 | `providerId` / `model` 透传；**`provider_fallback=true` 时在终端里明说"指定的供应商不存在，已回退到 X"** |
| 派发 | 新员工**继承当前项目的路由**（暂不启用 frontmatter 的 `model:` —— 直接用会把 Claude 型号名塞给第三方供应商，反而更危险） |

**验证**：Rust **65 passed / 0 failed**（新增 9 个：5 个路由解析 + 4 个 env 落地，含
"两个槽拿到不同凭据"「空 api_key 必须清掉旧值」「陈旧 env 必须被清」）；JS **269 passed / 0 failed**；
`cargo check` 无告警。

#### ⚠️ 顺手发现并修掉的一个**会破坏你真实配置**的测试 bug

`delete_non_current_cleans_refs` 会走 `delete_provider → save_to_file`，写到
**真实的 `~/.onecode/providers.json`** —— **在沙箱外跑一次 `cargo test` 就可能把你的
真实供应商列表覆写成测试夹具**。

它此前"绿"是**偶然**：同套件里另一个用例用 `std::env::set_var("HOME", tmp)` 泄漏了
进程级 HOME，替它挡了一下；并行时序一变就红（我加测试后就红了）。作者其实已在自己的
注释里记录过这个坑，但那个纪律没覆盖到这个用例。

修法（不是绕过去）：加一把进程级互斥 + RAII 恢复 HOME 的共享纪律
（`with_temp_home()`），并让两个改 HOME 的用例都用它。
过程中还踩到一个 **Rust 陷阱**：`let _ = with_temp_home();` 会**立即 drop** guard、
HOME 瞬间被恢复 —— 必须绑具名变量。

**证据**：并行 / 串行 / 单跑三种模式各跑 2 轮全部 65 绿；
真实 `~/.onecode/providers.json` 的 **sha256 与 mtime 在 4 次测试前后完全一致**（实测留证）。

#### 这一刀之后，目标 ② 到什么程度

| 能力 | 状态 |
|---|---|
| 数据层：槽携带 provider/model 并持久化 | ✅ |
| Spawn 层：按槽解析供应商凭据并注入 env | ✅ |
| 路由不可用时的诚实降级（回退并显示） | ✅ |
| **两个槽同时跑不同供应商/模型** | ✅ 机制已通（纯函数证明 + env 落地证明） |
| UI：设置某个员工的模型 | ❌ 还没有入口（只有继承项目路由） |
| 故障切换（provider 挂了自动切） | ❌ 未做（§2.3 第 6 项） |
| 窗口表 / 模型目录 | ❌ 未做（models.dev 可生成，§2.3 第 4 项） |
| 修 `_switching` watchdog / `perform_switch` 双写 | ❌ 未做（M1 遗留 P1） |

### 10.3 顺手堵住的规则 7 现场：三个界面各抄了一份"打 @name"

改「点击 = 派发」时按 CLAUDE.md 规则 7 grep 了一遍调用点，发现**除了侧栏，还有三处在走旧语义**：

| 位置 | 旧行为 |
|---|---|
| `orbital.js` 光球面板的 agent 列表 | 点击 → 往活跃 PTY 打 `@id ` |
| `palette.js` 命令面板（模糊搜索路径） | 选中 → 打 `@id ` |
| `palette.js` 命令面板（`@` 前缀路径） | 选中 → 打 `@id ` |

**这正是规则 7 描述的失效模式**：改了语义，只改了 1 处、漏了 3 处；不报错、不崩溃，只是"有些地方点下去变成打字"。

处置：把两种语义各抽成**一份**共享实现 `src/agent-dispatch.js`：

- `dispatchAgent({tabManager, agentId, agents, onNotice})` —— 点击 = 派发
- `insertAgentMention({tabManager, agentId, ptyWrite, onNotice})` —— ⌥/Ctrl/Cmd = 点名
- `isMentionModifier(event)` —— 两种语义的分界（三处共用同一判断，不再各写一遍 `altKey||ctrlKey||metaKey`）

三处界面统一改为调用共享实现；`terminal/mention.js` 的插入**保持不变** —— 那是"补全用户正在输入的 `@`"，机制与语义都不同，不该并入。

**结果**：全仓再搜"往 PTY 打 @name"只剩 2 处 —— 一份是有意的点名实现，一份是 `@` 补全。

**验证**：新增 `tests/agent-dispatch.test.mjs`（15 个用例，钉住语义边界与"失败必须如实提示"），
JS 套件从 269 → **284 passed / 0 failed**。

### 10.4 第三刀：员工级权限姿态（目标 ③ 的第三项，2026-10-14 已落地）

**先把 flag 词表验证清楚再写代码**（本机随发布的官方 Claude Code bundle 帮助原文）：

```
--allowedTools, --allowed-tools <tools...>   'Comma or space-separated list of tool names to allow (e.g. "Bash(git:*) Edit")'
--permission-mode <mode>                     取值：default | acceptEdits | planMode | bypassPermissions
```

也就是说 **「窄而实在的门」（§6 铁律 1）在 CLI 层面是可表达的**。于是本刀把它做成员工可声明的一项。

| 层 | 内容 |
|---|---|
| 声明 | agent frontmatter 新增 **`permission:`**（本产品扩展键，Claude Code 没有）：`bypassPermissions` / `acceptEdits` / `default` / `planMode` |
| 声明 | 复用已有的 **`tools:`** → `--allowedTools`（逗号或空格分隔；**括号内的 `Bash(git:*)` 不切**） |
| 纯函数 | `backend::apply_posture_args(profile, posture, tools, args)` —— **先清后写**，幂等 |
| Spawn | 从 agent 定义解析姿态 → 落到参数；回报 `posture` / `posture_inferred` / `allowed_tools` / `posture_note` |
| 前端 | 终端里显示 `· 权限档 acceptEdits（由 tools 声明推断）`、`· 工具白名单 Write, Bash, Read`；未生效时黄字告警 |

**三条设计决定值得记住**：

1. **`Inherit` = 一个参数都不动**。没声明就沿用后端默认（`bypassPermissions`）——
   **零回归**，不改变既有用户行为。
2. **只声明 `tools:`、没声明 `permission:` → 档位推断为 `acceptEdits`，并标记 `mode_inferred`**。
   理由：白名单在 `bypassPermissions` 下**形同虚设**，留着就是"门装了但没锁"。
   但界面必须标明"这是推断的"，不能让人以为是自己设的。
3. **未识别的 `permission:` 值 → 告警 + 不干预**，绝不静默当成 Inherit。

**过程中测试抓到一个真实的重复机制 bug**：我一开始用「逐 token 删除 `permission_args`」+「删 flag 及其取值」两套机制，结果前者把 `--permission-mode` 单独删掉、**留下孤立的取值** `bypassPermissions`，重复应用就累积垃圾。改成**按连续子序列整段移除**后收敛。幂等性测试（恢复会话会重复应用）是抓到它的原因。

#### 验证（含**真实数据**端到端）

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **80 passed / 0 failed**（本轮 +15：姿态解析/应用/幂等/回归/策略） |
| `npm test` | **284 passed / 0 failed** |
| `cargo check` | 无告警 |
| **真实 agent 端到端** | 用本机 `~/.claude/agents/dev.md`：`tools: Write, Bash, Read` + 无 `permission:` → **`acceptEdits`（推断）+ 白名单落地 + `bypassPermissions` 被整段替换** ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ③ 现在的完成度

| 员工字段 | 状态 |
|---|---|
| 身份（`@name`，自己的 system prompt） | ✅ |
| 模型（每槽 provider/model） | ✅ 机制通；❌ 无 UI 入口 |
| **权限姿态（权限档 + 工具白名单）** | ✅ 员工可声明并生效；❌ Review Queue 式的"哪些要问人"策略（属 ④/⑤） |
| **能力组合（Playbook：装哪些工具/技能）** | ❌ 未做 —— 员工目前只能用 `tools:` 做**收窄**，不能**扩展**（加技能/加工具） |
| 工作目录 / 预算 / 病史（持久 transcript） | 目录 ✅；预算 ❌；病史 ⚠️ 只存配置 |
| 招聘/派活/开会/暂停/解雇/看病史 | 招聘 ✅、派活 ✅、暂停（打断）⚠️、开会 ❌、解雇 ✅、看病史 ⚠️ |

### 10.5 第四刀：能力组合——技能（目标 ③ 的最后一块，2026-10-14 已落地）

员工原本只能**收窄**（`tools:` 白名单），这一刀让它能**扩展**：声明 `skills:`，技能正文进 system prompt。

**先确认引擎能力边界**：官方 bundle 里**没有任何"指定技能目录"的 flag**（只有 `--add-dir`），而 Claude Code 读技能的目录（`<project>/.claude/skills`、`~/.claude/skills`）都是**全局共享**、无法按槽区分。→ per-slot 技能组合只能靠**注入**。这是**如实的能力降级**，不是假装引擎原生支持。

**顺带修好一个真实的可达性问题**：Claude Code **不读 `.agents/skills`**（官方帮助串只列 `.claude/skills` 与 `~/.claude/skills`），而本机 `~/.agents/skills` 里有 **53 个技能**。我们的解析顺序把两套根都收进来，于是**那 53 个技能现在能被员工的技能组合用上**：

```
<cwd>/.claude/skills → <cwd>/.agents/skills → <onecode cc-config>/skills → ~/.claude/skills → ~/.agents/skills
（兼容扁平 <name>.md）
```

#### ★ 被真实数据推翻的一次设计（值得记住）

我最初的默认预算是 **32 KB，超了就不注入** —— 那个数是从 DSH"每个 SKILL.md 控制在 8192 字符"的约定推的。**实测把前提推翻了**：

| 根目录 | 技能数 | 最大单个 SKILL.md |
|---|---|---|
| `~/.agents/skills` | 53 | **127,524 B**（`gstack-ship`），另有多个 70–112 KB |
| 项目 `.claude/skills` | 116 | 33,168 B |

127 KB ≈ **32k token**。整体注入会吃掉一大块上下文；而**截断**会给出半截指令，比不加载更糟。

→ 改成引擎自己的**渐进披露**，三条路都必须能被如实区分：

| 情况 | 处理 |
|---|---|
| 找到且正文在预算内 | **内联**进 system prompt（省一次工具往返） |
| 找到但正文超预算 | 进 `deferred`：目录里列出，并**给出 SKILL.md 路径**，让模型需要时用自己的 Read 工具读 |
| 找不到 | 进 `missing`：界面黄字告警并给出查找顺序 |

**目录（名字+描述）永远注入** —— 哪怕一个都内联不进去。便宜，且让模型知道"有哪些能力可用"。

另外把注入文件名从 `agent_id` 改成 **`agent_id` + 正文 FNV-1a 摘要**：同一个 agent 在不同项目解析到不同技能，用 agent id 当文件名会让两个槽并发 spawn **互相覆盖**。

#### 验证（含真实数据）

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **95 passed / 0 failed**（本轮 +15） |
| `npm test` | **284 passed / 0 failed** |
| `cargo check` | 无告警 |
| 真实技能解析 | `~/.agents/skills` → `ai-papers-wechat-publisher`、`pptx` 均加载，missing 为空 ✅ |
| 真实大技能（生产预算） | `gstack-ship` = **127,524 B → deferred（给路径，不截断不丢弃）** ✅ |
| 真实 agent + 真实技能 → 注入文本 | `dev` + `gstack-browse` → 37,803 B，稳定键 `dev-e0ce6c6d889b1ef4` ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ③ 现在的完成度

| 员工字段 | 状态 |
|---|---|
| 身份（`@name` + 自己的 system prompt） | ✅ |
| 模型（每槽 provider/model） | ✅ 机制通，❌ 无 UI 入口 |
| 权限姿态（权限档 + 工具白名单） | ✅ |
| **能力组合（技能）** | ✅ 可声明、可加载、渐进披露；❌ 技能**市场/安装**（只有本地已有技能可用） |
| 目录 ✅ / 预算 ❌ / 病史 ⚠️ 只存配置 | 部分 |
| 招聘 ✅ 派活 ✅ 解雇 ✅ / 开会 ❌ 看病史 ⚠️ | 部分 |

### 10.6 第五刀：产出与证据化验收（目标 ④ 的地基，2026-10-14 已落地）

**原则一条不改**：**产出 = 真文件 + 证据 + 验收状态**，三者都不能由模型自述替代。

| 层 | 内容 |
|---|---|
| 度量 | `deliverables::git_snapshot` —— **我们自己**跑 git（argv 调用、不走 shell、限时 10s），拿 porcelain 状态 + numstat 增删行数。**"改了哪些文件"不是模型说的** |
| 协议 | 员工的工作台 system prompt 里注入**产出协议**：把交付写成 `<cwd>/.onecode/deliverables/<id>.json`（`files` / `evidence` / `summary`）。协议正文只有一份实现（`protocol_section()`），有测试钉住它真的进了注入文本 |
| 核对 | 声明的 `files` 由我们 **stat**：不存在就标 ❌ 并写进 `problem`。**坏 JSON / 缺字段的记录照样出现在列表里**（`unreadable` 单独计数），不静默丢 |
| 分级 | 证据按**可否机器验证**分四级：`exec`→🔧 可机器验证、`diff`/`artifact`→📐 可测量、`external`→❔ 核不了、**`self-report` 与未知→🚫 不是证据** |
| 验收 | 状态 `draft/pending/accepted/rejected` 落在 `<id>.status.json`；**三个命令都不暴露给模型**，所以 agent 无法把自己的产出标成"已验收" |
| 人机门 | `deliverable_review` 弹对话框摆开事实/证据/问题，人点「接受 / 驳回」。入口在命令面板（Cmd+K）→「产出审查」，**只在有项目上下文时出现** |

**审查文本长这样**（测试里真实打印的，不是设计稿）：

```
排版成品
记录 ID：rec
生成了 out.txt
验收状态：draft

⚠ 问题：声明的文件不存在：missing.txt

── 声明的文件（存在性由本机核对）──
✅ out.txt （1 字节）
❌ missing.txt （不存在）

── 证据（按可否机器验证分级）──
🔧 [exec] 可机器验证（本版不自动执行，需在审查时显式触发）   ← 第 4 轮当时的措辞；第 6 刀已改成「用「验证证据」跑一遍；不会自动执行」
      $ node check.js
❔ [external] 我们核不了（外部事实）：HTTP 200
🚫 [self-report] 不是证据（自述/未知类型）：我觉得好了

── 本机自己量到的工作区改动 ──
（不是 git 仓库：fatal: not a git repository ...）

接受这份产出吗？
```

#### 三条设计决定

1. **不自动执行模型写下的命令。** "自动跑一遍 eval 看绿不绿"听起来很美，但那是**让模型决定我们在本机执行什么** —— 一条提权路径。本版只把它标成"可机器验证"，执行留给**人在审查时显式触发**（下一刀）。
2. **未知证据类型一律判为"不是证据"**，宁可保守。反过来（把自述当证据）正是 ACE 实测过 100% 假通过的那类错误。
3. **问题排在事实之前。** "声明的文件不存在"这类事最容易被翻过去，所以放在最显眼处。

#### 过程中被"打印出来看一眼"抓到的一个真问题

测试全绿之后我把审查文本打出来看，发现是 `（不是 git 仓库：）` —— **冒号后面空的**。根因：`run_argv` 只捕获 stdout，而 git 的报错写在 **stderr**。这直接违背了我自己写的"非仓库必须有说明，不能静默"。

修法：失败时带上 stderr（空则回落到退出码），并把测试断言从 `error.is_some()` **加强为"说明不能是空串"** —— 弱断言正是它溜过去的原因。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **109 passed / 0 failed**（本轮 +14） |
| `npm test` | **284 passed / 0 failed** |
| `cargo check` | 无告警 |
| 真实 git 度量 | 临时真仓库里 `git init` + 改文件 → numstat 量出 `+1`、未跟踪文件也出现 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ④ 现在的完成度

| 要素 | 状态 |
|---|---|
| 真文件（由我们核对存在性/大小） | ✅ |
| 证据分级（自述被判为不是证据） | ✅ |
| 本机量到的改动（git） | ✅ |
| 验收状态机 + **人机门** | ✅ |
| **一键执行 exec 证据**（人触发，看真实退出码） | ❌ 下一刀 |
| 产出面板 UI（按项目列出、批量验收） | ❌ 现在只有对话框 + 命令面板入口 |
| 归档进知识库 / 成本并入产出台账 | ❌ |

### 10.7 第六刀：证据验证——声明变实测（④ 的最后一环，2026-10-14 已落地）

上一刀留下的话是"**声明了证据 ≠ 证据通过了**"。这一刀把后半句做出来：把 agent 声明的 `exec` 证据**真跑一遍**，记实测退出码、耗时、输出。

| 层 | 内容 |
|---|---|
| 执行 | `run_shell`（shell 表达 `a \| b`、`node x.js && …`）→ 逐条限时 120s，合并 stdout/stderr，输出带 2000 字节尾部上限与截断标记 |
| 超时 | 复用 **pty 的进程树清扫**（`snapshot_tree` + `sweep_tree`，SIGTERM→100ms→SIGKILL）—— 只杀直接子进程会留下被 launchd 收养的孙代 |
| 持久化 | 实测结果写回 sidecar 的 `verification` 键，与**声明**分开存放；审查文本里两者并列显示 |
| 一致性 | "给人看的命令清单"与"实际跑的命令清单"共用 `exec_commands`；"挑哪一份产出"共用 `pick_target` —— 不允许两处规则漂移（否则**人看的是 A、跑的是 B**） |
| 人机门 | `deliverable_verify` **先弹命令清单让人确认**，对话框里写明"以你的 OS 用户身份执行、**没有沙箱**、每条最多 120 秒"，确认后才跑；跑完把结果再弹给人看 |

**验过之后人看到的是**（测试真实打印）：

```
── 本机实测的验证结果 ──
1 条命令：1 通过 / 0 未通过（2026-10-14T01:55:09Z）
✅ 退出码 0 (24ms) $ echo ok
      | ok
```

#### ★ 测试抓到一个"假超时"（这一刀最值钱的发现）

我给超时写了断言 `timed_out == true`，它**通过了**——但那条测试**跑了 31.7 秒**。也就是说：**超时标记置对了，函数却根本没有按时返回。**

根因：我用了 `child.wait_with_output()`。它会一直读到**管道 EOF**，而超时杀掉直接子进程后，**孙进程仍握着同一个 stdout 管道写端** → 读到 EOF 要等孙代自己退出。

> 结论：**用 `wait_with_output` 的超时不是真的超时。** 它只是"标记为超时"。

修法：stdout/stderr 各起一个读取线程，超时后 kill + 树清扫，然后**有界排空**（最多 400ms），收多少算多少并标注"仍有子进程未退出"。测试也改成断言**硬不变量**：`600ms 的超时、总耗时必须 < 5s`。

**顺带发现**：本沙箱里 `/bin/ps` 被拒 → `snapshot_tree` 降级为只杀 root。真实机器上不会。但这件事说明**降级路径必须不能卡住**，上面的有界排空正是为此。`ps` 版本的孤儿检查测试做成**可用时才跑**（沙箱里跳过而不是假过），在你的机器上它会真去 `ps` 里查残留。

#### 另外修掉的一个会静默丢数据的隐患

验收状态与验证结果住在同一个 sidecar 里。如果两边各自"整体重写"，后写的会把先写的**悄悄抹掉**（不报错）。所以收敛成唯一写入口 `update_sidecar`（读-改-写，只动 patch 里的键），并加了**双向回归测试**：先验后接受 / 先接受后验，两个字段都必须还在。

顺手改掉一处**已经过时的自述**：`exec` 证据原先标注"本版不自动执行"，这一版已经能人触发了，措辞同步更正。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **118 passed / 0 failed**（本轮 +9） |
| `npm test` | **284 passed / 0 failed** |
| `cargo check` | 无告警 |
| 真跑证据 | `true`→退出码 0、`echo`→输出带回、`exit 3`→退出码 3 ✅ |
| 超时**真的**有界 | 600ms 超时 + 赖着不走的孙代 → 总耗时 **1.13s**（曾是 31.7s）✅ |
| 状态与验证互不覆盖 | 双向回归测试 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ④ 现在的完成度

| 要素 | 状态 |
|---|---|
| 真文件（我们核对）/ 证据分级 / 本机量到的 git 改动 | ✅ |
| 验收状态机 + 人机门 | ✅ |
| **一键验证 exec 证据（实测退出码/耗时/输出）** | ✅ 人触发、先看命令、树清扫 |
| 产出面板 UI（按项目列出、批量验收、批量验证） | ❌ 现在只有对话框 + 命令面板两个入口 |
| 归档进知识库 / 成本并入产出台账 / 产出→任务关联 | ❌ 属 ⑤ |

### 10.8 第七刀：任务板——工作第一次有了"板上"（⑤ 的第一块，2026-10-14 已落地）

⑤ 在目标里原本**一行代码都没有**。这一刀把它开出来，核心是**任务的"定义"与"裁决"分开存**：

| 存哪 | 是什么 | 谁能写 |
|---|---|---|
| `.onecode/tasks/<id>.json` | 任务的**定义**（标题/负责人/备注/阻塞/关联产出/声明状态） | 谁都能写（董事长、@ceo、员工上报进度） |
| `.onecode/tasks/<id>.human.json` | 人对状态的**裁决** | **只有人触发的命令** |

**为什么要拆成两份**：agent 有文件写权限，它完全可以自己往任务文件里写 `"status": "done"`。
如果状态只有一份，**agent 就能自己宣布任务完成** —— 这与 ④ 里"模型自述不算证据"是同一条规矩。
拆开之后：agent 写的 `done` 只是**自报**，生效状态以人的裁决为准，界面上两者都显示。

任务与产出的关联是**现场派生**的（每次扫描按 id 现查），不落盘复制 —— 复制一份状态就会分叉。

#### 任务板长这样（测试真实打印）

```
共 2 条任务

[👀 review（待验收）] b  人裁决的  @dev
      产出：d1 [accepted，证据未验证]
      → 关联产出全部**已被人验收**；但其中 1 个的**证据还没验证过**，可以考虑关闭

[⚠ done（自报，未经人裁决）] a  自报的  (未指派)
      ⚠ 状态 done 来自任务文件本身（未经人裁决）
```

#### 一并做出来的东西

| 件 | 说明 |
|---|---|
| 任务协议注入 | 与产出协议并列进员工身份：开始写 `doing`、干完写 **`review`**、**"不要写 done"**、交付了把产出 id 填进 `deliverables`。有测试钉住它真的进了注入文本，且顺序是 身份 → 产出协议 → 任务协议 |
| **派活** | 命令面板「派活到当前终端」：把最早的未完成任务正文填进活跃 Slot。**不代按回车** —— 自动发送等于让人一按就触发一段自己没审过的指令 |
| 任务裁决门 | 「任务裁决」对话框：摆出负责人/状态来源/关联产出与证据验证情况，人点「完成 / 退回重做」 |
| 诚实提示 | 关联产出被接受但**证据没验过**时，明确写出来 —— 否则那一行会被读成"证据也过了" |

**顺手做掉的规则 7 现场**：`insertAgentMention`（点名）与新写的 `sendTaskToSlot`（派活）本质是同一件事（人主动往当前会话塞一段字）。原先"写入活跃 PTY"的路径（`mention.sendInput` 优先、`ptyWrite` 兜底）只存在于点名里。这一刀把它抽成 `writeToActiveSlot` 单一实现，两条路共用 —— 否则兜底逻辑迟早漂移。重构后 284 个既有 JS 测试**全绿**，说明语义未变。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **138 passed / 0 failed**（本轮 +20） |
| `npm test` | **293 passed / 0 failed**（本轮 +9） |
| `cargo check` | 无告警 |
| **agent 自报 done 不被采信** | 专测：状态仍按声明走，但被标记"未经人裁决"，且不出现在已完成组 ✅ |
| 阻塞项解析 | 阻塞任务被**人裁决**完成后才解除；引用不存在的任务永远算未解决并报出 ✅ |
| 产出关联 | 现场派生；产出被驳回后"全部已验收"立刻变 false ✅ |
| id 安全 | 中文标题→时间 id、标题带 `../../etc/passwd` 折成安全 id、同名自动避让 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ⑤ 现在的完成度

| 要素 | 状态 |
|---|---|
| **任务板** | ✅ 任务定义 + 人的裁决 + 派生事实（阻塞/产出/证据） |
| **调度** | ✅ 派活到当前员工的 Slot（填字不代发）；❌ 自动排队/并发上限/按能力选人 |
| **审查队列** | ✅ 任务的 review 状态 + 产出审查 + 证据验证，三个门都是人触发 |
| 账本（成本/产出比） | ❌ **一行代码都没有** —— 依赖 token/成本数据，而这个仓目前没有预算概念 |
| 任务的自动推进 | ❌ 刻意没做：状态变化必须由人触发，避免静默改状态 |

### 10.9 第八刀：账本——先量清楚，再谈成本（⑤ 的第四块，2026-10-14 已落地）

账本分两侧，两侧都只记**可测量的事实**：

| 侧 | 数据从哪来 |
|---|---|
| 产出侧 | 我们自己的记录（任务/产出/验收/证据）→ 完成率、验收通过率、证据通过率、**产出有没有挂到任务上**、谁产出的 |
| 机时侧 | 本机 transcript 的 `message.usage` → token 用量，按模型拆分、子代理按执行者归属 |

#### 先说结论：**金额算不出来，所以不报**

实测本机 transcript：`usage` 里只有 `input_tokens` / `output_tokens` / `cache_creation_input_tokens` / `cache_read_input_tokens`，**`total_cost_usd` / `costUSD` 出现 0 次**。
算金额需要一张价格表（models.dev 的 `cost` 字段可以生成）。**在接上价格表之前只报 token，并把"为何没有金额"写进报告** —— 编一个数字出来比不报更糟。

#### 四个实测发现（都是先量后写，不是设计猜想）

**① 调度台的开销与你的开销是两个账本。** 槽位跑在隔离配置目录里，transcript 落在
`~/.onecode/cc-config/projects`；你自己手敲的落在 `~/.claude/projects`。混在一起就答不了
"这个调度台花了多少"。实测：**调度台全库 13.0B tokens ｜ 你自己 7.98B**。

**② 子代理的用量不在主转录里 —— 不下钻就是"结构性看不见"，不是"差一点"。**
主转录里 `isSidechain:true` 出现 **0 次**，而 `<sessionId>/subagents/agent-*.jsonl` 下有
**900 个**文件。我第一版只扫项目目录下的一层 `*.jsonl`，于是"子代理占多少"永远是 0。
修法：下钻一层，按**目录位置**判定子代理（不依赖 `isSidechain`，早期版本可能不写）。
这些文件还带 **`attributionAgent`** → 可以算出**每个员工的子代理开销**（实测归属到 `claude-code-guide`）。

**③ 顺手发现一个既有 bug。** `cc_sessions::decode_project_dir` 把目录名里的 `-` 一律还原成 `/`，
于是 `-Users-...-eng-onecode-desktop` 会被解成 `.../eng/onecode/desktop`（错的）。
那条路径只用于展示，本账本**不依赖它**：改用**正向编码 + 精确匹配**（`cwd.replace('/','-')`，已实测命中）。

**④ 完整扫描要 15 秒，而且是 I/O 瓶颈不是 JSON 瓶颈。**
我加了"先子串预筛 `"usage"` 再解析 JSON"的优化，2700+ 个文件只从 18.2s 降到 15.5s。
所以账本做成**两条路**：默认**只读本项目**（实测 **246ms**），全库合计是**显式的慢路径**；
没算的时候报告里写**"本次未算"并给出文件数**，绝不糊一个假数。

#### 账本真实长相（本机实测）

```
── 机时（token，本机 transcript 实测）──
调度台（隔离配置）  会话 0 ｜ 消息 0 ｜ 输入 0 ｜ 输出 0 ｜ 缓存写 0 ｜ 缓存读 0
你自己手敲的        会话 3 ｜ 消息 473 ｜ 输入 865.2k ｜ 输出 522.1k ｜ 缓存写 0 ｜ 缓存读 48.54M
  全库合计**本次未算**（完整扫描约 15 秒，是显式动作）。
     文件数：调度台 106 会话 / 442 子代理 ｜ 自己的 313 会话 / 900 子代理

按模型：
  deepseek-v4-flash  473 条消息 ｜ 输入 865.2k ｜ 输出 522.1k

子代理按执行者：
  claude-code-guide  56 条消息 ｜ 输入 246.5k ｜ 输出 24.0k

子代理 vs 主对话：子代理 2.15M ｜ 主对话 47.78M（子代理占 4%）

💰 金额未计算：本机 transcript 里没有成本字段（实测 total_cost_usd/costUSD 出现 0 次），
算金额需要一张价格表（models.dev 的 cost 字段可生成）。所以这里只报 token，不编金额。
```

注意 `缓存读 48.54M` 与 `输入 865.2k` 差了 56 倍 —— 这就是为什么账本**把缓存读单独一列**，
而不是加进一个"总 token"。混在一起只会让人误判开销结构。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **152 passed / 0 failed**（本轮 +14） |
| `npm test` | **331 passed / 0 failed** |
| `cargo check` | 无告警 |
| **真实快路径耗时** | **246ms**（断言 < 3000ms，防止退化回全库扫描） |
| **真实全库合计** | 调度台 13,019,069,892 ｜ 自己的 7,985,135,998 tokens ✅ |
| 真实子代理归属 | 归到 `claude-code-guide`（56 条 / 输入 246.5k）✅ |
| 快路径未算全库 | `Option::None` + 报告写"本次未算"（不糊假数）✅ |
| 慢路径 | 默认**不跑**（`ONECODE_TEST_LEDGER_FULL=1` 显式开），免得拖慢日常测试 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ⑤ 现在的完成度

| 要素 | 状态 |
|---|---|
| 任务板 | ✅ |
| 调度（派活） | ✅ 填字不代发；❌ 自动排队/并发上限/按能力选人 |
| 审查队列 | ✅ 三个门都是人触发 |
| **账本** | ✅ 产出侧 + 机时侧（token，按模型/按执行者/子代理 vs 主对话）；❌ **金额**（需价格表）、❌ 按任务的成本归集 |

### 10.10 第九刀：按员工自选模型 + failover（② 的缺口，2026-10-14 已落地）

② 的机制从第 2 轮就通了（`apply_route_env` 是每槽路由真正生效的地方），但**界面上没有任何地方能给一个员工选模型** —— 目标原话是「按员工/任务**自选模型**」。这一刀补上，并且把从第 2 轮就落盘、却一直没人消费的 failover 骨架接上。

| 层 | 内容 |
|---|---|
| 花名册 | `providers.json` 的 `catalog.roster`：`agent_id → {provider_id, model}`。**不写进 agent 的 md frontmatter** —— 那是 Claude Code 的约定文件，在里面写死别家模型名，换引擎就会把错误模型名喂给不认识的 provider |
| 解析 | `resolve_route_for_agent`：**显式指定 > 花名册 > 全局**，然后做可用性检查 |
| failover | 主选不可用时沿 `catalog.failover_queue` 找第一个可用的 —— **消费了从第 2 轮就存在的 `failover_queue` / `auto_failover_enabled`**（当时只落了盘没人读），并把 `history` 真正写起来（`record_switch`） |
| 报告 | `SpawnResult` 增 `route_source`（requested/roster/global）、`switched_from`、`switch_reason`；终端里显示；花名册把**绑定**与**实际**并排显示 |
| 入口 | 命令面板 `/roster` 两级：选员工 → 选供应商；另有「员工模型」入口与「查看花名册」对话框 |

#### 四条设计决定

1. **failover 只做静态检查**：在目录里 + 有凭据。**不发网络请求** —— 每次 spawn 探一次网既慢又会被网络抖动带偏；真正的"连不上"由引擎自己的报错暴露，用户看得到、也换得回来。
2. **`auto_failover_enabled = false` 时不换，但必须说出原因**（"主选不可用；自动切换已关闭"）—— 关掉开关不等于可以让问题隐形。
3. **花名册里的模型名不跟着 failover 走**。花名册绑的是 A 的模型，failover 到 B 时若把它硬塞过去，就是把 A 的模型名喂给 B。有专测钉住。
4. **路线必须有来源且来源必须真实**：`source` 与实际凭据来源必须一致（见下）。

#### ★ 测试抓到一个"来源标注与实际不符"

我第一版的写法是"花名册指向不存在的 provider → 直接换成全局路线"。而全局恰好可用 ⇒ 可用性检查通过 ⇒ **failover 根本不触发**，结果却把 `source` 标成 `Roster`：

> 也就是**实际走了全局，却告诉你是花名册**。

这类"标注与实际不符"是最难查的一种错（界面上一片正常）。修法：把"**想用谁**（intended）"与"**想用的那个坏了**（broken + 原因）"分开记，身份标签（origin）**显式携带**而不是从错误文案里反抠字符串（那种写法一改措辞就悄悄失效）。回归测试钉住：用了全局凭据就绝不能标成花名册。

#### ★ 本地模型档：一个与目标直接冲突的问题

目标 ② 明确要求「**含本地模型档**」。而我的可用性检查一律要求有凭据 —— 本地推理服务（Ollama / LM Studio / vLLM）**通常没有 api_key**。后果不是"多一句提示"，而是：

> 本地档被判不可用 → 开着 auto_failover → **被静默换到云上**，正好与"我要用本地模型"相反。

修法：`is_local_endpoint`（localhost / 127.0.0.1 / 0.0.0.0 / [::1] / host.docker.internal）不要求凭据。

**并且前端有两处各抄了一份"有没有 key"的判断**（规则 7 现场）：
① 花名册选择器把本地档标成"无凭据"；② **新建供应商时的"测试连接"强制要求 API Key** —— 等于让人编个假 key 才能加本地模型。

修法：把可用性判断收进后端算好的 `ProviderCatalogView.usable` / `unusable_reason`（这个视图本来就是"算好的事实"的载体，已有 `windows` 映射），前端只读不算；创建校验只强制 Base URL。

#### 顺手做掉的规则 7 清理

`resolve_route` 被 `resolve_route_for_agent` 完全覆盖（后者是前者的超集），于是**删掉它、把它的测试迁到新函数上** —— 现在路线解析**只有一份实现**。

#### 花名册在人眼前长什么样

```
花名册（2 个员工绑定了自己的路线）

@dev → glm
      绑定模型：glm-4
      实际：glm / glm-4（来源 roster）

@researcher → kimi
      实际：deepseek / deepseek-chat（来源 global）
      ⚠ 已从 kimi 切走：kimi 没有凭据（api_key 与 extra_env 都是空的） → 已按撤退队列切到 deepseek

可选的供应商：glm、kimi(不可用)、deepseek
```

`@researcher` 那一行就是这一刀的全部意义：**我设了，但没生效，而且我知道为什么。**

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **173 passed / 0 failed**（本轮 +21） |
| `npm test` | **293 passed / 0 failed** |
| `cargo check` | 无告警 |
| 本地模型档 | 指定 localhost 的 provider 无凭据也保留、**不被 failover 换走**；云端无凭据仍算不可用 ✅ |
| 来源标注一致性 | 回归测试：用了全局凭据就不得标成 roster ✅ |
| failover 关掉时 | 不换，但如实报"自动切换已关闭" ✅ |
| 撤退队列耗尽 | 报出试过谁、谁不在目录里 ✅ |
| 本地档前端显示 | 可用性来自 `ProviderCatalogView.usable`，两处 JS 重复判断已删 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ② 现在的完成度

| 要素 | 状态 |
|---|---|
| 每槽 provider/model 落到 env | ✅（第 2 刀） |
| **按员工自选模型 + 界面入口** | ✅ 花名册 + `/roster` + 实际路线可见 |
| **本地模型档** | ✅ 后端不要求凭据；前端不再误判 |
| failover | ✅ 静态可用性 + 撤退队列 + 历史；❌ 失败计数/熔断（`failover_params` 仍未消费） |
| 按**任务**覆盖模型 | ❌ 命令层参数已通，但没有"任务带模型"的入口 |
| 窗口表（models.dev） | ❌ 仍靠型号后缀 + 内置表 + 手动覆盖 |

### 10.11 第十刀：可扩展引擎定义（① 的第一个可实现步骤，2026-10-14 已落地）

① 的说法是「**可换引擎**的调度台」，但实现上引擎是**硬编码在 Rust 里的 7 个**——加第 8 个要改代码重编译。那就不叫可换。这一刀把它变成配置。

| 层 | 内容 |
|---|---|
| 文件 | `~/.onecode/engines.json`：`{"engines":[{...}]}` |
| 继承 | `"base": "claude-code"` = 从内置档继承，**只写要覆盖的字段** |
| 关能力 | 空串（`"system_prompt_file_flag": ""`）= **明确表达"这个引擎没有这个能力"** |
| 覆盖 | 用内置 id 写定义 = 覆盖它（改 `cmd`/参数时最方便） |
| 来源 | `BackendSource{Builtin, User}` —— 界面区分"内置的"与"你自己加的" |
| 报告 | `engines_problems()` 命令 + 引擎列表对话框里显式的「⚠ 定义有问题」段 |

**为什么 `base` 继承是必需的**：绝大多数自定义引擎只差一两项（换 `cmd`、换显示名）。让用户把 claude-code 的 `--append-system-prompt-file` / `--permission-mode` / `--allowedTools` 整套抄一遍，**必然抄错**，而且引擎升级后不会跟着变。

示例（界面里可直接抄）：

```json
{ "engines": [
    { "id": "claude-via-acp", "base": "claude-code", "cmd": "claude-agent-acp",
      "display_name": "Claude Code (ACP)" },
    { "id": "local-qwen", "base": "codex", "cmd": "qwen", "display_name": "Qwen CLI (本地)" }
] }
```

#### 做到的诚实性

- **坏定义一律上报，绝不静默丢**：不合法 id、重复定义、`base` 找不到、整份 JSON 坏了 —— 四种都有问题记录（"base 找不到"还会说明"已从零开始"，不让用户以为继承了能力）。
- **必定失败的条目不进选择器**：没有 `cmd` 的引擎**跳过并上报**（与"不安全 id / 重复定义"一致）。让一个永远启动不了的条目出现在引擎选择器里，比不显示更糟。

#### ★ 测试抓到一个"凭空编命令名"

`blank_profile` 原先把 `cmd` 默认成 `id`。后果不只是"少报一个错"：

> `{"id":"qwen"}` 这种没写 `cmd` 的定义会被当成 `cmd: "qwen"` —— **系统凭空编了一个命令名**。用户若是 `qwen-cli`，spawn 只会给一个难懂的 OS 报错；而"没有 cmd"这条检查**永远不会触发**。

修法：`cmd` 留空、明确上报。`display_name` 默认成 id 是无害的展示信息，可以兜。

#### 也是测试逼出来的一个隔离问题

`resolve()` 会读**真实的** `~/.onecode/engines.json`。测试若用它，**用户一旦覆盖了内置档，测试结果就跟着变**（和 `providers.json` 那一课同源）。所以：

- 删掉只被测试用的 `resolve()`（避免死代码，也避免这个入口被顺手用错）；
- 测试用 `builtin(id)`（只查内置）+ `Registry::load_from(&临时路径)`；
- 验证过：测试跑完 **没有创建** `~/.onecode/engines.json`。

#### 重构（本次改动的主体）

`BackendProfile` 的字段从 `&'static str` 全改 **owned**（定义要能来自配置），`PROFILES` 常量 → `builtin_profiles()` 函数，`apply_route_env` 的键表随之改 owned。**内置档行为一字未变** —— 既有的 backend 测试就是回归证明（claude-code 的三个 flag、`env_key_map`、`needs_auth_fix` 逐项断言）。

#### 引擎列表在人眼前长什么样

```
引擎（8 个）

✅  claude-code  (内置)
      Claude Code → claude
      能力：身份注入 / 权限档 / 工具白名单
✅  opencode  (内置)
✅  crush  (内置)      … 共 7 个内置
✅  mine  (**你自己加的**)
      Mine → mine

⚠ 定义有问题（1 条，未生效）：
  bad：没有 cmd，启动必然失败，已忽略

加引擎：写 ~/.onecode/engines.json
（形状见「引擎配置示例」）
```

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **185 passed / 0 failed**（本轮 +12） |
| `npm test` | **293 passed / 0 failed** |
| `cargo check` | 无告警 |
| **内置档零变化** | 无配置文件时 7 个档位逐字段断言（含 claude-code 三个 flag）✅ |
| base 继承 | 只改 `cmd` → 其余能力（身份注入/权限档/工具白名单/default_args）全部继承 ✅ |
| 空串关能力 | 能关掉继承来的 flag，未写的仍继承 ✅ |
| 用户定义驱动 spawn | 继承来的权限能力在新引擎上仍生效（`apply_posture_args` 实测）✅ |
| 四类坏定义 | 不安全 id / 重复 / base 找不到 / JSON 坏 → 全部上报且内置档不受影响 ✅ |
| 测试隔离 | 用临时路径；未创建真实 `engines.json`；真实 `providers.json` sha256/mtime 未变 ✅ |

#### 目标 ① 现在的完成度

| 要素 | 状态 |
|---|---|
| 引擎抽象（7 个内置 + 权限/身份/工具能力声明） | ✅（第 1、3、4 刀） |
| **可扩展引擎定义（不需重编译）** | ✅ 配置文件 + base 继承 + 关能力 + 问题上报 |
| **ACP 类引擎** | ⚠ 配置层已能接（示例里就给了 `claude-agent-acp`），但**没有 ACP 客户端** —— 仍靠"把 ACP 桥当普通 CLI 起" |
| 引擎能力协商（自动探测某引擎支持什么 flag） | ❌ 全靠定义里手写 |
| 项目级 `engines.json`（按项目换引擎） | ❌ 现在只有用户级 |

### 10.12 第十一刀：问引擎自己（① 的能力协商）+ 修掉一个已发布的致命 bug

这一刀本来打算写 **ACP 客户端**。**先取证，结论是不能写**：

| 取证 | 结果 |
|---|---|
| 本机有没有 ACP 桥 | `claude-agent-acp` / `codex-acp` / `gemini` / `acp` **全都没有** |
| Claude Code bundle 里有没有 ACP | `agent-client-protocol` / `session/new` / `session/prompt` / `session/update` 出现 **0 次** |

没有对端、也没有可核对的实现细节 —— 从记忆里写一份 ACP 协议，产出的就是 **[报道] 而不是 [实测]**，正是这个项目一直拒绝的东西。所以改做**本机存在且可端到端验证**的结构化协议：`--output-format stream-json`（bundle 里出现 23 次）。

#### ★ 顺手抓到一个已发布的致命 bug：`planMode` 是非法值

第一次按第 3 刀的实现去验证姿态接线，CLI 直接回了：

```
error: option '--permission-mode <mode>' argument 'planMode' is invalid.
Allowed choices are acceptEdits, bypassPermissions, default, dontAsk, plan.
```

也就是说：**任何声明 `permission: planMode` 的员工，终端会秒退、根本起不来**。而我在第 3 刀里把 `planMode` 当成合法值写进了代码 —— 当时的依据是"从 bundle 里看到的观察值"，不是"CLI 接受的取值"。

修法两层：

1. **取值改正**：`Plan` → `"plan"`；顺带发现并补上一个实测才知道存在的档位 **`dontAsk`**。旧配置里的 `planMode` 仍然被解析（兼容），但输出统一是 `plan`。
2. **结构性防护（更重要）**：引擎的合法取值是**版本相关**的，所以让 profile 自己声明白名单
   `permission_mode_values`，`apply_posture_args` 在**参数生成阶段**校验 —— 不接受就**如实报告并拒绝递出**，而不是让用户面对一个秒退的终端。用户也能在 `engines.json` 里覆盖这个白名单（引擎升级后取值变了就改这里）。

#### 顺带解决了第 1 轮遗留的 [未验证]

第 1 轮把 `--append-system-prompt-file` 标成 [未验证]（那是整个项目里唯一一个"基于帮助文本推断、没能对着真 CLI 跑"的东西）。这次实测：**被接受，没有 unknown option 报错**；`--allowedTools "Read,Bash(git:*)"` 同样被接受。→ 该条目可以转为 **[实测]**。

#### 能力协商：问引擎自己

```text
<engine> -p ok --output-format stream-json --verbose
```

会在**推理之前**先吐一行 `system/init`。三件事因此成立：

1. **探测不花 token**：init 之后我们**立刻杀掉整棵进程树**（复用 `pty::kill_process_tree`），不进入推理。
2. **它能验证我们的注入**：`apiKeySource` 就是引擎实际用的凭据来源；`permissionMode` 就是它实际采纳的权限档 —— 这比"我们以为传对了"强得多。
3. **能力从手写变实测**：引擎到底加载了哪些 tools/agents/skills/MCP，只有它自己说了算。

真实探测输出（本机，**539ms**）：

```
引擎 claude-code 的能力协商

命令：node …/claude-code/cli.js -p ok --output-format stream-json --verbose
耗时：539ms

✅ 引擎自己报告：
  模型：claude-sonnet-4-6
  权限档：default（这是对注入参数的**实测回执**）
  凭据来源：ANTHROPIC_API_KEY（验证 env 注入是否生效）
  版本：2.1.76｜输出风格：default
  能力：工具 25 ｜ agents 4 ｜ skills 5 ｜ slash 15 ｜ MCP 0 ｜ 插件 0
  工具：Task, TaskOutput, Bash, Glob, Grep, ExitPlanMode, Read, Edit, Write, …
```

**顺带印证了"能力必须实测"**：这次探测落到的 bundle 是 **2.1.76**（25 个工具），而先前手工探的是 **2.1.63**（21~22 个工具）—— 同一天、同一台机器、**工具集就不一样**。手写的能力声明迟早是错的。

不支持这个协议的引擎**如实报告不支持**（"有输出但 20 秒内没有 init 事件"／"没有任何输出"），绝不假装知道它的能力。

#### 过程中的一个自伤：测试套件从 1.1 秒变成 30 秒

真实探测测试一开始**优先用 PATH 上的 `claude`** —— 那正是第 1 轮记录过的 Go launcher，会卡在版本拉取上。于是"探测"变成"等满 30 秒超时"。改成**优先用已验证可用的官方 bundle 路径**（`node cli.js`），PATH 只作兜底 → 套件回到 **0.58s**。

（这本身也是个产品信号：如果用户 PATH 上的 `claude` 是那个 launcher，探测就慢。但结论不是"换成 cli.js"，而是**探测必须有超时并如实报告** —— 已经有。）

#### 测试隔离（零成本、零副作用）

真实探测测试用 **假 key + 不可达 base_url（`http://127.0.0.1:9`）+ 临时 `CLAUDE_CONFIG_DIR`**：认证失败发生在推理之前，所以**不花 token**，也**不碰用户配置**。验证过真实 `providers.json` 的 sha256/mtime 未变。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **195 passed / 0 failed**（本轮 +10） |
| `npm test` | **293 passed / 0 failed** |
| `cargo check` | 无告警 |
| **真实 init 夹具** | 用本机实测抓下的 `system/init` 整行当测试夹具（只截短长数组）✅ |
| **真实引擎探测** | 539ms 拿到模型/版本/权限档/凭据来源/工具/agents/skills ✅ |
| `planMode` 非法 | `apply_posture_args` 拒绝递出并报出可用值；参数里绝不出现它 ✅ |
| 白名单覆盖 | 五个合法档位逐个断言能被接受；白名单来自实测报错原文 ✅ |
| 桩引擎 | spawn → 读 init → 立刻杀，不等桩睡满 60 秒（实测 <10s）✅ |
| 不支持时 | 如实报"不支持"，`supported=false`，不假装 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ① 现在的完成度

| 要素 | 状态 |
|---|---|
| 引擎抽象 + 可扩展定义（第 9 刀） | ✅ |
| **能力协商（问引擎自己）** | ✅ 结构化探测：模型/权限档/凭据来源/工具/agents/skills |
| **`--append-system-prompt-file` / `--allowedTools` 实测可用** | ✅ 第 1 轮的 [未验证] 已消除 |
| 权限档取值与引擎对齐 | ✅ 白名单 + 拒绝非法值（不再秒退） |
| **ACP 客户端** | ❌ 本机无对端、无 ACP 桥 → 不写（见上）；配置层已能接 ACP 桥当普通 CLI |
| 结构化（非 PTY）Slot | ❌ 探测已证明协议可用，但 Slot 仍只支持 PTY |
| 项目级 `engines.json` | ❌ 只有用户级 |

### 10.13 第十二刀：工作台面板（④⑤ 从"有命令"到"能日常用"）

前十一刀把 ④⑤ 的能力都做出来了，但**入口全在 ⌘K 面板和对话框里**。而"管理"要的是**常驻面板**，不是一个个弹窗 —— 这一刀把右栏从"只有文件"变成"文件 + 工作"。

| 件 | 说明 |
|---|---|
| 面板 | 右栏新增「工作」页：**产出·待验收**（审查队列）+ **任务·未完成** + 一行摘要（待验收 / 未完成 / 证据未验证） |
| 动作 | 每条产出：「审查 / 裁决」「跑证据」；每条任务：「派活到当前终端」「裁决」—— 全部转发到已有命令，面板自己不做判断 |
| 入口 | ⌘K →「工作台」（会先展开右栏再切页，否则"点了像没反应"）；tab 选择持久化 |
| 分工 | 见下（这一条比代码本身重要） |

#### ★ 分工：**判断在 Rust，排版在 JS**

面板里**没有**任何"验过没有 / 完成没有 / 文件在不在"的判断逻辑。这些全部由 Rust 算好放进返回值：

| 事实 | 谁算的 | 字段 |
|---|---|---|
| 证据验过没有 | Rust | `verification`（`null` = 没验过） |
| 完成是不是人裁决的 | Rust | `status_source`（`human` / `declared`） |
| 声明的文件存不存在 | Rust | `files[].exists` |
| 证据能不能机器验证 | Rust | `evidence[].grade` |
| 记录本身有没有问题 | Rust | `problem` |

为什么这条纪律值得单列：第 9 刀的前端**就是因为自己抄了一份"有没有 api_key"的判断**，把本地模型档误判成"无凭据"。同一判断两处实现（规则 7）在 UI 上尤其危险 —— 后端修好了，界面还在显示旧口径。

所以 `work-panel.js` 里只有格式与排序，没有判断；测试也照这个口径写（用 Rust 返回值的形状当输入，断言面板只做**转述**）。

#### ★ 顺手发现并修掉一个会"对半切"的布局 bug

加第二页时注意到：`.fe-tab-content` 的隐藏方式是 `visibility: hidden`，而 **`visibility: hidden` 的元素仍然参与布局**。`#filePanel` 是 `display:flex; flex-direction:column`，两个 `flex:1` 兄弟 → 可见那一页只剩**一半高度**。

之前只有一页，所以这个坑没暴露过。修法：`.fe-tab-content:not(.active) { display: none }`（非激活页完全不占布局）。

> 证据分级：这一条是 **[推断]**（依据 CSS 布局语义：`visibility: hidden` 不脱离布局流），不是渲染实测 —— 我在这个环境里没法渲染 Tauri WebView 截图确认。采用的修法是**保守的那个**（`display:none` 在任何解释下都不会占位）。

#### 顺手清掉一处死代码

`switchRightTab` 原先把 `_activeRightTab` 硬编码成 `'file'`（当时只有一页，是退化的）；`localStorage.getItem('fe-active-tab')` **只读不写** —— 那个读取永远拿不到第二个值。现在切换是真的了：写入补上，闭环。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **195 passed / 0 failed** |
| `npm test` | **307 passed / 0 failed**（本轮 +14） |
| `cargo check` | 无告警 |
| 面板口径 | 14 条纯函数测试：待验收队列只含未处理、**"声明了证据"与"验过了"可区分**、文件缺失/问题显眼、空项目给出下一步、缺字段不炸、**标题转义**（防 agent 写的 HTML 进 DOM）✅ |
| 无参数 / 缺字段 | `buildWorkHtml()` 不炸 ✅ |
| HTML/CSS 结构 | 两个 tab + 两个内容容器；非激活页不占布局 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

> 未做：**渲染实测**。面板的真实观感需要在 GUI 里看一眼（`make dev` 或已装的 app）。这一刀验证到"数据口径 + DOM 结构 + 纯函数排版"，视觉层没有截图证据。

#### 目标 ④⑤ 现在的完成度

| 要素 | 状态 |
|---|---|
| ④ 真文件 / 证据分级 / 人机验收 / 实测验证 | ✅ |
| ④⑤ **常驻面板** | ✅ 右栏「工作」页（审查队列 + 未完成任务 + 摘要） |
| ⑤ 任务板 / 调度 / 审查队列 / 账本 | ✅ |
| 批量操作（多选验收/批量验证） | ❌ 逐条操作 |
| ⑤ 调度自动化（排队、并发上限、按能力选人） | ❌ 现在是手动派活 |
| 归档 / 成本并入产出台账 | ❌ |

### 10.14 第十三刀：渲染实测 —— 第一次真的看它一眼，就抓到三个 bug

第 11 刀交付面板时明确留了一句话：**"渲染实测未做"**。这一刀把它补上 —— 而 12 轮里**从没渲染过的 UI**，第一次渲染就暴露了三个问题，其中一个是治理性质的。

#### 怎么做到的

环境里有 Google Chrome，于是：

1. 写一个**渲染夹具** `tests/render/work-panel.html`：把工作台挂进**真实的右栏结构**（`.fe-tabs` + 两个 `.fe-tab-content` + 同一条 flex 链），数据用**真实形状**（字段名与 Rust 返回值一字不差），并刻意放进三种最容易骗过人的情况：缺文件、证据没验过、agent 自报完成；
2. `chrome --headless=old --dump-dom` 取**量化数字**（面板把测量结果写进 DOM）；
3. `chrome --headless=old --screenshot` 出图，用眼睛看。

> 两个踩坑：`--headless=new` 会因 GPU 进程直接崩（`GPU process isn't usable`），要用 `--headless=old`；而 `file://` 下 Chrome **禁止 ES module 导入**，夹具必须走本地 HTTP 服务。

#### ★ bug 1（治理性质）：agent 自报完成的任务**整个从面板消失了**

面板按 `status` 做过滤：`未完成 = 所有 status 不在 {done, dropped} 的任务`。于是一条"agent 自己把状态写成 done、但没人裁决过"的任务 —— **既不在未完成列表里，也不在任何地方**。

摘要显示"未完成任务 2"，而实际有 3 条任务。**"它自称完成了"这件事恰恰没人看得见** —— 而这正是 ④ 里"模型自述不算证据"在 ⑤ 上的对应物，是这一整块最该被看见的信号。

修法：单独立一组

```
任务 · AGENT 自报完成（未经你裁决）
这些任务的完成是「文件里写的」，不是人裁决的 —— 按我们的规矩，它还不算完成。
```

并加摘要 chip `自报完成待裁决 N`。**次序**也调了：它和审查队列同属"等人裁决"，必须排在长列表**之前**（否则面板一长没人往下滚，信号就被埋了）—— 这一条现在有测试钉住位置。

#### ★ bug 2：markdown 星号进了 HTML

产出行的文案写的是 `证据**未验证**`（从 Rust 的 `review_text` 抄来的写法）。在 HTML 里 `**` 不是加粗，**就是两个星号字面显示**。渲染图上看得一清二楚。

修法：改成 `<b>未验证</b>`，并加一条测试：**面板产出的 HTML 里不允许出现 `**`**。

#### ★ bug 3：过期响应会覆盖新内容

夹具里空项目那格显示的是**满内容**而不是"没有活跃终端"。根因是夹具自己重复挂载导致两个 `refresh` 交叠 —— 但顺着查下去，**产品代码有同一个毛病**：`refresh()` 在开始时读一次 `cwd()`，await 回来后直接写 innerHTML。期间切了终端/切了 tab，就会**用旧项目的响应覆盖新内容**。

修法：递增序号 + 写回前复核 `cwd()`，过期响应直接丢。

#### ★ 顺带把上一刀的 [推断] 变成 [实测]

第 11 刀判断"`visibility:hidden` 仍占布局，两个 `flex:1` 兄弟会把面板高度对半切"，当时标了 **[推断]**（渲染不了）。现在有数字了：

```
PASS  LAYOUT_FIX   可见内容高 692 vs 修复前 346（隐藏兄弟页 346）
```

692 与 346 —— 正好 2 倍。修复合乎预期，判断也被证实。

#### 做成可重复的检查

`npm run test:render` 会起临时静态服务、跑 7 项检查、出一张截图。**不放进 `npm test`**（它需要 Chrome 与临时服务，太重），需要时显式跑。

```
PASS  LAYOUT_FIX             可见内容高 692 vs 修复前 346（隐藏兄弟页 346）
PASS  NO_LITERAL_MARKDOWN    页面文本里不该出现 markdown 星号
PASS  SELF_CLAIMED_VISIBLE   agent 自报完成必须另立一组可见
PASS  EMPTY_STATE            空项目应提示先开终端
PASS  REVIEW_QUEUE           待验收队列口径
PASS  UNVERIFIED_MARKED      未验证要标出来
PASS  SELF_CLAIMED_ORDER     自报完成要排在待验收队列之前
RESULT: ALL_CHECKS_PASS
```

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test`（并行 / 串行） | **195 passed / 0 failed** |
| `npm test` | **309 passed / 0 failed**（本轮 +2） |
| `npm run test:render` | **7/7 通过** + 截图 ✅ |
| 渲染实测抓到的三个 bug | 星号 / 自报完成消失 / 过期覆盖 —— 全部修掉并有测试钉住 ✅ |
| 上一刀的布局 [推断] | 升级为 **[实测]**（692 vs 346，2 倍）✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

> **仍未做**：整个 app 的端到端渲染（`make dev` 开真窗口）。这次测的是**面板在真实 CSS 结构里的渲染**，不是 Tauri WebView 里的整机行为 —— 终端里那些诚实提示（"身份未注入""已从 X 切走"）仍然只有代码与日志证据。这个缺口保留在文档里，不假装它已经补上。

### 10.15 第十四刀：把最核心那条链路变成可测的，并第一次跨进程验证它

十三刀下来，产品最核心的一条链路 —— **点击员工 → 身份注入 / 权限姿态 / 技能组合 / 每槽路由 / failover** —— **一个测试都没有**。原因是它整段住在 `commands::pty_spawn` 的函数体里，而签名上挂着 `State<'_, MultiPtyManager>` / `ConfigManager` / `ProviderStore`，脱离 Tauri 就跑不起来。

这一刀把它抽出来：`spawn_plan::plan()` 是**纯函数**，`pty_spawn` 缩成"生成计划 → `state.spawn`"。

#### 抽取带出的一个顺手的修正

`spawn_plan` 里那句"无法识别的 permission 值"提示，**还在把 `planMode` 列为可用值** —— 第 11 刀修掉了值本身，却漏了这句文案（典型的"改了一处没搜调用点"）。现在列的是 CLI 真正接受的五个，并且有测试钉住**文案里不许再出现 `planMode`**。

#### ★ 第一次跨进程验证这条链路

前面的测试都只断言"计划里有什么"。新加的这条**真的起一个进程**：桩引擎把收到的 argv 与关键 env 落盘，测试再读回来核对。

桩引擎实际收到的（测试真实打印）：

```
ARG --permission-mode
ARG plan
ARG --allowedTools
ARG Read,Bash(git:*)
ARG --append-system-prompt-file
ARG …/dsh-plan-prompts-e2e-…/dev-66b4d058a8e2649a.md
ENV ANTHROPIC_API_KEY=key-roster-prov
ENV ANTHROPIC_BASE_URL=https://roster-prov.example/v1
ENV ANTHROPIC_MODEL=roster-model-2
ENV PATH_PRESENT=yes
```

一句话这就是 ①③ 的落地证明：**姿态用的是 `plan`（不是会让 CLI 秒退的 `planMode`）、白名单到了、身份文件路径到了、凭据来自花名册那条路线、且调用方原有的环境没被冲掉**。同一测试还打开那个 prompt 文件，确认里面有**身份正文 + 技能正文 + 产出协议 + 任务协议**。

#### 抽取时撞上的测试隔离问题（值得记）

`write_prompt_file` 默认写 `~/.onecode/cc-config/agent-prompts` —— **在工作区之外**，本沙箱直接拒绝；而且就算能写，**测试也不该碰用户的真实目录**。

加了 `ONECODE_AGENT_PROMPTS_DIR` 覆盖缝（这缝有两个真实用途：隔离运行、以及有人想把生成物放到别处），并用一把本地锁串起用到它的测试（env 是进程级的）。**验证：跑完之后用户真实的 `cc-config/agent-prompts/` 仍然是空的（0 个文件）。**

#### 两条我自己的错误假设，被新测试纠正

- 以为"没指定 provider → 用目录里激活的那条"，实际是**用 `AppConfig` 的全局快照**（`extra_env`/`model` 都来自它）；
- 因此"供应商 `extra_env` 里声明 AUTH_TOKEN"那条，也只有**显式指定该 provider** 时才成立。

两条都是测试先错、代码对。这本身说明抽取有价值：这些行为以前只在注释里写着，现在被断言钉住了。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 ×2 / 串行 | **209 passed / 0 failed**（本轮 +14，三种模式一致） |
| `npm test` | **309 passed / 0 failed** |
| `cargo check` | 无告警 |
| **跨进程端到端** | 桩引擎收到 `--permission-mode plan` / `--allowedTools Read,Bash(git:*)` / `--append-system-prompt-file <真实文件>` / 花名册凭据 / PATH 保留 ✅ |
| 注入正文内容 | 身份 + 技能 + 产出协议 + 任务协议 全部在文件里 ✅ |
| 写错的文案 | 提示里不再出现 `planMode`（有测试钉住）✅ |
| 测试隔离 | 真实 `~/.onecode/cc-config/agent-prompts/` 跑完仍为 **0 个文件** ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 这个缺口补上意味着什么

以前每次改 `pty_spawn`（十三刀里改了六次）都是**盲改** —— 唯一的安全网是"能编译"。现在这条链路上有 14 条测试，其中 2 条跨进程。

> 仍未做：`state.spawn` 之后那一段（PTY 真起、xterm 真渲染、`@name` 在真终端里的观感）依旧只有代码证据。这次补的是**参数与环境到达进程**，不是"终端看起来对"。

### 10.16 第十五刀：账本的金额 —— 并且推翻我自己第 8 刀的结论

第 8 刀做账本时我写下："**金额算不出来**：本机 transcript 里没有成本字段（`total_cost_usd`/`costUSD` 出现 0 次），算金额需要一张价格表。"

**前一半对，结论错。** 那个结论只对 **transcript** 成立；我没去看**结构化协议的结算事件**。先从 bundle 取证：

```
…is_error:x.boolean(),num_turns:x.number(),result:x.string(),stop_reason:x.string().nullable(),
total_cost_usd:x.number(),usage:fj4(),modelUsage:x.record(x.string(),jj4()),…
```

`result` 事件里有 `total_cost_usd`、`usage`、**`modelUsage`（按模型拆分）**。跑一次真的确认：

> ⚠️ **成本披露**：为了取证我花了你 **$0.174335**（一次 `-p "Reply with exactly: ok"`）。
> 回复只有 2 token，但**输入 34,857 token**（系统提示 + 全部技能/agents 上下文）——
> 这个数字本身就说明账本有必要。

```
result 事件（本机实测）
  total_cost_usd  = 0.174335
  usage           = { input_tokens: 34857, output_tokens: 2, … }
  modelUsage      = { "deepseek-v4-flash[1m]": { costUSD: 0.174335,
                        contextWindow: 1000000, maxOutputTokens: 32000 } }
```

于是三件事一起成立，而且**都不用价格表**：

1. **金额引擎自己算**（它还知道缓存价与分档价）；
2. **按模型拆分**直接给；
3. **上下文窗口引擎自己报** —— 顺手关掉 ② 里"窗口表还靠型号后缀 + 内置表 + 手动覆盖"那项欠账。

#### 这一刀做出来的东西

| 件 | 说明 |
|---|---|
| `engine_run` | **结构化跑一次**：`-p <prompt> --output-format stream-json --verbose`，复用 `spawn_plan`（身份/姿态/技能/路由与终端槽位**同一套判断**），解析事件流 → 回复 + 金额 + 用量 + 按模型明细 + 引擎自报的窗口 |
| 记账 | 每次有 `result` 就写一行 `<cwd>/.onecode/runs.jsonl`（**失败也记** —— 失败的花费同样是真花费） |
| 账本 | 新增「金额（引擎自报，不是我们估的）」段：合计 / 平均 / **按员工** / **按模型** / 引擎自报的窗口；`cost_note` 连结论一起改成正确的 |
| 面板 | 工作台任务项多一个「结构化跑一次」按钮（跑完金额自动进账本） |

账本现在的样子（测试真实打印）：

```
── 金额（引擎自报，不是我们估的）──
运行 3 次 ｜ 合计 $0.350000 ｜ 平均 $0.116667
按员工：@dev $0.300000（2 次） ｜ @writer $0.050000（1 次）
按模型：deepseek-v4-flash[1m] $0.300000（2 次） ｜ glm-5.2 $0.050000（1 次）
引擎自报的上下文窗口：deepseek-v4-flash[1m] → 1000000 ｜ glm-5.2 → 1000000
```

#### ★ 同一个坑第二次踩：假超时

`engine_run` 的超时测试又跑了 **30 秒**才返回。根因和 **第 5 刀**在 `run_shell` 上一模一样：收尾时 `reader.join()` 会一直等读线程 → 而读线程在等管道 EOF → 而**孙进程**（脚本里的 `sleep`）还握着 stdout。本沙箱里 `/bin/ps` 不可用，进程树清扫降级为只杀 root，孙代就留下了。

修法也和第 5 刀一样：**有界排空**（轮询 `is_finished`，超时就不等），并在 stderr 尾部如实写"仍有子进程未退出，输出可能不完整"。测试的断言也都升级成硬不变量：**超时必须约束墙钟**（< 5s），而不只是"标记为超时"。

> ⚠️ **规则 7 欠账（记在账上）**：现在有**两处**各自实现了"spawn + 读取线程 + 有界排空"（`deliverables::run_shell` 与 `engine_run::run`）。这两份逻辑本该抽成一处。这一刀没动它（临近收尾做这种改动风险不划算），但它是明确的欠账，不是"没注意到"。

#### 顺带把一条过时的测试升级

旧测试 `ledger_text_never_shows_a_fake_amount` 钉的是"**报告里绝不出现 `$`**" —— 那是"金额算不出来"时代的纪律。金额现在能算了，纪律要升级成**更强**的那条：

> **没有运行记录时不许出现任何金额；有记录时金额必须等于记录之和。**

（顺带把 `cost_note` 文案里的 `$0` 也去掉了，好让这条不变量能用一条简单断言钉住；也去掉了里面的 markdown 星号 —— 那段是塞进对话框的纯文本。）

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 ×2 / 串行 | **222 passed / 0 failed**（本轮 +13，三种模式一致） |
| `npm test` | **309 passed / 0 failed** |
| `npm run test:render` | **7/7 通过** |
| 真实结算事件 | `total_cost_usd=0.174335`、`input_tokens=34857`、`contextWindow=1000000` ✅ |
| 真实事件当夹具 | `testdata/claude-code-run.jsonl`（本机实测整串事件）✅ |
| 金额聚合 | 合计 / 平均 / 按员工 / 按模型 / 窗口；**没记录时一个 `$` 都不出现** ✅ |
| 坏运行记录 | 计数上报（`runs_bad_lines`），不静默 ✅ |
| 超时墙钟 | 桩引擎 `sleep 30` + 600ms 超时 → **< 5s 返回**（曾 30s）✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ⑤（以及 ② 的一项）现在的完成度

| 要素 | 状态 |
|---|---|
| 任务板 / 调度 / 审查队列 | ✅ |
| **账本（含真实金额）** | ✅ 引擎自报的金额 + 按员工/按模型拆分 + 运行记录 |
| **②的窗口表** | ✅ 引擎自报 `contextWindow`（比内置表准）；❌ 还没用它去替换 `model_context` 的推导 |
| ⑤ 调度自动化（排队/并发上限/按能力选人） | ❌ |
| 成本并入**产出台账**（每个产出花了多少） | ❌ 运行记录已带 `label`，但还没和产出 id 挂上 |

### 10.17 第十六刀：按**任务**自选模型（② 的字面缺口）

重新对着**目标原话**逐项核对，发现最明确的未完成项其实很小、也很直白：

| 目标原话 | 状态 |
|---|---|
| ③ 花名册 + 能力组合 + 权限姿态 + 模型 | 四项都 ✅ |
| ④ 交付物 + 证据化验收 | ✅ |
| ⑤ 任务板 + 调度 + 账本 + 审查队列 | 四项都 ✅ |
| ② 按**员工**自选模型 | ✅（第 9 刀） |
| **② 按任务自选模型** | ❌ **只有员工级** |

这一刀补它，并顺手让上一刀的金额**挂到任务上**（"这条活花了多少钱"）。

#### 做出来的东西

| 件 | 说明 |
|---|---|
| 任务记录 | 新增 `provider_id` / `model` 两个字段（谁都能写：手写、@ceo、`task_create`） |
| 优先级 | `Task::route_override()` 把它变成**"显式指定"** → 在既有的 `resolve_route_for_agent` 里**压过员工花名册**（判断链路早就支持，这一刀只是把任务的声明接上去） |
| 结构化运行 | `engine_run` 接受 `task_id`：用任务上的模型（也用它上面的 owner），并把这次花费**记到该任务名下** |
| 账本 | 新增「**按任务**」一行：`write-draft $0.300000（2 次） ｜ fix-bug $0.050000（1 次）` |
| 面板 | 任务项显示任务级模型（含"压过员工花名册"的说明）；「结构化跑一次」带上 task id |

**为什么任务级要压过花名册**：一条任务要换模型，通常正是因为**这条活特殊**。如果花名册优先，那"按任务自选模型"就永远只是个摆设。

判断只保留一处：命令层不再自己读任务字段，改用 `Task::route_override()`。

#### 取证

```
===== 任务板（任务级模型）=====
共 1 条任务

[· todo] t1  X  @dev
      模型：glm / glm-4-plus（任务级覆盖，压过员工花名册）

===== 账本（按任务）=====
运行 3 次 ｜ 合计 $0.350000 ｜ 平均 $0.116667
按员工：@dev $0.350000（3 次）
按模型：m $0.350000（3 次）
按任务：write-draft $0.300000（2 次） ｜ fix-bug $0.050000（1 次）
```

**端到端证明**（跨模块测试）：任务写 `provider_id=glm`，员工花名册把 `dev` 绑到 `roster-prov` → 计划出来是 `route_source=requested`、`provider=glm`、`ANTHROPIC_API_KEY=key-glm`。**任务赢。**

#### 顺手清掉的两个小债

1. **多余的包装函数**：加了 `create_full` / `update_definition_full` 之后，原来的 `create` / `update_definition` 变成了纯转调（规则 7：多一个几乎同名的入口只会让人猜该用哪个）。删掉，测试迁到 `_full`。
2. **markdown 星号又漏进纯文本**：任务板那句"（**任务级覆盖**，压过花名册）"是要塞进对话框的纯文本，`**` 会字面显示 —— 与上一刀 `cost_note` 同一类问题。修掉并加断言：**对话框文本里不许出现 `**`**。

> 过程记录：这一刀的脚本化编辑把 `tasks.rs` 改乱过两次（一次把函数名误改成 `_UNUSED`，一次正则不平衡）。两次都是**停下来读文件、用精确编辑修**才恢复的 —— 脚本批量改代码在这种"多处相似结构"的文件上确实危险。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 ×2 / 串行 | **228 passed / 0 failed**（本轮 +6，三种模式一致） |
| `npm test` | **310 passed / 0 failed** |
| `npm run test:render` | **7/7 通过** |
| 任务级模型 | 解析 / 显示 / create / update（`None` = 该项不动）/ 空串归一 ✅ |
| **压过花名册** | 跨模块端到端：任务是 `requested`，凭据来自任务指定的 provider ✅ |
| 按任务计费 | 同任务两次累加、不同任务分开；总额仍按次 ✅ |
| 对话框文本 | 不含 markdown 星号（有断言）✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ② 现在的完成度

| 要素 | 状态 |
|---|---|
| 每槽 provider/model 落到 env | ✅ |
| 按**员工**自选 + 界面入口 | ✅ 花名册 + `/roster` |
| **按任务自选** | ✅ 任务字段 + 压过花名册 + 面板可见 |
| 本地模型档 | ✅ |
| failover | ✅ 静态可用性 + 撤退队列 + 历史；❌ 失败计数/熔断（`failover_params` 仍未消费） |
| 窗口表 | ✅ 引擎自报 `contextWindow`；❌ 还没用它替换 `model_context` 的型号推导 |

### 10.18 第十七刀：前端终于被跑起来了 —— 启动 + 真实流程，带反向对照

十六刀里我改过 `main.js` / `palette.js` / `index.html` / `styles.css` / `work-panel.js` / `ipc-bridge.js` / `agent-dispatch.js` / `tab-manager.js`，而**没有任何东西能证明 app 还能启动**。单元测试只覆盖纯函数；一个模块加载时抛错、一个 import 名字写错、一个 DOM id 不存在，测试照样全绿。

这一刀把这道网补上：`npm run test:boot`。

#### 怎么做的

用无头 Chrome 加载**真实的 `index.html` + `main.js`**，把 `window.__TAURI__` 换成桩（记录了它调了什么），然后在页面里**驱动真实流程**：

| 阶段 | 检查 |
|---|---|
| 启动 | 模块加载了吗 ｜ 调了哪些命令 ｜ DOM id 齐不齐 ｜ 有没有启动期错误 |
| 派发一个员工 | 桩返回**带全部诚实降级**的 spawn 结果 → 读 **xterm 缓冲**，断言那 6 条提示真的显示出来了 |
| 工作台 | 桩数据 → 断言审查队列 / 未验证 / 自报完成成组 / 任务级模型 / 无星号 |
| 派活 | 点面板上的按钮 → 断言 **PTY 真的收到了任务正文**、没有 `\r`、且要求写成 `review` |

为了让脚本能驱动流程，`main.js` 暴露了一个 `window.__onecode` 调试句柄（注释里写明它是调试/e2e 用的，不是业务 API）。

#### 取证（18/18）

```
MODULE_LOADED: yes
INVOKES: 26 [list_backends, cc_status, load_config, providers_list, is_first_run,
             list_projects, get_home_dir, session_restore, debug_log,
             pty_spawn, pty_set_active, session_persist, deliverables_scan,
             task_list, pty_write]
MISSING_DOM_IDS: none
BOOT_ERRORS: 0
PASS  SPAWN_OK                  终端里有内容（长度 453）
PASS  TERM_AGENT_NOT_INJECTED   身份没注入要说出来
PASS  TERM_PROVIDER_FALLBACK    回退要说出来
PASS  TERM_SWITCHED_FROM        换过路线要说出来
PASS  TERM_POSTURE_INFERRED     推断的档位要标明
PASS  TERM_SKILLS_MISSING       找不到的技能要说出来
PASS  WORK_SELF_CLAIMED         自报完成要单独成组
PASS  WORK_TASK_MODEL           任务级模型要显示
PASS  DISPATCH_WROTE            派活要写入任务 id（本次 1 次：["任务 write-draft：写一篇稿
                                面向技术读者 … 再把本任务状态改成 review 等我验收。"])
PASS  DISPATCH_NO_ENTER         不能代按回车
RESULT: ALL_FLOW_CHECKS_PASS
```

**这是第 1、3、4、9 刀做的那些"诚实提示"第一次被观察到真的渲染出来了** —— 在此之前它们只有代码与日志证据。

#### ★ 反向对照：证明这些检查**真的会失败**

一个从不失败的检查比没有检查更糟。所以逐个注入故障、确认被抓到：

| 注入的故障 | 结果 |
|---|---|
| import 一个不存在的导出 | `MODULE_LOADED: no` + `Uncaught SyntaxError: … does not provide an export named …` ✅ |
| **删掉**"agent 身份未注入"那条提示 | `FAIL TERM_AGENT_NOT_INJECTED` ✅ |
| 派活时偷偷改成 `\r`（会触发提交） | `FAIL DISPATCH_NO_ENTER` ✅ |

> 过程记录：我第一次的对照**注入得太弱** —— 把提示改成 `⚠ XX身份未注入`，而检查断言的是"包含 `身份未注入`"，子串还在，于是**照样通过**。改成真删掉那一行才暴露。这本身就是"弱断言 = 假安全"的又一次现身。

#### ★ 我自己的检查也错了两次（同类问题的自我示范）

1. **真空通过**：`WORK_NO_STARS`（断言 HTML 里没有 `**`）在**面板根本没渲染、内容为空**时也通过。修法：每一条依赖内容的断言都必须**先要求有内容**（`hasHtml && …`）。终端那组同理。
2. **假设错了**：`pty_write` 的 `data` 是**字节数组**（不是字符串），而我用 `String(w.data).includes(...)` 断言；而且 `writes[0]` 是**启动时的欢迎语**，不是派活那次。修法：`TextDecoder` 解码 + 指名点 `data-wp-id="write-draft"` 的按钮。

顺带确认了一件事：DOM 里第一个派活按钮属于**「agent 自报完成」那一组** —— 说明第 12 刀把该组排到最前的改动确实生效了（这算是它的一个副作用证据）。

#### 新增两条可重复命令

```
npm run test:render   # 工作台在真实 CSS 结构里的渲染（7 项）
npm run test:boot     # 启动 + 真实前端流程（18 项）
```

两条都**不放进 `npm test`**（需要 Chrome 与临时 HTTP 服务），需要时显式跑。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 ×2 / 串行 | **228 passed / 0 failed**（三种模式一致） |
| `npm test` | **310 passed / 0 failed** |
| `npm run test:render` | **7/7 通过** |
| **`npm run test:boot`** | **启动 0 错误 + 18/18 流程检查通过** |
| 反向对照（三类故障） | 全部被抓到 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

> **仍未做**：真 Tauri 窗口（`make dev`）。这次是**浏览器里跑真实前端代码**，不是 WebView 里的整机行为 —— 窗口/拖拽/原生菜单/托盘那层仍无证据。缺口继续留在文档里。

### 10.19 第十八刀：把 app 真的跑起来 —— 然后立刻发现一个真问题

十七刀里我一直把"真机跑一遍"往后推。这一刀做了，而且**第一次跑就抓到一个真问题**。

#### 怎么跑的（以及哪一步失败）

```
make build   →  cargo build（frontendDist: ../src → 前端在构建时嵌进二进制）
             →  14.7s 成功
启动二进制   →  进程存活、无 panic、无 ERROR
```

**这一步拿到了浏览器夹具拿不到的证据**：日志里出现

```
[js:renderList] orbs before clear: 0 of 0 tabs
[js:renderList] reattached: 0 of 0 saved orbs
```

说明**真实 WebView 里的 JS 启动成功并调进了 Rust** —— 这比 `test:boot`（stub 掉 `__TAURI__`）更强一层，因为这条 IPC 是真的。

**失败的一步：截不了图。** `screencapture` 报 `could not create image from display`（本沙箱没有屏幕录制权限）。所以**窗口里长什么样仍然没有证据** —— 我不假装它有了。

#### ★ 跑起来之后立刻发现的真问题

日志里躺着三条降级，而**界面上一点提示都没有**：

```
[providers] reconcile: sync isolated CC settings failed: write …/cc-config/settings.json failed: Operation not permitted
[session] init failed (P1, non-fatal): attempt to write a readonly database
```

第二条尤其要命：**会话库打不开 → 终端列表/会话恢复直接是空的**。用户看到的是"我的终端都不见了"，而原因只写在日志里。（顺带说明"管理员工"这件事的可靠性前提就是：**Slot 花名册读不出来时必须说**。）

这与整个项目反复出现的那条规矩同源（第 3、4、9、12 刀）：**降级必须如实显示，不能只躺在日志里**。

#### 修法：启动期降级进界面

| 层 | 内容 |
|---|---|
| `diagnostics` 模块 | 收集**启动期**、影响功能的降级：`{kind, message, impact}` —— `impact` 是**用户视角的后果**（"终端列表与会话恢复不可用"），只说技术原因没用 |
| 接线 | 四处：会话库打不开 / app_data_dir 不可用 / providers reconcile 失败 / **隔离 settings 写失败**（这条单独被 `log::warn` 吞掉过） |
| 去重 | 同一 `kind` 只留第一条（重复不增加信息量，只会把状态栏撑长） |
| 界面 | 状态栏 `#healthWarn` 显示后果文本，**悬停看技术原因**；启动降级优先于进程健康告警（前者影响"功能有没有"，后者只是"跑得好不好"） |
| 合并渲染 | 两个来源合并到一个渲染函数 —— 否则后台每 5s 的健康轮询会把启动降级**冲掉** |

#### 真机验证（重建后重跑）

```
[startup] cc-config-settings：write …/cc-config/settings.json failed: Operation not permitted (os error 1)
          （影响：隔离配置没写进去 —— 切换供应商可能不生效）
[startup] session-store：attempt to write a readonly database
          （影响：终端列表与会话恢复不可用 —— 重启后可能看不到原有终端）
```

两条降级在**真实二进制**里被记录并带上了后果；同时前端仍然正常调进来（`[js:renderList]`）、无 panic。

#### 验证

| 检查 | 结果 |
|---|---|
| **真机构建** | `make build` 成功（19.7s） |
| **项目级引擎定义** | 只能新增不能覆盖；真机 8 个引擎；点名不存在会如实报出 ✅ |
| **ACP 类** | 按 v1 规范实现；对 mock 做协议一致性；真机端到端 18/18 ✅ |
| **有界排空唯一实现** | `crate::proc`：两处已迁移，ACP 复用同一份 ✅ |
| **默认不提权** | 空/拼错的权限字段都不提权；继承档不加任何参数（5 条不变量测试）✅ |
| **熔断** | 三态+结束方式判定；**真机实测触发并切档**（$0）；失败运行也记账 ✅ |
| **多轮上下文** | 实测否定引擎接续 → 改为本机回放（实测有效，引擎无关）✅ |
| **结构化常驻 Slot** | 消息列表+多轮+每轮结算；30/30 流程检查 ✅ |
| **真机自检** | `ONECODE_SELFCHECK=1`：真实 WebView 对真实命令 **19/19**，含任务板读写往返与 ACP 端到端 ✅ |
| **真机启动** | 进程存活、**0 panic / 0 ERROR**、JS→Rust IPC 通（`[js:renderList]`） |
| **真机诊断** | 两条降级带上用户后果被记录 ✅ |
| `cargo test` 并行 ×2 / 串行 | **231 passed / 0 failed**（本轮 +3） |
| `npm test` | **310 passed / 0 failed** |
| `npm run test:render` | **7/7** |
| `npm run test:boot` | **0 启动错误 + 20/20 流程**（新增 2 条：降级要显示 + 悬停要有技术原因） |
| 反向对照 | 不显示启动降级 → `FAIL STARTUP_ISSUE_SHOWN` ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |
| 遗留进程 | 无 ✅ |

顺带修掉一个**测试基础设施**问题：两条 Chrome 检查原先用固定端口，连续跑时会连到**上一轮遗留的服务**上 —— 那时"通过"其实是测的上上轮内容。改成自动挑空闲端口。

> **仍未做**：窗口内容的视觉证据（本沙箱无屏幕录制权限）。真机跑通了、IPC 通了、降级通路验了，但"它看起来对不对"仍然只有浏览器级证据。

### 10.20 第十九刀：真机自检 —— 第一次跑就抓到一个"死表面"

第 17 刀的 `test:boot` 用的是**桩**（`window.__TAURI__` 被换掉），所以它证明的是"前端逻辑对"，证明不了"真命令真的回答了、形状真的对"。更要紧的是：④⑤ 那一批命令 —— `deliverables_scan` / `task_list` / `ledger_scan` / `roster_list` / `startup_issues` —— **真前端从来没有调用过**（它们只在命令面板或对话框里按需触发，没人点就永远不走）。

这一刀把这道缝补上：`ONECODE_SELFCHECK=1` 时，前端在**真实 WebView 里**对**真实 Rust 命令**逐条校验**返回形状**（不只是"没抛错"），结果经 `debug_log` 落到进程日志。

开关由后端命令 `selfcheck_enabled()` 唯一决定 —— 前端不猜；关掉时连动态 `import` 都不做，零开销。

#### ★ 第一次真跑就抓到：注册了却没人能调的命令

```
SELFCHECK FAIL 9/11
FAIL  engines_problems     抛错：ipc.enginesProblems is not a function
FAIL  engines_config_path  抛错：ipc.enginesConfigPath is not a function
```

两个 `#[tauri::command]` 在 Rust 里注册着、**却没有任何前端包装** —— 从界面上永远够不着。它们的职责已被 `engines_show` / `engines_example_show` 覆盖，于是**删掉命令本身**（死表面不该留）。

> 过程记录：删的时候连 `engines_example()` 一起删了，而它**同时**是命令和辅助函数（`engines_example_show` 在用）—— 编译器立刻报 `cannot find function`，恢复成普通函数即可。**命令注册是多余的，函数不是。**

#### 修完之后的真机结果

```
SELFCHECK PASS 9/9
PASS  startup_issues       2 条
PASS  load_config          backend=claude-code
PASS  providers_list       providers=8 usable=8
PASS  list_backends        7 个引擎
PASS  roster_list          0 个员工绑定
PASS  deliverables_scan    产出=0 git=false
PASS  task_list            任务=0
PASS  ledger_scan          合计金额=0 运行=0
PASS  cc_sessions_list     119 条会话
（panic/ERROR 计数：0）
```

`providers=8 usable=8` 与 `119 条会话` 都是**你的真实数据**。这是 ④⑤ 那批命令**第一次**在真机里、由真前端、对真后端跑通。

#### ★ 顺手发现并修掉一个"测试基础设施"缺陷：假失败

`npm run test:boot` 我原以为稳定，用**退出码**核对时发现它 **1/3 概率失败**，而失败信息是"没找到 bootlog" —— 那是**观察失败**（`--dump-dom` 偶尔在夹具把内容写进 DOM 之前就落盘），不是产品失败。这类假失败最坏的结果是**让人不再相信这个检查**。

修法：把**观察**与**判定**分开：

- 没观察到 → 重试（最多 4 次）；
- **一旦观察到内容 → 严格判定、立即退出，绝不允许重试掩盖真实 FAIL**。

连跑 4 次全部 exit=0；再注入真故障（删掉"身份未注入"提示）→ **exit=1** 且打出 `FAIL TERM_AGENT_NOT_INJECTED`。两条路都验过。

#### 验证

| 检查 | 结果 |
|---|---|
| **真机自检** | `SELFCHECK PASS 9/9`，真实数据，0 panic ✅ |
| 自检抓到的缺陷 | 2 个不可达命令 → 已删；1 个误删的函数 → 已恢复 ✅ |
| `cargo test` 并行 ×2 / 串行 | **231 passed / 0 failed**（三种模式一致） |
| `npm test` | **310 passed / 0 failed** |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **连跑 4 次 exit=0**；注入真故障 → exit=1 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |
| 残留进程 | 无 ✅ |

> 仍未做：窗口内容的**视觉**证据（本沙箱无屏幕录制权限）。真机跑通、IPC 通、命令形状对、降级通路验了 —— 但"它看起来对不对"仍然只有浏览器级证据。

### 10.21 第二十刀：结构化常驻 Slot —— 补齐 ① 最后一块

① 一直说"可换引擎"，但实现上**要求引擎长得像个终端**。这一刀做出第二种槽位：**不走 TUI，走结构化协议**。

#### 先免费探测连续性机制

多轮接续得有机制。用"不可达 base_url + 假 key"探测（init 在推理之前 → **不花 token**）：

| flag | 结果 |
|---|---|
| `--session-id <uuid>` | ✅ 被接受 |
| `--continue` | ✅ 被接受 |
| `--resume <id>` | ❌ 被拒 |

所以"指定会话"这条路走 `--session-id`。

> ⚠️ 证据分级：**"同 id 会接续上下文"尚未实测确认** —— 确认它需要一次真实推理（本机一次短提示输入就有 ~34.8k token）。这里只保证把同一个 id **稳定传下去**，并把引擎回的 `session_id` 如实记下来；两者不一致时界面**显眼告警**（`session_mismatch`）。首次真实使用时可以对着记录核对。

#### 做出来的东西

| 件 | 说明 |
|---|---|
| `structured_slot` | 槽位 = 头 + 若干轮，落 `<cwd>/.onecode/structured/<id>.jsonl`（**append-only**，坏行计数） |
| 每轮记什么 | prompt / reply / 成功与否 / **金额 / 输入 / 输出 / 耗时 / 引擎回的会话 id** |
| 命令 | `structured_open` / `structured_send` / `structured_state` / `structured_list` |
| 复用 | 走**同一套** `spawn_plan`（身份/姿态/技能/路由）与 `engine_run::run`（协议+结算）—— 结构化槽位不是另起一套引擎驱动 |
| 记账 | 每轮同时进账本（`label = slot:<id>`），所以账本能看到"哪个槽位花了多少" |
| 视图 | `StructuredSlotView` 把**合计金额/用量/会话不一致**算好交给前端；前端只排版 |
| 前端 | `structured-view.js`：消息列表 + 输入框 + 每轮金额；`tab-manager.createStructuredTab` 把它做成一种 tab |
| 入口 | ⌘K →「结构化槽位」 |

#### ★ 启动检查立刻抓到一个真 bug：加一种 tab 类型必须护住 PTY 专属路径

第一次建结构化槽位就抛：

```
TypeError: Cannot read properties of null (reading 'buffer')
```

根因：`switchTo()` 里无条件读 `st.term.buffer.active`（保存滚动位置）、再 `fit()`/`focus()` —— 这些**全是 PTY 专属**，而结构化槽位没有 xterm。加护栏（`kind === 'structured' || !st.term` 直接返回）后通过。

> 顺带说明为什么"先做前端再验证"是危险的：这个 bug 在 Rust 测试、JS 单元测试里都照不出来，**只有真的把 tab 建出来才会炸**。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **240 passed / 0 failed**（本轮 +9，exit=0） |
| `npm test` | **321 passed / 0 failed**（本轮 +11） |
| `npm run test:render` | **7/7**，exit=0 |
| **`npm run test:boot`** | **30/30**（新增 10 条结构化槽位流程），exit=0 |
| 结构化流程实测 | 建槽位（非终端）/ 显示身份与路线 / 回复 / **每轮金额** / 失败原因 / 合计来自后端 ✅ |
| 坏记录 | 坏行计数；追加到不存在的槽位报错而非静默创建；非法 id 全拒 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |
| 残留进程 | 无 ✅ |

#### 目标 ① 现在的完成度

| 要素 | 状态 |
|---|---|
| 引擎抽象 + 可扩展定义 + 能力协商 | ✅ |
| 结构化**一次**（`engine_run`） | ✅ |
| **结构化常驻 Slot** | ✅ 消息列表 + 多轮 + 每轮结算 + 落盘 |
| ACP 客户端 | ❌ 本机无对端、无桥 → 不写（第 11 刀已取证） |
| `--session-id` 的**接续语义** | ⚠ 传参已实测接受；接续效果**未实测**（需要真实推理） |
| 项目级 `engines.json` | ❌ |

### 10.22 第二十一刀：实测否定了上一刀的核心前提 —— 然后修好它

上一刀我给自己留了一句"⚠️ `--session-id` 的**接续语义尚未实测**"。这一刀去实测，结果是：**不成立。**

#### 实测（本机真实调用，三笔账都列出来）

为了压成本，cwd 用一个**空目录**（没有 `.claude/` → 上下文从 34.8k 降到 ~10k）：

| 调用 | 结果 | 金额 |
|---|---|---|
| ① `--session-id X`「请记住 42」 | reply=`ok`，input **10038** | **$0.062807** |
| ② **同一个 X**「我刚才让你记的是什么？」 | reply=「**我没有你先前告诉我的数字记录…这段对话里没有相关上下文**」，input **367** | **$0.021552** |
| ③ `--resume X` | `subtype=error_during_execution`，`duration_ms=0`，**无错误文本** | **$0** |

**第二轮输入从 10038 掉到 367** —— 如果接续了，输入只会更大。结论：`--session-id` **只给会话打标签，不接续上下文**；`--resume` 则直接无效（好在是**零成本失败**）。

> 本轮共花 **$0.135069**（三笔）。

**顺带一个实操发现**：第一次尝试（**不加** `--disallowedTools`）**卡住 >90 秒**——只发出 init + 2 条 assistant 就没有下文。推测是模型想用工具，而 `-p` 非交互模式下在等批准。所以后续测试都显式禁掉工具。这条对"结构化槽位"是真问题：**服务端一旦想用工具就可能挂住**，超时必须有（已有 300s）。

#### 修法：对话由**我们**拥有，不靠引擎

不接续就不接续 —— 我们自己回放：

```rust
pub fn compose_prompt(turns: &[Turn], prompt: &str, budget_chars: usize) -> String
```

- 把之前的轮次拼进这一轮的提示词；**超预算就丢最早的几轮，并写明丢了几轮**（静默截断会让模型以为"我们从头聊到现在"）；
- 失败的轮次**也要带上**并标成失败（否则模型会以为"上一句我已经答过了"）。

**为什么这不是权宜之计**：回放**对任何引擎都成立**。反过来，如果依赖引擎侧 resume，那么"换引擎后对话就断了" —— 那恰恰违背 ① 的"可换引擎"。所以这一步是**设计上的正确选择**，不是补丁。

#### 修法实测有效

同样的两轮，第二轮把历史回放进提示词：模型**答出 `42`**（$0.050710，输入 9932）。

#### 前端措辞也跟着改诚实

既然实测证明它不接续，就不能把它显示成"会话"。结构化槽位头部现在是：

```
标签 aaaaaaaa → 引擎回 aaaaaaaa ｜ 2 轮 ｜ 合计 $0.174335 ｜ …
多轮上下文由本机回放（引擎的 session-id 只作标签，不接续）
```

**把不接续的东西显示成"会话"就是误导** —— 与整个项目"降级必须如实显示"是同一条规矩。

#### 一个值得命名的规律

二十刀里，凡是**我自己标注"未验证"**的东西，实测之后被否定的已经有两例：

| 标的 | 实测结果 |
|---|---|
| `--permission-mode planMode`（第 3 刀写进代码） | **非法值**，会让终端秒退（第 11 刀） |
| `--session-id` 接续上下文（第 19 刀写进设计） | **不接续**，多轮前提不成立（这一刀） |

两例都是"从 bundle/文档里看到的形状"被当成了"实际行为"。**"标了未验证"不是免责，是待办。**

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **246 passed / 0 failed**（本轮 +6，exit=0） |
| `npm test` | **322 passed / 0 failed**（+1） |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| 接续实测 | 否定（10038 → 367）✅ |
| 回放修法 | 实测有效（答出 42，$0.050710）✅ |
| 回放实现 | 空历史原样发 / 带历史 / 不丢不写"省略" / 丢最早的并写明 / 失败轮标失败 ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

#### 目标 ① 现在的完成度

| 要素 | 状态 |
|---|---|
| 引擎抽象 + 可扩展定义 + 能力协商 | ✅ |
| 结构化一次 / 结构化常驻 Slot | ✅ |
| **多轮上下文** | ✅ 由本机回放（实测有效，且引擎无关） |
| ACP 客户端 | ❌ 本机无对端无桥 → 不写（第 11 刀取证） |
| 工具调用挂住的风险 | ⚠ 实测"想用工具时会挂住"，已有 300s 超时；❌ 尚无"检测到挂住就提示" |
| 项目级 `engines.json` | ❌ |

### 10.23 第二十二刀：熔断 —— 补上 ② 最后一项，判定刻意保守

`failover_params`（连败阈值/超时）从第 9 刀就写在配置里，**一直没有被消费**：failover 只在**静态不可用**（没供应商 / 没凭据）时发生，跑起来之后某个供应商开始连败是没人管的。这一刀补上。

#### 最难的地方不是"换"，是"**什么时候才算该换**"

直觉做法是"槽位非零退出 = 供应商坏了"。**这是错的**：用户自己 `exit`、按 Ctrl-C 都会非零退出，照这个判据会**莫名其妙换供应商**。第 20 刀还实测过"模型想用工具时会挂住 >90 秒"——那是工具问题，也不是供应商问题。

所以判定按三态、且**刻意保守**：

| 三态 | 条件 | 对计数的影响 |
|---|---|---|
| `Success` | 引擎正常返回 | **归零** |
| `ProviderFailure` | 引擎报执行错误，且**一点推理都没做**（token 全零、费用为 0） | +1 |
| `Inconclusive` | 其余全部 —— **没有结算 / 超时 / 推理做过才报错** | **既不加也不清零** |

`ProviderFailure` 只要"报错且没做任何推理"，与"凭据坏了/连不上"的特征一致。
**误换比不换贵** —— 宁可漏判。

#### 做出来的东西

| 件 | 说明 |
|---|---|
| `classify_run` | 上面的三态判定（纯函数） |
| `Breaker` | 连败计数器，**进程内不落盘** —— 落盘会让昨天的失败影响今天；成功归零、`Inconclusive` 不动 |
| `next_usable_provider` | 先按撤退队列、再按目录顺序，跳过自己与不可用的（`provider_usable` 为准，判断只有一处） |
| `note_run_outcome` | 记结果；真的到阈值就**复用 `perform_switch`** 切档 |
| 接线 | `engine_run` 与 `structured_send` 走**同一套**判定 |
| 切换原因 | `perform_switch` 加了 `reason` 参数（`manual` / `consecutive_failure`）—— 历史里必须分得清是谁换的 |

复用 `perform_switch` 而不是另写一套：切档要写 AppConfig + 同步隔离 settings + 队列自愈 + 发事件，漏一处就是"看起来切了其实没生效"（规则 7）。

#### ★ 前端：自动切换必须**看起来就是自动的**

原来的 toast 对任何切换都说"已切换到 X" —— 于是**熔断触发的自动切换与用户自己点的一模一样**，而它会**重启你的终端**。这是误导。

抽成纯函数 `switch-toast.js`（便于单测，也避免把 `ipc-bridge` 的 DOM 依赖带进测试）：

| reason | 文案 | 样式 |
|---|---|---|
| `manual` | 已切换到 GLM | `ok` |
| `consecutive_failure` | **Kimi 连续失败，已自动切换到 GLM（终端已重启）** | `warn` |

#### ★ 顺手又抓到一个真 bug（桩测不出来的那种）

改命令返回类型时发现：**`structured_send` 的返回类型漏了视图**（还是 `StructuredSlot`，不含后端算好的合计）—— 真前端会显示"**合计 $0.000000**"。

为什么之前没发现：启动检查用的是**桩**，桩里我手写的假数据**带着** `total_cost_usd`。**桩会替你把漏掉的字段补上。**

两处修：

1. 返回类型改为 `StructuredSlotView`；
2. 把"交给前端的 JSON 形状"钉在**单元层**：`serde_json::to_value(view)` 必须含 `total_cost_usd` / `total_input_tokens` / `session_mismatch` / `turns`。

同时给真机自检加了 `structured_open` / `structured_list` 的形状探测，并引入**第三种状态**：

```
SELFCHECK PASS 6/6（跳过 5）
SKIP structured_open      跳过：没有活动项目（写类命令会污染目录）
```

**跳过 ≠ 通过**：写类命令在没有活动项目时不能写到 `/` 去，也不能把环境问题报成产品问题。所以显式标 SKIP 并说明原因，且**不计入通过率**。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **258 passed / 0 failed**（本轮 +12，exit=0） |
| `npm test` | **329 passed / 0 failed**（+7） |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| **真机自检** | `PASS 6/6（跳过 5）`，0 panic ✅ |
| 熔断判定 | 成功归零 / 无推理的执行错误=故障 / **超时与"推理后报错"=说不清** / 说不清不洗计数 ✅ |
| 候选选择 | 按队列、跳过自己与不可用、队列用尽回退目录顺序、唯一可用时不换 ✅ |
| 前端文案 | 自动切换必须含"自动"+"连续失败"+"重启"，且用 warn 样式 ✅ |
| 真实 `providers.json` | sha256 / mtime 与测试前一致 ✅ |
| 残留进程 | 无 ✅ |

> ⚠️ 仍未做：**熔断从未在真实连败下触发过**（需要把某个供应商真的弄坏）。判定与计数器都有单测，但"真实连败 → 真的切过去"这一整条只有代码路径证据。

#### 目标 ② 的完成度

| 要素 | 状态 |
|---|---|
| 按员工 / 按任务自选（含本地档） | ✅ |
| 静态可用性 failover + 撤退队列 + 历史 | ✅ |
| **熔断（连败阈值）** | ✅ 判定 + 计数 + 复用切换链路 + 自动切换可见 |
| 熔断的真实触发 | ⚠ **未在真实连败下验证过** |
| 窗口表替换型号推导 | ❌ 引擎已自报 `contextWindow`，但推导路径仍在用内置表 |

### 10.24 第二十三刀：把真机自检从"只读探测"扩成"真实读写往返"

上一刀的自检结果是 `PASS 6/6（跳过 5）`—— 5 条跳过的原因都是"没有活动项目"。而**跳过的那几条恰好是 ④⑤ 的核心命令**（任务板 / 产出 / 账本 / 结构化槽位）。自检的价值因此打了折。

这一刀把折扣补上：让后端给一个**可写的临时项目目录**（`ONECODE_SELFCHECK=1` 时才可用，且**每次重建**），读写类命令全部跑在它里面 —— 碰不到用户任何真实数据（供应商/花名册是全局的，自检绝不碰）。

#### 结果：`PASS 15/15`（0 跳过）

```
（读写类命令跑在临时目录：/var/folders/…/T/onecode-selfcheck）
SELFCHECK PASS 15/15
PASS  startup_issues        2 条
PASS  load_config           backend=claude-code
PASS  providers_list        providers=8 usable=8
PASS  list_backends         7 个引擎
PASS  roster_list           0 个员工绑定
PASS  deliverables_scan     产出=0 git=false
PASS  task_list             任务=0
PASS  task_roundtrip        建出 1791948212410
PASS  task_model_persisted  任务级模型 glm/glm-4-plus
PASS  task_starts_as_todo   初始状态 todo
PASS  task_human_ruling_wins 状态 review（来源 human）
PASS  ledger_scan           合计金额=0 运行=0
PASS  structured_open       id=c4983ebf turns=0
PASS  structured_list       1 个槽位
PASS  cc_sessions_list      119 条会话
```

**这是 ④⑤ 的核心命令第一次在真机上、由真前端、对真后端跑通**（此前只有桩级或单元级证据）。其中三条是真正的**行为**断言，不只是形状：

| 断言 | 证明了什么 |
|---|---|
| `task_roundtrip` | 建出来的任务能在列表里按 id 找到 |
| `task_model_persisted` | **任务级模型**（目标 ②）真的写进磁盘又读回来 |
| `task_human_ruling_wins` | **人的裁决压过声明**（`status_source === 'human'`）—— ④⑤ 的核心不变量 |
| `structured_open` | 上一刀修的 `total_cost_usd` 等视图字段**在真机上也确实在**（上一刀只有单元测试） |

#### 顺带拿到了"两份文件"的实物证据

任务设计是"**声明 + 人的裁决**"两份文件，此前只有代码与单测说明。自检产物直接摆出来：

```
T/onecode-selfcheck/.onecode/tasks/1791948359704.json          ← 声明
T/onecode-selfcheck/.onecode/tasks/1791948359704.human.json    ← 人的裁决
T/onecode-selfcheck/.onecode/structured/<uuid>.jsonl           ← 结构化槽位
```

**可重复**：连跑两次都是 `PASS 15/15`，且第二次开头 `task_list 任务=0`（目录每次重建，上次的不污染这次）。

#### 一点自我更正

这一刀**没有**发现新的产品 bug（前四刀各发现一个）。它的价值是**覆盖**与**确证**：把"跳过"变成"真的跑过"，并确证上一刀的修复在真机成立。我不把它说成"又抓到一个 bug"。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **258 passed / 0 failed**，exit=0 |
| `npm test` | **329 passed / 0 failed**，exit=0 |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| **真机自检** | **`PASS 15/15`（0 跳过）**，连跑两次一致；0 panic ✅ |
| 自检隔离 | 跑在临时目录；真实 `providers.json` sha256 / mtime 未变 ✅ |
| 残留进程 | 无 ✅ |

### 10.25 第二十四刀：默认权限姿态审计 —— 结论是"安全的"，所以把它钉住

调度台会拿 AI 员工去**跑 shell**。所以"默认给多大权限"是这个产品最要紧的一个默认值。这一刀去审它。

#### 审计结果：**默认不提权**

| 情形 | 结果 |
|---|---|
| 员工没写 `permission:` | 解析成 `Inherit` → **不追加任何参数** → 引擎自己的默认（Claude Code = `default`，会先问） |
| 员工写了空串 / 全空格 | 同上（`parse("") == Inherit`） |
| 员工写了不认识的值（`bypas` / `yes` / 乱写） | `None` → 由 `spawn_plan` **报出来**并退回继承，**不猜成提权** |
| 只有显式写 `bypass` / `yolo` / `full` | 才走 `bypassPermissions` |

**代码里没有任何一条路径默认提权**（`grep BypassPermissions` 只有枚举定义、解析分支与这条测试）。

> 对比：仓库里另一个产品 `eng/onecode`（Web 版，已休眠）在脚本层默认 `bypassPermissions` —— 那正是第 1 刀列进 Phase 0 安全链条的问题。**桌面版不是那个默认值**，这一点值得写下来，免得被混为一谈。

#### 把结论钉成测试（默认值最容易悄悄变）

审计的结论如果不钉住，下次改代码就会漂。所以加了 5 条不变量测试：

| 测试 | 钉的是什么 |
|---|---|
| `an_empty_permission_field_means_inherit_never_bypass` | 空字段 → 继承，**绝不是** bypass |
| `an_unrecognised_permission_value_is_not_silently_treated_as_bypass` | 拼错 → `None`，不"猜"成提权 |
| `inherit_appends_no_permission_arguments_at_all` | 继承档 **一个参数都不加**，且参数里不许出现 `dangerously`/`bypass`/`yolo` |
| `only_an_explicit_bypass_escalates` | 只有显式写才提权 |
| （同一条里）`plan` 用合法值 | 历史坑：`planMode` 会让 CLI 秒退，必须用 `plan` |

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **262 passed / 0 failed**（本轮 +4，exit=0） |
| `npm test` | **329 passed / 0 failed**，exit=0 |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 与测试前一致 ✅ |

### 10.26 第二十五刀：ACP 客户端 + 有界排空的债 —— "ACP 类"是目标点名的

目标原话里点名了 **"ACP 类"**，而我此前因为本机没有对端就一直没写。这一刀写了，做法是：
**按官方规范实现** → 用**照规范写的 mock agent** 做协议一致性测试 → 再用**临时 HOME** 让**真机**跑完整条链路。

规范依据（照抄，不靠记忆）：[overview](https://agentclientprotocol.com/protocol/v1/overview)、[initialization](https://agentclientprotocol.com/protocol/v1/initialization)、[session-setup](https://agentclientprotocol.com/protocol/v1/session-setup)、[prompt-turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)、[tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls)。

#### 两个刻意的"不做"

1. **不宣告 fs / terminal 能力**。规范：未宣告即不支持（"MUST treat all capabilities omitted as UNSUPPORTED"）→ Agent 不会通过我们去读写文件、起终端。**默认不给**（与第 24 刀的姿态审计同一条纪律）。mock 侧实测确认：`client_capabilities == {}`。
2. **权限请求默认拒绝**：优先挑 `reject_once`/`reject_always` 选项；没有可拒绝的就回 `cancelled`。**绝不默认放行。**

#### 先还一笔我自己记过的债

写 ACP 需要进程交互，而"spawn + 读取线程 + **有界排空**"已经有两份实现，且**出过两次同样的错**（第 5 刀 600ms 超时跑了 31.7s；第 14 刀又跑了 30s）。再写第三份就是第三次复制。

所以先把**易错的那一段**抽成唯一实现 `crate::proc::drain_bounded`（+ `drain_note`），并**迁移已有的两处**（`engine_run` / `deliverables`）。迁移后原测试（含"超时必须约束墙钟"那条）全绿。

> 仍留的重复：spawn 与读取线程的**样板**在三处仍有 —— 但那部分没出过错，且各自数据形态不同（按行 / 按字节）。这条写清楚，不假装债全还了。

#### 做出来的东西

| 件 | 说明 |
|---|---|
| `crate::acp` | initialize → session/new → session/prompt → 收 `session/update` → 答 `session/request_permission` / 无法服务的请求回 `-32601` → 等 `stopReason` |
| 事件解析 | `agent_message_chunk` / `agent_thought_chunk`（**与回复分开**）/ `tool_call` + `tool_call_update`（**补丁语义**）/ `usage_update` |
| 版本协商 | 版本不一致时**报出来**并断开（规范：客户端 SHOULD close and inform the user） |
| `Turn.extra_meta` | 协议特有的事实单独一句话（stopReason / 上下文占用 / 权限请求次数），**不硬塞进 input/output token**（ACP 报的是会话级占用，塞进去会污染账本） |
| 协议分流 | `BackendProfile.protocol`（`print`/`acp`）+ `SpawnPlan::is_acp()`（判断只有一处）→ **结构化槽位**按协议分流；`engines.json` 里写 `"protocol": "acp"` 即可 |
| 优雅退出 | 收到最后一轮后先关 stdin 等它自己退（1s），再杀树 —— 早先是**立刻杀**，导致一次真实竞态（agent 还没落盘就被砍） |

#### ★ 真机端到端（临时 HOME + ACP 引擎指向 mock）

用 `HOME=/tmp/onecode-acp-home` 启动真机 app（`engines_path()` 读 `$HOME`），`engines.json` 里注册一个 `protocol: acp` 的 mock 引擎：

```
SELFCHECK PASS 18/18
PASS acp_turn          回复="你好，我是 mock agent。"
PASS acp_stop_reason   ACP（mock-agent） · stop=end_turn · 上下文 53000/200000 · 有思考片段（未计入回复）
PASS acp_protocol_tag  协议要标出来，别与 print 混为一谈
```
```
[structured] ACP：stop=end_turn 工具调用=1 权限请求=0 花费=$0.045000
mock 侧观察到：client_capabilities={} protocol_version_seen=1 got_session_new_cwd=true
```

整条链路在**真机**上跑通：`desktop.json` → `engines.json` → profile → `spawn_plan` → 协议分流 → ACP 客户端 → 结构化槽位 → IPC → 前端渲染。

#### ★ 顺手抓到并修掉的两个真 bug

| bug | 症状 | 修法 |
|---|---|---|
| **未等 `initialize` 响应** | 主循环立刻发 `session/new`，initialize 的响应因 id 不匹配被当"迟到答复"丢掉 → 版本/能力全空 | `pending = Some(init_id)` 先等它。规范：会话**必须**在初始化完成之后建立 |
| **收到最后一轮就立刻杀进程** | agent 还没落盘/关连接就被砍（mock 的 `observed.json` 因此空着）—— 真实竞态 | 先关 stdin 等 1 秒优雅退出，再杀树 |

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **281 passed / 0 failed**（本轮 +19，exit=0） |
| `npm test` | **331 passed / 0 failed**（+2） |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| **真机 ACP 端到端** | `PASS 18/18`（含 3 条 ACP 断言），0 panic ✅ |
| 协议一致性（对 mock） | 完整一轮 / 默认拒绝挑 reject 选项 / 显式放行挑 allow 选项 / 不支持请求回 -32601 / tool_call 补丁语义 / 用量照抄 / 版本不一致要报 / 起不来要报 / 赖着不退墙钟有界 ✅ |
| 有界排空唯一实现 | 两个调用点已迁移，原测试全绿 ✅ |
| 真实 HOME 的 `providers.json` | sha256 / mtime 未变（自检跑在临时 HOME）✅ |
| 残留进程 | 无 ✅ |

> **仍未做 / 不确定**：与**真实** ACP agent 的互通（本机没有：`claude-agent-acp` / `codex-acp` 都不存在，第 11 刀已取证）。这里证明的是"我们符合 v1 规范"，**不是**"与某个真实实现互通"。ACP 也还没有常驻槽位（目前是结构化槽位的一轮一次）。
>
> 附带确认：临时 HOME 下 `startup_issues` 是 **0 条** —— 说明第 17 刀看到的两条降级确实是**沙箱造成**的，不是产品缺陷。

### 10.27 第二十六刀：熔断的真实触发 —— 而它一开始**抓不到**连通性故障

上一刀把熔断写完，但留了一句"⚠️ 从未在真实连败下触发过"。这一刀去真机触发它 —— 结果**第一次没触发**，而且原因很有价值。

#### 为什么能免费验证

用临时 HOME 起真机 app，配两个**不可达**的供应商（端口 9 / 10 关闭）+ 阈值设成 **1**：
失败是确定的，且**一分钱不花**。再用一个显式开关 `ONECODE_SELFCHECK_NETWORK=1`
让自检跑一轮真实的结构化对话（默认**跳过** —— 自检绝不能在用户不知情时替他花钱）。

#### 第一次：熔断没动，自检指出了原因

```
PASS print_turn_honest      失败但如实记了原因："引擎结束了但没有 result 事件（可能不支持 stream-json）"
FAIL print_turn_in_ledger   运行记录 0 条，金额 $0
[failover] （无）
active: prov-a   history: []
```

两个**真缺口**，都是"只跑真机才能发现"的那一类：

| # | 缺口 | 为什么危险 |
|---|---|---|
| 1 | **供应商连不上时引擎压根不产生 `result` 事件**，而 `classify_run` 把"没有结算"一律当 `Inconclusive` | 熔断**永远抓不到连通性故障** —— 而那正是它最该抓的 |
| 2 | 没有 `result` 的失败运行**根本没被记账** | 账本上看起来像"什么都没发生"，而实际是"跑了一次没成" |

#### 修法 1：没有结算时，**按结束方式**再分

新增 `ExitKind`（`Completed` / `ExitedWithoutResult` / `TimedOut` / `SpawnFailed`），判定随之细化：

| 结束方式 | 判定 | 理由 |
|---|---|---|
| 拿到 result | 按原三态 | — |
| **自己在没有 result 的情况下退出** | **ProviderFailure** | 起来了、一轮都没成 → 连接/凭据问题的特征 |
| 超时 | `Inconclusive` | 可能是"模型想用工具挂住了"（第 20 刀实测过）→ 换供应商没用 |
| 连进程都没起来 | `Inconclusive` | 二进制不在，换供应商同样救不了 |

**"说不清"与"故障"的界线，从此由结束方式决定，而不是靠猜。**

#### 修法 2：**每一次运行都记账**

`RunEntry` 增加 `error`；两处（`engine_run` / `structured_send`）都改成**每次都写**（没有结算时金额为 0、`ok=false`、带上原因）。账本新增 `runs_failed` 与一行说明：

```
其中失败 2 次（失败通常不花钱，但要说出来）
```

理由与"失败的花费同样是花费"一脉相承：**只看金额会把"跑三次都失败"看成"什么都没发生"**。

#### 第二次：熔断真的触发了

```
SELFCHECK PASS 19/19（跳过 1）
PASS print_turn_in_ledger   运行记录 1 条，金额 $0

[failover] prov-a 连败 1/1
[failover] 连败 1 次 → 已切到 prov-b
```
```json
{"active_provider_id": "prov-b",
 "history": [{"from": "prov-a", "to": "prov-b", "reason": "consecutive_failure"}]}
```

**② 的最后一个"未测"关掉了**：真实连败 → 计数 → 达到阈值 → 复用切换链路真的切过去 → 历史里写明是 `consecutive_failure`。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **285 passed / 0 failed**（本轮 +4，exit=0） |
| `npm test` | **331 passed / 0 failed**，exit=0 |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| **熔断真机触发** | 连败 1/1 → 切到 prov-b → 历史记 `consecutive_failure` ✅（**花费 $0**） |
| 失败运行记账 | 每次运行都记；失败计数可见；金额只按真实的算 ✅ |
| 结束方式判定 | 无 result 自己退出=故障 / 超时=说不清 / 起不来=说不清 ✅ |
| 花钱探测默认关闭 | 不设 `ONECODE_SELFCHECK_NETWORK` 时 `SKIP`（自检不替用户花钱）✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 未变 ✅ |

#### 目标 ② 的完成度

| 要素 | 状态 |
|---|---|
| 按员工 / 按任务自选（含本地档） | ✅ |
| 静态 failover + 撤退队列 + 历史 | ✅ |
| 熔断判定 + 计数 + 切换链路 | ✅ |
| **熔断的真实触发** | ✅ **已实测**（不可达供应商，阈值 1） |
| 自动切换在界面上可见 | ✅（warn 样式 + "连续失败"字样） |
| 用引擎自报 `contextWindow` 替换型号推导 | ❌ 仍是内置表 |

### 10.28 第二十七刀：项目级引擎定义 —— 以及"点名 X 却跑了 Y"

`~/.onecode/engines.json` 是**全局**的。但"这个项目需要不同的引擎参数"是真实需求（比如本仓要 `--append-system-prompt-file`、那个仓要 codex）。这一刀加上**项目层**：`<项目>/.onecode/engines.json`。

#### ★ 先想清楚安全问题：项目层**只能新增，不能覆盖**

这不是洁癖。你 `git clone` 一个仓库、在调度台里打开它 —— 如果项目层能覆盖 `claude-code` 的 `cmd`，那么**开一个槽位就等于执行仓库里写的任意命令**，而且你完全看不出来。

所以规则是：

| 情形 | 结果 |
|---|---|
| 项目层定义**新 id** | ✅ 生效，来源标成 **来自项目**（跟着仓库走） |
| 项目层定义里 id 与**已有引擎重复** | ❌ **拒绝**，并如实报出问题 |
| 项目里想改 claude 的行为 | ✅ 走**新 id + `base` 继承**：`{"id":"claude-code-p","base":"claude-code","default_args":[…]}` |
| 项目想让自己成为默认引擎 | ❌ 做不到 —— "默认"只能由全局配置指 |

真机上验到（临时 HOME + 演示项目）：

```
[engines] /tmp/onecode-proj-demo/.onecode/engines.json: 项目层不能覆盖已有引擎 "claude-code"
          （克隆一个仓库就能改掉要跑的命令）—— 请用新 id + base 继承
list_backends → 8 个引擎        （内置 7 + 项目层 1）
```

#### ★ 真机验证又抓到一个真 bug：**静默替换**

第一次真机跑，日志是这样：

```
[structured] backend=proj-only-xyz protocol=print cmd=claude args=["--permission-mode","bypassPermissions", …]
```

**点名的是 `proj-only-xyz`，实际跑的是 `claude`** —— 而且**没有任何提示**。根因：`plan()` 解析引擎时，id 找不到会**静默退回内置 `claude-code`**，但 `SpawnPlan.backend` 仍然写着点名的那个 id。

这正是本项目最在意的那类毛病（第 3/4/9/12/17 刀都栽在"降级没说出来"）。修法：**把"点名的"与"实际用的"分成两件事**。

| 字段 | 含义 |
|---|---|
| `backend` | **实际用的**引擎 id（日志与界面以此为准） |
| `backend_requested` | 你点名的那个（可能不存在） |
| `backend_fallback` | 是否发生了替换 |

并且：`describe()` 会写出 `⚠ 点名的是 X（不存在，已退回）`，`plan.warnings` 里也加一条 —— 终端上的诚实提示机制会自动把它显示出来。

顺带把"到底要跑什么"的一句话描述抽成**一处实现**（`spawn_plan::describe`），两条运行路径（`engine_run` / `structured_send`）都用它 —— 排查时第一个要看的就是这句话，两处各写一份迟早会不一致。

#### 验证

| 检查 | 结果 |
|---|---|
| `cargo test` 并行 / 串行 | **294 passed / 0 failed**（本轮 +9，exit=0） |
| `npm test` | **331 passed / 0 failed**，exit=0 |
| `npm run test:render` | **7/7**，exit=0 |
| `npm run test:boot` | **30/30**，exit=0 |
| 真机 · 项目层可见 | `list_backends → 8 个引擎`（内置 7 + 项目 1）✅ |
| 真机 · 覆盖被拒 | 告警原文如实输出，"请用新 id + base 继承" ✅ |
| 项目层只能新增 / 能 base 继承 / 不影响无项目时 | 测试 ✅ |
| **点名不存在 → 如实报出** | `backend`=实际、`backend_requested`=点名、`describe` 带"点名的是" ✅ |
| 真实 `~/.onecode/providers.json` | sha256 / mtime 未变 ✅ |

#### 目标 ① 的完成度

| 要素 | 状态 |
|---|---|
| 引擎抽象 / 可扩展（全局） | ✅ |
| **项目级引擎定义** | ✅ **只能新增**（安全规则 + 真机验证） |
| 强制"点名 vs 实际"可见 | ✅（真机验证时发现并修掉静默替换） |
| 终端槽位 / 结构化槽位 / ACP 类 | ✅ |
| ACP **常驻**槽位 | ❌ 仍是一轮一次 |

### 10.29 累计验证证据（二十六轮 + 二十七刀）

| 检查 | 结果 |
|---|---|
| `cargo check` | 无告警 |
| `cargo test`（并行 / 串行各 2 轮） | **294 passed / 0 failed**，确定性、不再 flaky |
| `npm test` | **331 passed / 0 failed** |
| 真实 `~/.onecode/providers.json` 在多次测试前后 | **sha256 与 mtime 完全一致**（改动前那个测试会覆写它） |
| 真实 agent 定义解析 | 实测解析到 1 个（`~/.claude/agents/dev.md`，以前隐形那个） |
| 真实 agent → 参数 | `acceptEdits` + `Write,Bash,Read` 白名单，bypass 被整段替换 ✅ |
| 真实技能 → 注入 | `~/.agents/skills` 的技能可解析；127KB 的技能走 deferred 给路径 ✅ |
| 真实 git 度量 | 临时真仓库 numstat `+1`、未跟踪文件出现 ✅ |
| 真实 agent + 真实技能 → 注入文本 | 含产出协议（有测试钉住）✅ |
| 真跑证据命令 | 退出码 / 输出 / 非零码如实记录 ✅ |
| 超时墙钟上界 | 600ms 超时 + 泄漏孙代 → 1.13s 返回 ✅ |
| agent 自报完成 | 任务文件里的 `done` 只算自报，被标记且不生效 ✅ |
| 真实 token 账本 | 快路径 246ms；全库调度台 13.0B / 自己 7.98B tokens ✅ |
| 按员工自选模型 | 花名册绑定 vs 实际并排；failover 原因可见 ✅ |
| 本地模型档 | 无凭据的 localhost provider 保留、不被切走 ✅ |
| 可扩展引擎定义 | 无配置时内置 7 档逐字段不变；用户定义可加/覆盖/继承 ✅ |
| 真实引擎能力协商 | 539ms 拿到引擎自报的模型/权限档/凭据来源/工具集 ✅ |
| 引擎取值对齐 | `planMode` 非法已修；非法值被拒绝递出并报出可用值 ✅ |
| 工作台面板 | 右栏「工作」页；渲染实测 7/7 通过；**自报完成单独成组** ✅ |
| **真机启动** | 构建+启动成功、0 panic、JS→Rust IPC 通、启动降级带后果被记录 ✅ |
| **前端启动 + 真实流程** | `npm run test:boot`：0 启动错误 + 19/19 流程；反向对照三类故障可抓 ✅ |
| **按任务自选模型** | 任务字段压过员工花名册；账本按任务计费 ✅ |
| **真实金额进账本** | 引擎自报 `total_cost_usd`；按员工/按模型拆分；无记录时零金额 ✅ |
| **跨进程 spawn 端到端** | 桩引擎收到姿态/白名单/身份文件/路由凭据；PATH 保留 ✅ |
| 前端渲染（工作台） | `npm run test:render`：7/7 ✅ |
| 全仓残留检查 | 无旧字段名、无死代码（`cargo check` 零告警） |

