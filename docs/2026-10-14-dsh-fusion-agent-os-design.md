# OneCode OS — DeepSeek Harness × OneCode 融合形态设计

> 日期：2026-10-14 ｜ 作者：@dev（应董事长要求）
> 输入：① DeepSeek Harness 发行版源码（本机 `DeepSeek Harness.app`，v0.2.0-rc.2，288 包）② `eng/onecode` v0.5.0（**已停更**）③ `eng/onecode-desktop`（**活跃**，Tauri v2 + Rust）④ `eng/ace` 实测数据 ⑤ 外部公开报道 ⑥ 董事长既有判断（`desktop-agents-ux-comparison.md` / `xcode-agent-brief.md`）
>
> **证据等级**：**[实测]** 直接读源码/文件确认（含实跑测试）｜**[报道]** 外部公开信息，未独立验证｜**[推断]** 我的判断｜**[未验证]** 明确不知道
>
> **修订状态**：v3（2026-10-14）。v1→v2 修正 DSH 侧 6 处事实错误（§0.4）；v2→v2.1 补第二轮深读（§0.5）；**v2.1→v3 修正 OneCode 侧的根本性误判（§0.6）**。

---

## 0. 结论先行

### 0.1 一句话

> **不要做第 X 个 harness，也不要做更好的终端。做一个"Agent 调度台"：软件单元是 Playbook，工作单元是 Slot/Session，组织单元是 Team，信任单元是证据化审查队列，经济单元是"经过验证的每任务成本"。**

### 0.2 四条硬判断

**① 引擎层已经商品化，而且是免费的。**
DSH 全部 288 个包是 **MIT [实测]**；Codex Harness 是 Apache-2.0 **[报道]**；社区 8 天冒出 2600+ 插件仓库 **[报道]**。**"我有个好框架"不再是护城河。**

**② 瓶颈从"模型够不够强"迁移到"人看不看得过来"，而 DSH 是全行业把"上下文经济学"做得最深的一家。**
- 外部数据点：仅调 harness（保留推理 + 上下文压缩），ARC-AGI-3 得分 13.3% → **38.3%**，输出 token 降 6 倍 **[报道]**。
- 我们自己的数据：ACE 用本机 **50,564 次真实调用 / 58 天**测出 —— 成本三分**未命中输入 49.3% / 命中输入 35.9% / 输出 14.9%**；模型分层路由省钱天花板 **<2%**；本地判官**假通过率 100%**；真正的杠杆是**上下文瘦身 ≈25% + 调用次数 ≈20% + 峰谷 5–10%** **[实测 ace/docs/03–07]**。
- **咬合点**：DSH 的 `token-meter` 投影**恰好输出这四个计数器**（`uncachedInputTokens`/`outputTokens`/`cacheReadTokens`/`cacheWriteTokens`），重放一致、零模型调用。**ACE 要的成本地图，DSH 免费给。**

**③ DSH 与 OneCode 缺的正好是对方有的，接口天然吻合。**
- DSH `dsh-sandbox` 原文："需要整环境隔离时用容器、microVM 或远端执行器" **[实测]** —— 而容器是 OneCode 的核心资产。
- `dsh-api-terminal-controller` 上限 **8 终端/会话、500×200、scrollback 1000、2MiB/订阅者**，且它的设计目标是"**操作者自己的一个终端**"：**输出永不进 agent transcript**，无多端重放、无慢客户端驱逐 **[实测]** —— OneCode 的 PTY 多路复用**正好补这个位**。
- DSH 有进程级 launch token + 签名 cookie，但**没有多用户身份/租户、无登出、拒绝 `--host 0.0.0.0`** **[实测]** —— OneCode 有反代、服务注册、远程访问、移动端。

**④ OneCode 的真实重心不是 web，是 desktop。**（这条 v1/v2 判错了，见 §0.6）
`eng/onecode`（web）**已停更**（3 commits，最后 2026-08-10）；`eng/onecode-desktop` **是活的产品**（62 commits，活跃到 2026-09-24），是 **Tauri v2 + 7,473 行手写 Rust** 的原生重实现，**不是套壳**。它有：多槽 PTY、**多 provider 切换（M1 已验收）**、SQLite 会话存储、健康循环、托盘、以及**全仓最有原创性的产物 `model_context.rs`（上下文窗口推导 + autoCompact 预算数学）**。→ **后继方案的立足点是 desktop；web 端是零件库，不是平台。**

### 0.3 新形态（一句话素描）

**三个表面 × 五层：**
- 🌱 **Pocket**（手机/语音）—— 人不在场时：审查队列、一键批准、接管
- 🖥 **Studio**（桌面）—— 深工作：Slot 花名册、会话账本、diff 审查、Playbook 编辑、两种终端
- 🏢 **Fleet**（团队/组织）—— 多成员、看板、预算、审计导出

**五层：** 引擎层（可换）→ 隔离层（容器 + PTY）→ 能力层（Playbook）→ 交互层（调度台）→ 治理层（证据 + 钱）

### 0.4 v1 → v2 修正记录：DSH 侧的 6 处事实错误

| # | v1 的错误说法 | 事实 | 影响 |
|---|---|---|---|
| 1 | "DSH 无 TLS/认证/policy → 缺认证" | **错。** webserver **包**自身不带，但应用层有：每进程随机 launch token → `GET /?token=` 换成 authority-bound 签名 cookie（`HttpOnly`+`SameSite=Strict`+30 天），**401 发生在 RPC dispatch 之前**；另有 Host/Origin 信任检查防 DNS rebinding（403），且 **`dsh web --host 0.0.0.0` 不支持** **[实测 `dsh-client-connection/README.md:39-43`]** | DSH 缺的是**多用户身份与租户**，不是认证。§1.8/§4 已改 |
| 2 | 把"运行时动态挂载/卸载插件"当成创造模式 | **错。** 模型侧**没有** `cordis_define`；`tool-cordis` 只有两个**只读**工具；host-runner README 原文："Agent 通过 tool-cordis 发现 API，**通过 Plugin Manager 安装持久化 bundle**；没有模型工具创建动态定义"。动态 runner 是**编程/嵌入 API**，"带浏览器半的包在没有页面连接的地方会永久挂起" **[实测]** | §1.7 已改：创造模式 = 只读内省 + `plugin_manager install_bundle`（持久） |
| 3 | "每包提供 `lib/types/**/*.d.ts`" | **错。** 288 个包里 **`.d.ts` 数量 = 0**（asar 里也没有），但 `package.json` 仍声明 `types: lib/types/index.d.ts`，且出厂技能指示 agent 去读它 **[实测]** | 类型驱动扩展在**发行版里不被支持**；是实打实的文档/产物缺陷。§1.1 已改 |
| 4 | 说 DSH 有"多端重放"能力 | **错。** DSH 的终端是"人自己的终端"（系统用户权限、**独立于 agent 沙箱与审批**、**输出不进 transcript**），单会话最多 8 个，无多端重放/慢客户端驱逐 **[实测]** | **这是设计洞察**：终端有**两种**。见 §6.1 |
| 5 | 未提 DSH 的默认数据出境 | **重大遗漏。** `dsh-session-log-deepseek` 的 `enabled` 默认 **`true`**、`maxBytes` **8 MiB**，把 transcript 作为 `dsh_session_log` 字段**塞进每一次官方 API 请求**（at-least-once）；OTel 侧很克制但**redaction 规则出厂为空** **[实测 `dsh-session-log-deepseek/lib/index.js:18-19`]** | 任何"本地优先"叙事必须**先关掉它**。见 §1.9/§9 |
| 6 | 未提"全仓无预算概念" | **重大遗漏。** 全仓 grep `tokenBudget`/`budgetCredits` = **0 命中**；子代理"slots 不约束累计历史或 token 用量"；workflow 上限是"cooperative，不是 host-enforced 配额" **[实测]** | **验证并强化**本设计的核心新建项（§6.4）。DSH 最强的架构 + 最空的护栏 = 我们的位置 |

### 0.5 v2 → v2.1：第二轮深读的补充 [实测]

1. **两套事件命名空间，别混。** Cordis 事件（`agent/pre-step`、`tools/execute`、`session/flush`…）≠ 会话日志事件。会话日志词表是**冻结生成的 58 条** `KNOWN_SESSION_EVENT_TYPES`；词表外类型需带 `ignorable: true`（这正是"新增普通事件类型**不**需要 bump 格式版本"的机制）。
2. **减少上下文的顺序是按"成本"排的，不是"一个摘要器"。** 逐结果保留（head/tail 按字节，**绝不切断码点**）→ spill（给真实文件路径）→ **无模型**裁剪（`8192/4096/1024`）→ **付费** LLM 摘要 → 图片卸载。**先把免费的用完再付费**；裁剪后压力可能已够，摘要**整步跳过**。
3. **`maxParallelToolCalls` 默认 10**；重试默认最多 5 次（`500ms→10s` +10% 抖动），`always` 模式无上限；**持久化先于等待**（先落 `llm/retry` 再起定时器）。
4. **三个 fail-closed 落盘闸**：适配器建流前、顶层工具体执行前、每个 `agent/pre-step`。写不成功就不跑。
5. **超时没有全局默认**：`timeoutMs` 来自工具自身定义，**没有 `timeoutMs` 的工具（bash/read/write/edit）永不被切断**。
6. **会话物理格式**：JSONL + **逐批 zstd 帧** + 校验和；每次 append **先 fsync 再算完成**；迁移**非破坏**（只读打开在内存里迁；写打开走同目录临时文件 + Worker 校验 + 重查源版本，**源文件字节不变**，产物按代命名 `session.vN.jsonl.zstd`）。
7. **计量启发式是 4 字符/token**，且**明确承认 CJK 与 JSON schema 被严重低估** —— 这直接约束我们的成本账本（§6.4）。
8. **`replay-safe` 用词已交叉核实**：该词**确实出自 DSH** —— `dsh-compaction-tool-result-pruner/package.json` 的 `description` 与它自己的 README。保留该表述并标注出处（深读时只搜了 README 正文，漏了 description）。
9. **迁移的代价是自认的**：历史性 `stat`/`list` 的工作量与根目录下会话**数量成正比**，且只读打开**从不**发布升级，所以冷进程会**重复扫描**（§9.1）。

### 0.6 v2.1 → v3 修正记录：OneCode 侧的根本性误判 [实测]

**我原来把 `eng/onecode`（web）当成"产品本体"、把 `onecode-desktop` 当成"套壳"。事实相反。**

| # | v2 的说法 | 事实 | 影响 |
|---|---|---|---|
| 7 | "onecode 是产品，desktop 是薄壳/Tauri 套壳" | **反了。** web 端**已停更**（3 commits，最后 2026-08-10）；desktop **是活的产品**（62 commits，活跃到 2026-09-24），**不是套壳** —— Tauri v2 + **7,473 行手写 Rust**：`pty/mod.rs`(1146) 多槽 `MultiPtyManager`、`slot.rs`(439)、`providers.rs`(1171) 多 provider 切换（M1 已验收 2026-08-24）、`model_context.rs`(268)、`session/`(163)、`fs_explorer.rs`(549)、`cc_sessions.rs`(498)。Rust **从不调用** Node 网关，PTY 走原生 `portable-pty`；两边是**概念移植，不是共享代码**（`copy-static.sh` 只从 npm vendor xterm/marked/hljs/Monaco，**没有从 `onecode/` 拷任何文件**） | §2 重写；§7 映射表重写；**立足点改为 desktop** |
| 8 | "角色系统只改提示词" | **比我说的更糟：`@mention` 根本不派发。** `cc-status.js:286-325` 解析出角色列表 → 侧栏；`onecode.html:409-411,453-458` 只往 PTY 写裸字节 `'@'+aid+' '`（先用 DEL 擦掉已输入的字符）。**没有路由、没有 system prompt 隔离、没有会话分离。** README:17 的"10 个 agent，一人公司全栈协作"是**营销覆盖在一个文本宏上** | 这同时是**最大的自摆乌龙**和**最便宜的修复**（见 §8 Phase 0） |
| 9 | 未提维护浪费与测试真实状态 | web 端 README/CHANGELOG 声称"**277 assertions · 0 failures**"，**实跑是 302 assertions / 294 pass / 6 red**（其中一条是真 bug：版本号在 4 个文件里分裂成 0.4.0 与 0.5.0）。**desktop 的 268 个绿色 JS 测试测的是手写的 Rust 逻辑副本**（`ringbuffer.mjs` 自述"忠实移植…用于逻辑验证"），**并且把一个缺陷写成绿色断言**：Rust 移植 `slot.rs:88-97` 用盲字节 `drain`，**丢掉了 web 端 `pty.js` 的 UTF-8 边界保护** | §6.1/§9 增列；**不采信对外宣称的测试状态** |
| 10 | 未提真正的安全链 | **比 §2.3 原稿严重得多**：`oc:565-566` 挂载 `/var/run/docker.sock`，`entrypoint.sh:22-30` 把它 `chmod 666`；网关以 `node` 用户运行。**链：未鉴权浏览器 → `/ws/term` → PTY → `docker run --privileged -v /:/host` → 宿主 root。** 另有：`/ws/term` **无 Origin 校验**、`POST /api/restart-terminal` **无 CSRF**、`filebrowser --noauth` 暴露 `/workspace` 读写、`code-server --auth none` 且把 `ANTHROPIC_API_KEY` 写进其 settings、`~/.ssh` 只读挂载 | §9.1 单列 🔴 |

**同时要肯定的**：§5 的核心判断（"人机接缝才是产品"、"组合而非替代"）**不是我首创** —— 董事长在 `desktop-agents-ux-comparison.md` 里已经写了："同一类 agent 循环，但**人机接缝长在哪**完全不同"，结论 **"DSH 当大脑，Xcode 当工具箱"**。本设计是**把这条既有判断落到 DSH × OneCode 的具体形态上**，不是另起一个判断。

---

## 1. 证据 A：DeepSeek Harness 到底是什么

### 1.1 规模与形态 [实测]

实现全部在 Electron 的 `app.asar` 归档内（归档 = 8 字节头 + JSON 目录 + **按 4 字节对齐**的数据段）。解析后确认：

| 项 | 值 |
|---|---|
| 已发布包数 | **288 个** `@deepseek-ai/*` |
| monorepo 布局 | `packages/<域>/<名>`（由各包 `repository.directory` 还原）、`apps/{cli,web}`、`native/*`、`vendor/*` |
| 内核 | **Cordis 4.0.4**（`vendor/cordis`） |
| 许可证 | **MIT**（抽检 dsh / dsh-base / cordis / dsh-agent-loop / dsh-web-app / dsh-desktop-host 全部 MIT） |
| 版本 | 0.2.0-rc.2（**rc 阶段**）；`desktop-runtime.json` 固定 `hostProtocolVersion: 4`、node 24.18.1、pnpm 11.7.0 |
| 文档密度 | 每包 `README.md` + `README.zh.md`，含 **Known Limitations**、**Invariant ownership**、**Model Experience（Token effect / KV Cache effect）** |
| ⚠️ 产物缺陷 | **零个 `.d.ts`**，但每个 `package.json` 都指向 `lib/types/index.d.ts`，出厂技能也叫 agent 去读它 **[实测：`find -name '*.d.ts' \| wc -l` = 0]** |

外部报道称开源于 2026-08-13、50 个包组、12,293 次提交、社区 dsh-plugin 标签下 2600+ 仓库 **[报道]**，量级与我们实测的 288 包 / 50 组吻合。

### 1.2 内核机制 [实测]

**插件模型**：Cordis 是**基于 fiber 的 DI 容器**。三种插件形态 —— `Plugin.Function`（`(ctx, config)`）/ `.Constructor` / `.Object`（`apply(ctx, config)`），靠 `inject` 声明依赖，**加载由"服务是否可用"驱动而非注册顺序**；fiber 六态 `PENDING → LOADING → ACTIVE|FAILED → UNLOADING → DISPOSED`。插件创建的一切必须走 `ctx.effect(fn, label)`，返回 disposer，重载时逆序执行、失败只记日志不抛。**单个插件 `apply()` 失败只让该插件 `FAILED`，兄弟插件照常运行。**

**组合模型 = profile + bundle + patch 分层**。精确优先级（`readProfilePatches`，`dsh-app-boot/lib/index.js:1023-1031`）：

```
空根
 → bundles，按 dsh.profile.bundles 顺序
 → profile 的 cordis.patch.yml
 → $DSH_HOME/cordis.patch.yml（home 级，压在 profile 之上）
 → --patch 覆盖（可重复）
 → （若 DSH_TELEMETRY_DISABLED 非空）最后追加一条遥测硬关闭 patch
```

本机真实 profile（`~/.dsh/profiles/desktop/package.json`，**已验证**）：

```json
{"name":"dsh-profile-desktop","private":true,
 "dsh":{"profile":{"bundles":[
   "@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app",
   "@deepseek-ai/dsh-experimental-voice-input-bundle",
   "@deepseek-ai/dsh-experimental-agent-team-profile"]}}}
```

**Patch 语义**（`applyEntryPatches`，`cordis-plugin-include/src/index.ts:57-127`，**启动与 `--dump-config` 共用同一函数，所以 dump 不可能与启动漂移**）：
- `insert` 无 `id` → 追加到根；有 `id` → 目标必须存在**且** `group: true`，否则**警告并跳过**。
- 非 insert 的 patch 必须有 `id`；**未知 `id` 只警告跳过**；声明的 `name` 不匹配也跳过。
- 其余每个键**整块赋值** → **`config` 一改就是整体替换，不做深合并**。
- 空/纯注释的 patch 文件会让启动失败；用 `[]` 表示该层空。

**开箱 bundle**：`dsh-base`（共享内核，**94 个插件行** / 529 行 YAML 含逐行理由 [实测]）、`dsh-web-app`、`dsh-headless`、`dsh-sdk-app`、`dsh-sdk-minimal`、`dsh-acp-app`。`dsh-base` **刻意不挂 `dsh-invariants`**；`dsh-sdk-minimal` 才挂它 + 四个不变量伴生包。

**Agent 预设（preset）—— 全案最关键的一点**：preset 不是提示词模板，而是**一份声明式的"子插件清单"**，即**能力组合**：

| preset | 内容 [实测] |
|---|---|
| `standard` | persona + AGENTS.md/CLAUDE.md 加载 + bash/fs/搜索 + skills + goal + plan-mode + subagent + workflow + todo + 压缩/溢出策略 … |
| `minimal` | 极简（bash + 编辑器） |
| `ptc` | 把工具面换成 `run_code` + 生成的 SDK（程序化工具调用） |
| `cordis` | **创造模式** = standard + `tool-cordis`（只读内省）+ `customSkillDirs` 指向 `dsh-agent-preset/skills/` |

> **对 OneCode 的直接含义**：OneCode 的 `.claude/agents/*.md` **连提示词都只是被塞进 PTY**（§0.6 #8）。DSH 的 preset **改能力组合**（装哪些工具/技能、什么权限、什么人格、什么委派策略）。**"角色"的下一代定义 = 能力组合，不是文本。**

### 1.3 会话 = 事件溯源的日志（CQRS）[实测]

一句话设计原则：**"model-visible means logged"** —— 凡进入模型请求的东西，必须能从日志重建。

- **单一事实源**：append-only 的 `SessionEvent` 日志。信封恰好是 `{type, seq, time, data, surfaceOp?, sourceEventSeqs?, ignorable?}`，`seq = log.length` 连续从 0 开始。每次 append **快照并深冻结**负载、校验 lossless JSON（拒绝 BigInt/环/稀疏数组/`-0`/奇异原型），**拒绝的 append 不改变日志、派生状态或事件流**；热路径**从不阻塞 I/O**。
- **读模型分离（CQRS）**：log → **surface**（有序派生视图，`append` | `{op:'replace', startSeq, endSeq}`，端点是**当前 surface 顺序**上的闭区间）→ 投影。`session-projection` 是 merge 可扩展的类型表（`{key, stateSchema, stateVersion, init, apply, wire?}`，`apply` **必须同步**、且对无关事件**必须返回同一个状态引用** → 恒等即零下游工作）；另有 `session-projection-cache`（写回节流，需 `writeEveryEvents` + `writeIntervalMs`）、`session-query-sqlite`（**SQLite FTS5**，`unicode61`，**无 bm25** 而是手写排序）、`session-stats`、`session-turn-outline`、`session-title`。
- **压缩是"遮蔽"不是"删除"**：`replace` 把旧节点从未来输入中遮蔽，原始记录保留。
- **四个冻结格式迁移**：`v0→v1→v2→v3→v4`。链是 **build-static 且查缺口**的：`dsh-session-format-catalog` 在模块初始化时组装编解码器与相邻边，**拒绝重复编解码器、重复边、缺口、以及高于当前版本的条目** —— 任何 profile 都无法增删或重排迁移边。
- **物理格式**：JSONL，一事件一行，装在**带校验和的 zstd 帧**里；`create` 不写盘；首次 append 落 header+首批并 fsync；**后续每批先 fsync 再算 append 完成**，失败回滚到上一字节长度。**迁移非破坏**：只读打开在内存里迁；写打开走**同目录临时文件 + Worker 校验 + 重查源版本**再发布后继，**源文件字节保持不变**。
- **存储层不懂业务："破坏性修复"归读者。** 存储只返回"物理上有效"的日志，从不解释它；`agent-loop` 计算 `interruptedTurnClosers` 并当成普通批次追加；冷只读观察者只在内存里把中断日志"配平"。**代价（自认）**：每个读者都要重实现同一套修复语义，且历史性 `stat`/`list` 的工作量与根目录会话数成正比、只读打开从不发布升级 → 冷进程重复扫描。
- **fork**：按任意 `seq` 边界复制事件前缀成子会话，带 `inheritedEventCount`。

> **对 OneCode**：web 端唯一持久状态是 **10MB 环形缓冲的 PTY 回放** [实测 `pty.js:8`]；desktop 端的 SQLite **只存会话配置，不存内容** [实测 `session/`(163) "Config only"]。**终端字节流不是账本。**

### 1.4 上下文经济学（DSH 最被低估的资产）[实测]

罕见纪律：**每个包的 README 都强制标注该机制的 Token effect 与 KV Cache effect**。这是把"上下文的钱"当架构约束。

**两个必须抄的设计：**

**① 减少上下文的顺序是按"成本"排的，不是"一个摘要器"。**
逐结果保留（`TextRetainer` 按**字节** head/tail，**绝不切断码点/代理对**）→ **spill**（超预算结果换成首尾 + **真实文件路径**，模型可 `read`/`grep` 回收）→ **无模型**裁剪（`headChars 4096` + "middle pruned" 标记 + `tailChars 1024`，且约束 `head+marker+tail ≤ threshold`，所以**永远不会变大或反复重写**）→ **付费** LLM 摘要 → 图片卸载。**先把免费的用完，再决定要不要付费**；裁剪后压力可能已够，摘要**整步跳过**。**完整原始结果永远留在日志里**，可精确回放。这是全案最值得抄的工程品味。

**② 计量的启发式是 4 字符/token，且它自己承认不准。**
`CHARS_PER_TOKEN = 4`、每 block `+4`、角色框架 `+4`。README 原文：**"CJK 文本与 JSON schema 在四字符/token 下被严重低估"**，`contextBreakdown` 明确**不是计费数据**。→ **定价必须优先用 provider 回报的用量。**

| 机制 | 关键数字 [实测] | 作用 |
|---|---|---|
| `token-meter` | `tokenUsage{uncachedInputTokens, outputTokens, cacheReadTokens, cacheWriteTokens}`、`contextPressure{pressureTokens, projectedTokens, contextWindow}`、`contextBreakdown{systemTokens, toolsTokens, messageTokens}`；**重放日志、零模型调用、确定性、O(surface)**；provider 用量**只在规范信封一致且不低于该次锚点时**才复用 | 计量底座 |
| `output-retention` | `ItemRetainer` 精确 omitted 计数；`TextRetainer` 按字节 head/tail，**绝不切断码点** | 第一道（免费） |
| `spill-policy` | `maxInlineTokens: 12500`（**省略即禁用**）；文本+图片共享预算；模型可见文案含 `(Omitted N bytes. Full formatted result stored at: <path>. Use read with offset/limit, or grep this path…)`；**spill 失败只警告、保留原文** | 第二道（免费且**可回收**） |
| `compaction-tool-result-pruner` | 包描述原文 **"Replay-safe model-free head/middle/tail pruning"**；`8192 → 4096 + 标记 + 1024`；**无模型调用** | 第三道（免费，**最值得抄**） |
| `compaction-basic` | 阈值 `floor(min(W×0.8, W − O − 65536))`；`retainRatio 0.16`；`maxTokens 65536`；重试各 1 次；**每个区域都从第一个非 system 节点开始 → 系统提示（node 0）永不被遮蔽**；**恰好一个** `<compacted-summary>` 替换；**不收缩的摘要被拒绝** | 第四道（**付费 1 次**） |
| `compaction-image-offload` | 只处理 `IMAGE_OFFLOAD_REQUIRED`；**不消耗 provider 重试预算、不写 `llm/retry`** | 图片省钱 |
| `session-checkpoint-policy` | **三个 fail-closed 落盘闸**：适配器建流前 / 顶层工具体执行前 / 每个 `agent/pre-step` | 崩溃不重放、不丢 |
| `llm-retry` | 默认最多 **5** 次，退避 `500ms→10s` +10% 抖动；`always` **无上限**；**先落 `llm/retry` 再起定时器** | 抖动不烧钱 |
| `tool-call-timeout-policy` | 从**工具自身** `timeoutMs` 武装；**没有全局默认** —— 不带 `timeoutMs` 的工具**永不被切断** | 不卡死（但要自己设） |
| `fs-observation-policy` | 未观察 → `createIfAbsent`；已观察 → `replaceIfVersion`；未读就编辑 → `FS_NOT_OBSERVED`；版本旧 → `FS_STALE_VERSION`。**观察状态不持久化** → **每次 resume/fork 都要重读目标** | 防盲改、防覆盖并发写 |
| `repeat-tool-reminder` | 阈值 `[3,5,8]`；参数 key 排序后比对；**仅劝告** | 防空转 |

> **⚠️ 与 ACE 实测的精确咬合**：ACE 测出钱在"未命中输入"（49.3%），DSH 的 `tokenUsage` 恰好就暴露这四个计数器。**零埋点拿到 ACE 的成本地图。** 但注意计量启发式对中文低估严重 → 定价用 provider 用量。

### 1.5 程序化工具调用（PTC）与编排 [实测]

`dsh-tools` 三种呈现模式：`native` / `ptc` / `both`。**PTC 模式**下模型只看到 `run_code` + 一份生成的 SDK，然后**写代码调工具**：

```ts
await tools.bash({ command: 'pwd', description: 'Show current directory' })
// tools["exotic-name"](args)；失败抛 ToolCallError 可 try/catch
// 只读调用可 Promise.all 并发（安全调用并发，变更调用单独、按提交顺序）
```

- `run_code` 默认超时 **120s / 上限 600s**（含嵌套工具与审批等待）；并发子调用 `maxParallelSubCalls: 10`。
- **中间值执行局部、不进会话**；只有 `return`/`console.log` 进上下文。
- **PTC 的隔离是"指导"不是边界**：原文 —— "**the VM is not a security boundary** … **Network access is not restricted by the file policy**"。一个 workflow 脚本**可以开 socket**。

**workflow** 的钩子（**工具描述原文**）：

```
agent(prompt, opts?)    // 跑一个子 agent 到完成；无 schema 返回最终文本，有则返回校验过的对象；失败 resolve null
pipeline(items, ...stages)   // 无 barrier，逐项过阶段；stage 抛错 → 该项 null
parallel(thunks)             // 并发全部并 await（有 barrier）；抛错 thunk → null
phase(title) / log(message) / args
// script 没有 fs / network / timer / Node API —— 活由 agent 干
```

引擎上限：`maxTotalAgents: 1000`、`maxItemsPerCall: 4096`、`syncTimeoutMs: 5000`；**无 journaling / 无 resume / 无 token-budget 词汇**。**ralph**：每轮全新子 agent，`maxRounds: 256`，`maxHandoffChars: 16384`，**shared workspace 就是长期记忆**，**出厂默认关闭**。

> **为什么这跟 ACE 数据直接相关**：PTC 与 workflow 的本质都是**减少来回次数** —— 把 N 次"模型→工具→模型"往返压成 1 次代码执行。ACE 测出"调用次数 ≈20%"是三大杠杆之一，**DSH 把它做成了机制。**

### 1.6 多智能体与长期自治 [实测]

| 机制 | 关键设计（含硬上限） |
|---|---|
| `subagent` | `spawn`（全新）/ `fork`（继承 Lead 已完成轮次前缀，收益是 KV 前缀复用）。**硬上限**：`maxDepth` 默认 **1**（子代理不能再派；本会话真的撞到过 `subagent depth 2 exceeds maxDepth 1`）；`maxActiveSubagents` 默认 **8**，**满了直接拒绝、不排队**。**没有 token 预算** |
| 子代理权限 | **不继承工具与权限**。**审批策略被硬钉成 `never`，无论父级是什么**；只传会话**显式**的 `sandbox/mode` 覆盖 |
| `experimental-agent-team` | **Lead = 会话本身**（`TeamId == SessionId`）；durable roster + peer mailbox + 共享任务板；**任务 CAS 版本号**；依赖 DAG + 环检测；`writeScopes` **只是重叠告警**。上限：**16 成员 / 256 任务 / 64 待投递消息 / 64KiB 单消息**。**自述"Experimental prototype with no stability promise"：单进程、单 checkout、非跨进程 exactly-once、无 worktree/merge/lock、无自动释放 ownership、花名册扁平不可嵌套/改名/删除** |
| `goal` + `goal-round-driver` | 每会话**至多一个**目标；`maxGoalRounds` 默认 **256**；`blocked` 在连续 **3** 轮前**机械拒绝**。创建/编辑/暂停/恢复**必须有一次"运行根 agent 当轮的直接人类消息"**（子代理与调度器不能伪造）；`complete`/`blocked` **纯模型自述，无机械验证器**；**会话启动边即 disarmed**，需人显式 resume |
| `schedule` | 一条 storage 记录 + **一个 Host 定时器**（**不是 launchd/cron**）。**错过只补最近一次**；回执 ack 的是**收件箱投递，不是模型执行**；崩溃可能**重复投递**；**纯 headless/SDK 挂不上** |
| `webhook` (+github) | **HMAC 在 JSON 解析之前**；`202` 只表示"已验证并在内存里派发"。**无队列/无重放/无重试/无去重**；`permissionPreset` **由规则自选**（治理缺口） |
| `jobs` | `dsh-jobs-local` 是**纯内存**："**每条记录在 harness 进程退出时消失**"。上限 `maxConcurrentJobsPerOwner 10`；"`bash-1` 这类 id 可预测，所以这道栅栏是**授权，不是保密**" |

### 1.7 自扩展：创造模式（关键杠杆 + 最大风险）[实测]

出厂即带 `dsh-agent-preset/skills/cordis-plugin-development/SKILL.md`（含可复制的双半模板）。

- **工具**：`cordis_inspect_list` / `cordis_inspect_query`（**严格只读** —— 可查 Service 方法签名、事件模式、**活动 Loader 条目及其 Config JSON Schema + `packageDir`**、可见工具、**活动 Web UI 的 slot 树与主题 token**；原文："This Tool cannot invoke business Service methods or modify the runtime"）；`plugin_manager`（`install_bundle` / `set_bundle` / `remove_bundle`…）。
- **流程（技能原文）**：解析目标 → 只发现需要的 API → 读模板 → 写进工作区 → `install_bundle` → **读安装结果的 `application`/`warnings` 判断是否生效（不是看日志）** → 用连接中的页面做视觉验证。
- **持久性**：装到 profile 后**跨会话持久、影响该 profile 的每个会话**。替换已装包**必须重启**；配置热重载好，代码热重载是 best-effort。
- **权限**：`plugin_manager` 每次调用都要审批。工具自己的 justification 原文：**"Profile changes persist across sessions; installed Host code runs outside the workspace sandbox."** 安装会**在 profile 目录跑 pnpm**；被 pnpm v11 拦下的构建脚本走 `pendingBuilds`，**只有用户显式批准**才能通过 `approvedBuilds` 执行。
- **⚠️ 澄清**：**模型侧没有 `cordis_define`**。动态双半 runner（`node:vm` + 浏览器半）是**编程/嵌入 API**，其 README 原文："Agent 通过 tool-cordis 发现 API，通过 Plugin Manager 安装持久化 bundle；**没有模型工具创建动态定义**"。动态 runner 的隔离也是"**不是安全边界**"，**挂起的 run 请求没有超时**，**带浏览器半的包在没有页面连接的地方会一直挂到轮次被取消**。

> **含义**：价值在于"Agent 学会一个新能力后，可以自己写代码**固化成可安装的包**"；风险有两条 —— **① 装进去的 Host 代码跑在工作区沙箱之外；② 安装会执行第三方包的构建脚本**。合起来是一条**供应链 + 提权**通道。任何面向多用户的 OneCode OS 都必须建立**签名 + 权限声明 + 安装入审查队列**（§6.2/§6.3）。

### 1.8 表面与互操作 [实测]

- **Web UI 本身是插件组合**：四类 slot（`single`/`list`/`keyed`/`chain`）+ 五组框架注入的 prop share + 组件工厂；`ui-renderer` 做程序里**唯一一次** `renderSlot('root')`。**已观测 slot 字面量**（可直接挂我们的 UI）：`conversation.composer.dock`、`conversation.chat.turnTail`、`conversation.chat.node`、`conversation.session.header.corner`、`main`(keyed)、`shell.leading`、`shell.overlay`、`sidebar.right.pane.tab`(+`.title`)、`sidebar.right.tab.{guide,menu.item,document,files.actions}`、`tool.call.toolview`(keyed)、`deliverables.file.actions`、`deliverables.review.file.actions`。
- **交付物体验（IDE 级）**：每轮结束的**改动文件卡**（逐文件 +/- 行数、binary/too-large 态、四行后折叠）、**显式交付卡**（`present`，≤4 文件/次）、**悬停 500ms 出 diff 浮层**、**右栏 `changes-review` 标签页做真 diff 审查**（并排⇄内联、换行、Shiki 着色、单侧增/删时单列、5000 行截断）、正文内联代码路径可点击跳转（**basename 不唯一就故意不可点，不猜**）、原生应用打开。
- **轨迹**：`ui-trajectory` —— 纯投影的事件账本 + 交互式耗时总览，助手条分 **TTFT / decode**，可拖选过滤、滚轮缩放。
- **右栏四面板，权限刻意不对称**：

| 面板 | 后端能力 | 关键约束 |
|---|---|---|
| 浏览器 | **无**（Web 用 iframe，Desktop 用 Electron `<webview>`） | **不向被访问内容注入任何 Electron/Node 能力**；拒绝 `file:`、脚本/blob、内嵌凭据、DSH 自身 origin |
| 终端 | 真 PTY 服务 | **"用户终端使用执行环境中系统用户的权限，独立于 Agent 的沙箱模式和审批策略"**；**输出永不进 transcript**；8 终端/会话 |
| 文件树 | `workspaceFiles`（**只读**） | 懒加载、自动刷新、按 tab 记忆展开与滚动 |
| 文档预览 | 注册表 + 三种加载模式 | 表格**客户端渲染**；Office → 宿主 LibreOffice 转 PDF（2 并发/60s/50MiB 入/100MiB 出）；HTML 默认 DOMPurify + CSP 禁脚本，开启交互则 `sandbox="allow-scripts"` **且故意不加 `allow-same-origin`** |
- **互操作矩阵**：MCP client（`tools/list` → `mcp__<server>__<name>`；stdio 环境洗掉 `*KEY*/*PASSWORD*/*SECRET*/*TOKEN*` 与全部 `DSH_*`；或 Streamable HTTP）、MCP resources（3 个显式 `server` 参数的工具）、**ACP 服务端**（只服务标准 ACP v1；**故意省略** `session/load`、fork、modes、commands、plans、terminals）、**SDK JSON-RPC 服务端**（**无取消、无关闭、无版本协商**，`serverInfo.version` 冻在 `0.0.1`）、**Claude Code hooks 桥（30 个事件只支持 7 个）**、**Codex hooks 桥（10 个只支持 5 个）**、SKILL.md 技能（**只在深度 1 发现**；根优先级 100 项目 `.dsh/skills` / 200 `.agents/skills` / 300 `customSkillDirs` / 400 `$DSH_HOME/skills` / 500 agentsHome / 600 内置）、签名 webhook。
  - ⚠️ hooks 桥的**承重脆弱点**：`transcript_path` 硬编码成 `''`/`null`（默认会话日志 zstd 压缩、hook 脚本读不了）→ **任何读 transcript 的 hook 会静默降级**；`{"continue": false}` **折叠进了 stop 但从不被强制**。
- **认证（v1 说错的地方）**：应用层有 —— 每进程随机 launch token → `GET /?token=` 换成 **authority-bound 签名 cookie**（`HttpOnly`+`SameSite=Strict`，30 天，去 `Secure` 因为是 loopback HTTP）→ **坏 cookie 在 RPC dispatch 前 401**；另有 Host/Origin 信任检查（非 loopback Host 或跨站 Origin → 403）。**`dsh web --host 0.0.0.0` 不支持。** 缺的是**多用户身份、租户、无登出**。
- **桌面载体**：Electron 壳与**独立 Node host 进程**（`ELECTRON_RUN_AS_NODE`）分离；host 调 `runProfile({profile:'desktop', args:['--no-open','--port','19387'], ...})`，**端口 19387 硬编码**。退出前**先锁准入**（`connection/request` 答 503）并等在飞请求结束，还会报告是否有会话挂着 `schedule` 提醒。

### 1.9 治理与安全 [实测]

**沙箱"真分档 + 失败即拒"**：

| 档 | 平台 | 原语 | 限制 | 强制级别 |
|---|---|---|---|---|
| `bwrap`（Linux 首选） | Linux | bubblewrap 命名空间 + bind mount | 只读宿主根、私有 PID 命名空间的 `/proc` | `full` |
| `landlock`（Linux 回退） | Linux | Landlock LSM | 按模式的路径允许清单 | `full`（老内核自报部分） |
| `seatbelt`（macOS 唯一档） | macOS | `sandbox-exec` + SBPL | `(deny file-write*)` + 白名单 `subpath` | `full` |
| `windows-acl` | win32 | AppContainer / 受限令牌 + NTFS DACL | **只限写；读不受限** | `partial` |
| `dsh-fs-sandbox` | 全平台 | **不是内核边界**，是可信代码里的策略检查 | 写前重新规范化并检查包含 | 进程内栅栏，**接受 TOCTOU** |

**无链或全部探测失败 → 在 `confine()` 处失败关闭（`SANDBOX_UNAVAILABLE`），绝不放行无约束执行**。`sandbox-policy` 每次调用解析**一个完整策略**：已批准的单次授权 > 会话最近 `sandbox/mode` 事件 > 部署默认；`mode` 默认 **`read-only`**；工作区根是会话**不可变**的 `SessionHeader.cwd`。

**人在环**：
- `user-approval`：结果集封闭，**`allowed-once` 是唯一的授予** —— seam 里**没有"总是允许"**；策略只有 `ask`（默认）/`never`；**失败关闭**；每次询问写 durable `approval/asked`+`approval/decided`，且**必须有开着的轮次**。
- `permission-presets`：出厂 `workspace-write→ask`、`danger-full-access→never`；在**不设限的 shell 上挂它会直接拒绝加载**。
- `ask_user_question`：`askTimed()` 到点**释放模型并返回 pending，绝不中止轮次**；**子代理不能开人类交互** —— 必须把问题放进自己的最终结果。
- **`plan-mode` 没有牙**：原文 —— "It does not restrict the agent: every tool stays callable"，只是"一个 prompt 段 + 一个工具，靠文字约束而非过滤能力"。**不能把"计划已批准"当安全边界。**
- `experimental-auto-review`（**出厂关闭**）：逐调用 LLM 授权，用**当前 agent 自己的 provider/model**；`low+allow`/`medium+allow|deny`/`high+deny`；**畸形响应与技术故障一律失败、绝不放行**；Auto 档把审批设为 `ask`，所以它**替人做第一道"允许"判定，把"拒绝"升级给人**；但在 `never` 下（**委派子代理就是被钉成 never**）拒绝是**终局**。成本：**每次多一次模型请求，无缓存**；README 自述它"可能放行不安全的操作、拒绝有用的工作、并多花 token"。

**⚠️ 数据出境姿态是"一半很克制、一半默认全开"**：
- **克制侧**：OTel 默认 `FEEDBACK_ONLY`（**`FULL` 是被拒绝的，不是别名**）；上传**由人类反馈授权**；**不继承给子会话**；`ctx.sessionTelemetry.emit()` 在任何模式下都是 no-op。
- **默认全开侧**：`dsh-session-log-deepseek` —— `enabled` 默认 **`true`**，`maxBytes` **8 MiB**，把 transcript 作为 `dsh_session_log` 字段**注入每一次官方 API 请求**，**at-least-once**。而且**两条路径的 redaction 规则出厂都是空的**。

**凭据**：`dsh-credentials-local` 是 `$DSH_HOME/.credentials.yaml` 的**明文 YAML**；README 原文："**文件权限做不到（把 provider key 挡在 agent 之外），因为 agent 的工具进程就是以你的 OS 用户身份跑的**"。

---

## 2. 证据 B：OneCode 到底是什么

### 2.1 诚实定位：一个名字下的三个半成品 [实测]

> **`eng/onecode`（web）是一个合格的本地 AI 终端多路复用器 —— 而它的"agent 层"是一个文本宏。`eng/onecode-desktop` 才是产品：durable 多槽终端 + 多 provider。`eng/onecode-cli` 不存在。**

| 腿 | 状态 | 事实 |
|---|---|---|
| `eng/onecode`（web） | **已停更**：3 commits，2026-07-22 → **2026-08-10** | 单进程 Node 网关 + 44KB 单页 xterm.js UI + **一个** `claude --permission-mode bypassPermissions` PTY |
| `eng/onecode-desktop` | **活跃**：62 commits，2026-06-19 → **2026-09-24** | Tauri v2 + **7,473 行手写 Rust**；多槽 PTY、二进制 IPC、SQLite、健康循环、托盘、多 provider 切换 |
| `eng/onecode-cli` | **不存在**：只有一个 `CLAUDE.md`（"规划中·未落地"），且不是独立 repo | 0 行代码 |

> **产品自己的审视文档比多数外部评审更狠**（`onecode-desktop/plans/active/产品审视与优化计划.md`）："**工程扎实、体验割裂**"；定位在"Claude Code 壳"与"多内核 AI 终端管理器"之间摇摆；"**管理多个 AI 终端是品类描述，不是购买理由 —— iTerm2 多 tab + Claude Code 就能做**"；名字名实不符；目标用户未定义。**已收敛的答案是「一人公司的 AI 员工调度台」** —— 正好就是董事长自己的运作方式（`CLAUDE.md`：董事长 + @ceo + 9 条线）。**OneCode 是为作者自己的编排痒处造的产品。**

### 2.2 真资产（值得留的）[实测]

**① PTY 多路复用器（web，475 行）—— 这类产品最难的部分，写得对、零依赖**
- **二进制分帧**：`0x00` 控制帧（`RESET`/`REPLAY_START`/`REPLAY_END`/`PTY_RESIZE`）/ `0x01` 数据帧，与文本零歧义。
- **10MB 环形缓冲 + 惰性裁剪**：`_lazyTrim` 用 `_trimOffset` 逻辑起点避免频繁 `Buffer.concat`；裁剪前用 `findUtf8Boundary()` 对齐 UTF-8 边界；chunk 数超阈值才 flatten。
- **双触发 flush**：50ms 定时（~20fps）+ **128KB 立即** + **用户输入后 100ms 内立即**。
- **多客户端重放**：`REPLAY_START` → 全量 replay → `REPLAY_END`；replay buffer 带脏标记缓存。
- **慢客户端驱逐**：`bufferedAmount > 256KB` 直接 `terminate()`。
- **尺寸协商**：取所有客户端 cols/rows 最大值；resize 防抖 150ms。
- **崩溃自愈**：指数退避 `500ms·2ⁿ` 上限 30s、10 次后放弃、稳定 5s 后重置。
- **无人观看时激进丢弃**：`clients.size === 0` 时缓冲降到 1/10 上限（不寻常但正确的判断）。

**② 原生多槽 PTY（desktop，Rust 1,585 行）—— 真正的产品内核**
`pty/mod.rs`(1146) 的 `MultiPtyManager`：`RwLock<HashMap<Uuid, TerminalSlot>>`（写/resize/replay/list 用读锁，spawn/kill 用写锁）、每槽 `Channel<Vec<u8>>`（**不用 base64**）、与 `pty.js` 相同的 50ms/128KB/100ms flush 常量、mpsc 读线程、移植的退避。`slot.rs`(439) 加 `TerminalSlot` + `RingBuffer` + **generation counter**（修掉了重启竞态）。
> ⚠️ **但移植丢了一样东西**：`slot.rs:88-97` 用盲字节 `flat.drain(0..drop_len)`，**丢掉了 web 端 `pty.js` 的 UTF-8 边界保护**，而且这个缺陷被写成了绿色断言（`tests/lib/ringbuffer.mjs:5` 自述"作为已知缺陷保留"）。

**③ 多 provider 切换（desktop，Rust 1,171 + JS ~1,851）—— 唯一已验收的里程碑**
`providers.rs` 做 provider CRUD、预设、`settings.json` 同步写 `CLAUDE_CODE_MAX_CONTEXT_TOKENS` + `autoCompactWindow`、连接测试；`src/provider-switch.js` 提供 `/model` 与 F2。**M1 于 2026-08-24 验收**，带两个 P1 债（`_switching` 无 watchdog，可永久锁死；`perform_switch` 双写非原子，会 split-brain）。
> ⚠️ Key 以明文存 `providers.json`，且 `providers.rs:796-799` 把 API key 放进 `curl` **argv**（`ps` 可见）。

**④ `model_context.rs`（desktop，268 行）—— 全仓最有原创性的产物**
从**两次真实生产 400** 反推（`docs/incident-20260923-context-overflow.md`）：用户输入 → 模型后缀（`[1m]`）→ 2 条有出处的表项 → 保守 128K；阈值 = 窗口 − 32k 补全 − 25% 突发 − 5% 余量；**永不覆盖 Claude 模型**。它解决的是**多 provider agent 宿主的通用难题：上下文窗口不确定，错了要么 400 要么白白压缩**。而且它**承认自己的缺口**（表里只有 2 条，其余回落到 128K，本地 Ollama 被算错）。
> **这是本设计认定的"楔子"**，见 §6.5。

**⑤ 其余可复用件**：SQLite 会话存储 + 启动恢复（事务、毒化互斥恢复，**但只存配置不存内容**）；`oc` 容器生命周期 CLI（本地优先镜像解析、容器 ID 校验 `^[0-9a-f]{64}$`、90s 健康轮询、结构化失败）；安装器 UX（步骤标记、带恢复的错误框、向导、交叉校验的常量）；code-server 集成 + **补丁台账**（`CODE-SERVER-PATCHES.md` 把每个上游补丁写下来并单点锁版本 —— **这个"台账"习惯本身才是可复用的东西**）；角色加载器 + `cc-status` + 无依赖 `cronNext`（129 行 Vixie cron 解析，含 DOM/DOW OR 逻辑与 2 月 31 日防护）；statusline（上下文/成本/git）+ `flock` 守护的 cron 追踪器；移动端 UI（4-tab、虚拟键盘、**CJK IME 修复**、惯性滚动）—— 这些是硬碰出来的 WKWebView 知识。

### 2.3 短板与真正的安全链 [实测]

| 问题 | 证据 | 严重度 |
|---|---|---|
| **未鉴权 + 远程可达 + 宿主 root（一条链）** | `config.js:9` `TERM_TOKEN` 默认 `''`，`term-ws.js:25-33` 仅在非空时校验；`index.js:189` 绑 `0.0.0.0`，`oc:995` 发布 `-p ${p}:7681`；**`/ws/term` 无 Origin 校验**，`POST /api/restart-terminal` **无 CSRF**（任何访问过的页面都能开 `ws://localhost:7681/ws/term`）；`oc:565-566` 挂 `/var/run/docker.sock`，`entrypoint.sh:22-30` `chmod 666`，网关以 `node` 运行 → **未鉴权浏览器 → PTY → `docker run --privileged -v /:/host` → 宿主 root** | 🔴 **最严重** |
| **权限全关 + 更多开口** | `index.js:165`/`entrypoint.sh:210` 用 `bypassPermissions`，`settings.json` 设 `skipDangerousModePermissionPrompt`；`filebrowser --noauth` 暴露 `/workspace` 读写；`code-server --auth none` 并**把 `ANTHROPIC_API_KEY` 写进它的 settings**；`~/.ssh` 只读挂载 | 🔴 |
| **供应链** | `install.sh:186-188` 用 `curl -k` 重试，且 **`README.md:58-59` 把 `curl -kfsSL … \| bash` 写成用户选项** → 一个持有 API key 的脚本里的 MITM→RCE；`oc:1606-1626` 自更新把远程 `install.sh` 管道给 bash，而 **SHA-256 分支是死代码**（`bin/install.sh.sha256` 不存在，已验证） | 🔴 |
| **命令注入 / 引用** | `install.sh:598,638,664,756,759` 对来自提示或 `--provider` 的 `PROVIDER` 做 `eval`；`oc:82` `eval "$cmd"`；`oc:1218-1219` `pkill -f "socat TCP-LISTEN:${port}"` —— **端口为空会杀掉所有 socat**，且子串匹配跨端口（`22` 匹配 `2222`）；`oc:1695` 未加引号的数组展开；密钥经 docker `-e` 与 `--registry-pass` argv 传递 | 🟠 |
| **`@mention` 不派发** | `onecode.html:409-411,453-458` 只往 PTY 写 `'@'+aid+' '`（先用 DEL 擦掉已输入）。**无路由、无 prompt 隔离、无会话分离** | 🟠 产品级 |
| **测试状态不可信** | web 端宣称"277 · 0 failures"，**实跑 302 / 294 pass / 6 red**（含真 bug：版本号 4 处 2 值）；`pty.test.js`/`index.test.js` 在缺 `node-pty` 时**静默降级为源码字符串 grep**（约 115 个调用点无行为证明）；`proxy.js` 零测试；**desktop 的 268 个绿色 JS 测试测的是手写 Rust 副本** | 🟠 |
| **脆性** | `scanDir`（`cc-status.js:158-213`）跟随符号链接且无深度上限 → `skills/` 下符号链接环会递归到 OOM；`pty.js:469` 模块级单例 → web 端**零并发**；code-server 在 boot 时打补丁，升级即静默失效（台账写了，但文档不是测试）；`onecode.html:547-550` 留着指向 `--noauth` filebrowser 的死 `auth()/X-Auth` 代码，**暗示了一个不存在的安全** | 🟠 |
| **desktop 特有** | `slot.rs:88-97` 丢 UTF-8 保护；CSP 仍允许 `unsafe-eval`/`unsafe-inline`；两个未修 M1 P1；`README.md` 的 `make install # npm install` 与 Makefile 矛盾；打包平台声明超过 README 说的"仅 macOS-ARM" | 🟠 |
| **规模天花板** | 单进程、单 PTY、内存缓冲、20 WS 上限、无持久内容、无租户。web 端的上限是"**一个用户、一个 agent、一台机器**" | 🟠 |

> **一个刺眼的对比**：**OneCode 的远程安全姿态比 DSH 差。** DSH 有进程级 token + 签名 cookie + Host/Origin 检查，且**拒绝** `--host 0.0.0.0`；OneCode 默认空 token、绑 `0.0.0.0`、无 Origin 检查、还挂 `docker.sock` 并 `chmod 666`。**在谈"多用户"之前，这条链必须先断。**

### 2.4 董事长已经想清楚的（本设计要接上的）[实测]

`desktop-agents-ux-comparison.md`（2026-10-13）用本机实测对比了 DSH、Xcode 27、Claude Cowork 三家，核心论点在第一段：**三者跑的是"同一类 agent 循环，但人机接缝长在哪完全不同"** —— DSH 的接缝是**权限档 + 审批卡**，Xcode 是**把 IDE 当 MCP 工具服务器、agent 当外部进程**，Claude 是**provisioned Linux VM + 文件夹信任**。全文围绕"下达→执行→审批→验收"这一循环、隔离矩阵、上下文/并行/长任务、扩展面、手感表组织。

**六条已经形成的直觉**：
1. **接缝才是产品**，功能清单不是 —— 批评 Xcode"未受门控的 build/test vs 受门控的 shell"造成"节奏割裂"，这是接缝批评。
2. **隔离模型就是身份** —— 权限表是全文最长一节。
3. **自查闭环是护城河** —— 结尾最战略的一句是 `xcrun mcpbridge`，让"**DSH 当大脑，Xcode 当工具箱**"。**组合而非替代。**
4. **证据优于感觉** —— 版本、体积（`claudevm.bundle = 9.9G`）、`mcp-server status`、从 `app.asar` 读出 287 个依赖，且**明确标注哪一条未验证**。
5. **收敛点识别** —— 三家都落到 `SKILL.md` + `references/` + `scripts/`，押注一个**可移植的扩展格式**。
6. **按任务选工具**，不是"一个赢家通吃"。

> **本设计的角色**：把上面这条既有判断，**落到"DSH 当大脑 + OneCode 当调度台"的具体形态上**。不是另起判断，是执行它。

---

## 3. 外部变量：未来 5–10 年

> 标 **[报道]** 的为外部公开信息，我未独立验证；**[推断]** 为我的判断。

### 3.1 已发生（2026 年内）[报道]

- **2026-08-13**：DeepSeek Harness 开源。Cordis 微内核、"一切皆插件"、四种模式（标准 / 极简 / PTC / 创造）。
- **+8 天**：OpenAI 开源 Codex Harness（Rust，`app-server` JSON-RPC，"反套壳"：把 Agent 引擎装进你自己的产品）。
- **市场叙事**：DeepSeek = Agent 的"安卓"（组装、广度、社区），Codex = "iOS"（嵌入、深度、企业）。原话结论："**未来的竞争在框架之上**"。
- **ARC-AGI-3 数据点**：仅调 harness（保留推理 + 上下文压缩），得分 13.3% → **38.3%**，输出 token **降 6 倍**。

### 3.2 未来 5–10 年的六个确定性方向 [推断]

**① 从"聊天"到"岗位"。** 交互对象从"一次对话"变成"一个持续在岗的职能"。主界面从聊天窗 → **调度台**（花名册 / 任务板 / 账本 / 审查队列）。

**② 瓶颈迁移：能力 → 注意力 → 信任。** 2024–26 是模型能力；2026–29 是**人的审查带宽**；2029+ 是**信任与责任归属**。→ "审查队列 + 证据化验收 + 审计导出"是**跨代际的长期资产**。

**③ 验证必须是可执行的，不是 LLM 裁判。** ACE 实测本地判官**假通过率 100%**；DSH 用 replay-safe 不变量、事件日志、git 快照；董事长铁律 7 也是同一句话（"拿一个可测量的差异证明它真的生效了"）。→ **"证据"= 可执行的检查结果，不是"模型说 OK"。**

**④ 协议收敛，引擎可换。** MCP（工具）、ACP（agent）、SDK JSON-RPC（嵌入）、hooks 桥（兼容）已成型。**DSH 自己证明了这条路**：同一份 runtime 被 Electron、CLI、ACP stdio、SDK JSON-RPC、webhook 五种载体驱动 **[实测]**。

**⑤ 经济重心从"token 单价"到"每任务通过成本"。** token 单价会持续降，但**失败重试、审查人时、不可逆错误**的成本不会等比降。→ 计量单位应是 **¥/已完成且已验证的任务**。ACE 的 "Verified Saving" 应升格为整个 OS 的北极星。

**⑥ 本地与合规从成本变卖点。** 本地模型（DSH 已内置本地 SenseVoice ONNX 语音）、本地优先数据、事件溯源审计。→ "数据不出本机 + 可审计"会从合规负担变成采购理由。
> ⚠️ **前提**：DSH 出厂默认把 transcript 塞进每次 API 请求（§1.9）。**任何"本地优先"叙事必须先把 `dsh-session-log-deepseek` 关掉并留证据。**

### 3.3 三个必须下注的不确定项 [推断]

| 赌注 | 我的下注 | 若错则如何 |
|---|---|---|
| Agent 的主界面是不是手机？ | **是"手机管、桌面干"** | 若手机端审批疲劳无解，退回"桌面为主 + IM 通知" |
| 小模型能否承担 loop 内决策？ | **不能**（ACE 已证短板在 L3 行动层） | 只在**分类/抽取平价**的判定任务上下沉（情感 100%、工具选择 91.7%） |
| 生态会不会被大厂收口？ | **不会完全收口**，但会形成 3–5 个事实标准 | 若收口，转做垂直 Playbook（内容/量化/工程） |

---

## 4. 互补性分析：谁缺什么

| 维度 | DeepSeek Harness | onecode-desktop（活） | 融合后谁提供 |
|---|---|---|---|
| 插件内核 / 能力组合 | ✅ 288 包、Cordis、preset 能力组合 | ❌ | DSH |
| 会话账本 / 审计 / fork | ✅ 事件溯源 + FTS5 + 4 版迁移 | ⚠️ SQLite **只存配置不存内容** | DSH |
| 上下文经济学 | ✅ token-meter / spill / 压缩 / PTC | ⚠️ **有原创的窗口推导与 autoCompact 预算**（`model_context.rs`）+ 真实 400 事故记录 | **DSH（机制）+ OneCode（楔子）** |
| 多 agent / 任务板 / 长自治 | ✅ Teams / goal / schedule / webhook（**但都标了硬上限与"原型"**） | ❌ | DSH（需补护栏） |
| 交互界面 | ✅ 桌面级 Web（diff 审查、交付卡、轨迹、右栏四面板） | ⚠️ 自研 UI，体验割裂（**产品自己的原话**） | DSH（桌面）+ OneCode（移动端知识） |
| **多槽 PTY / 崩溃存活 / 生成计数器** | ⚠️ 8 终端/会话，无多端重放、无驱逐；是"操作者自己的一个终端" | ✅ **原生多槽 + RingBuffer + generation counter** | **OneCode** |
| **容器 / 隔离 / 部署** | ❌ 明确"要用容器请自便" | ✅ Docker + `oc` + 安装器 UX | **OneCode** |
| **多 provider / 模型切换** | ✅ `llm` seam + 4 个 adapter | ✅ **已验收的 M1**（CRUD/预设/`/model`/F2） | 两边（DSH 更规范） |
| **上下文窗口推导 + 预算** | ⚠️ 有 `contextWindow` 与压缩阈值，**没有"窗口本身怎么来"的推导** | ✅ **`model_context.rs` + 事故文档** | **OneCode（→ 楔子，§6.5）** |
| **远程访问 / 反代 / 服务注册** | ❌ 拒绝 `0.0.0.0`、无多用户 | ⚠️ 有网关与反代，**但鉴权是假的**（§2.3） | OneCode（**必须先修**） |
| **多用户 / 多租户 / 登出** | ❌ 无概念 | ❌ 无 | **都要新建** |
| **成本地图 / 质量验证闭环** | ⚠️ 有数据（tokenUsage），无产品；**全仓无预算概念** | ❌ | **ACE + 新建** |
| **沙箱（进程级写约束）** | ✅ 五档后端 + 失败关闭 | ❌ 只有容器，容器内无约束 + `bypassPermissions` | DSH |
| **供应链安全（插件/Playbook）** | ❌ 安装跑 pnpm 构建脚本；装后 Host 代码在沙箱外 | ❌ `curl -k \| bash` + 5 处 `eval` | **都要新建** |
| 自扩展（Agent 固化新能力） | ✅ 只读内省 + `install_bundle` 持久化 | ❌ | DSH（需加签名与审批） |

**结论：没有一行实质重叠。** 这是**引擎 × 调度台 × 经济学**的三方拼图。

---

## 5. 新形态：OneCode OS

### 5.1 一句话

> **OneCode OS 是"一人公司的 AI 员工调度台"：它把任意引擎（DSH 原生，或 Claude Code / Codex / 任何会说 ACP 的 agent）装进隔离工作区里跑，用 Playbook 定义它们会什么，用 Slot/Session 账本记录它们做了什么，用 Team 任务板组织它们怎么协作，用证据化审查队列决定人要不要放行，用"已验证的每任务成本"告诉你这笔钱花得值不值。**

**核心定位：不做引擎，也不做更好的终端，做引擎与终端之上的调度台。** 这条定位**直接沿用产品自己已收敛的答案**（「一人公司的 AI 员工调度台」），只是把它的实现底座从"一个 PTY 里打字"换成"引擎 + 账本 + 治理"。

### 5.2 五层架构

```
┌─ L4 治理层 Governance ───────────────────────────────────────────┐
│  权限档 · 审查队列 · 预算硬闸 · 审计导出 · 合规包 · 成本账本(ACE)  │
├─ L3 交互层 调度台 ───────────────────────────────────────────────┤
│  Slot 花名册 · 任务板 · 会话账本 · 轨迹 · 交付物 · 两种终端        │
│  三表面：Pocket(手机) / Studio(桌面) / Fleet(团队)                │
├─ L2 能力层 Playbooks ────────────────────────────────────────────┤
│  bundle+preset+skills+policy+UI slots+eval，版本化、签名、可分发   │
│  注册表：本地 → 团队 → 市场                                       │
├─ L1 隔离层 Runtime ──────────────────────────────────────────────┤
│  容器/微VM 每工作区 · 多槽 PTY · 服务注册 · 密钥库 · 窗口推导     │
│  实现 DSH ctx.sandbox / ctx.subprocess / ctx.shell 的"容器后端"   │
├─ L0 引擎层 Engine (可换) ────────────────────────────────────────┤
│  DSH 原生(首选) · Claude Code · Codex CLI · OpenCode · 任何 ACP   │
└──────────────────────────────────────────────────────────────────┘
```

**关键架构决定：L0 与 L1 之间用协议，不用代码耦合。** DSH 走进程内 + ACP/SDK；异种 CLI 走 PTY。两者在 L3 呈现为同一种对象："一个 Slot"。

> **为什么 L0 能换是"操作系统"名分的唯一证明**：DSH 自己已证明"一个 runtime，多个载体"可行。我们要做的是把"载体"换成"任意引擎"。

### 5.3 三个表面

| 表面 | 场景 | 主交互 | 来源 |
|---|---|---|---|
| 🌱 **Pocket** | 人不在场。Agent 在跑，需要拍板 | **审查队列**：一屏一张卡（做了什么 / 证据 / 成本 / 三个按钮：批准·质询·接管）；语音 | OneCode 移动端 WKWebView/IME 知识 + DSH 交付卡/审批 seam + 本地 SenseVoice |
| 🖥 **Studio** | 人在场，深工作 | Slot 花名册 + 会话账本 + diff 审查 tab + Playbook 编辑器 + 两种终端 | DSH Web GUI（`ui-deliverables`/`ui-trajectory`/`ui-sidebar-*`）+ desktop 的原生槽/托盘/健康循环 |
| 🏢 **Fleet** | 团队/组织 | 多成员花名册 · 任务板 · 预算看板 · 审计导出 | DSH `agentTeam` 投影 + 新建多租户 |

**设计准则：raw mode 是逃生舱，不是主界面。** 终端保留（异种 CLI 需要），但默认视图是**结构化账本** —— 因为终端字节流无法审计、无法 diff、无法计量。

### 5.4 五个"单元"（形态的定义）

| 单元 | 定义 | 由谁提供 | 为什么是它 |
|---|---|---|---|
| **软件单元 = Playbook** | 版本化能力包：`bundle + preset + skills + policy + UI slots + eval` | DSH 组合机制 + 新建格式/注册表 | 引擎免费后，**"选哪些插件、怎么配、验收标准是什么"才是手艺** |
| **工作单元 = Slot（= 一个 Session）** | 命名 agent + 项目 + provider + 权限姿态 + durable transcript | **OneCode desktop 已有 Slot 骨架** + DSH 会话账本 | 产品自己已把定位收敛到调度台，**而 Slot 就是"AI 员工"的物理形态**（§6.1） |
| **组织单元 = Team** | roster + mailbox + 任务板（CAS 版本 + 依赖 DAG + write scope 告警） | DSH `agent-team`（**需从"原型"补到产品级**） | 一人公司的组织形态天然就是"Lead + 若干 teammate" |
| **信任单元 = Review Item** | 每个不可逆动作 → 一条带**可执行证据**的待审项 | **新建**（吸收 DSH approval + ACE 验证） | 人的注意力最稀缺，必须按"证据"批量放行 |
| **经济单元 = ¥/已验证任务** | 每任务成本 × 通过率，而非 token 单价 | **ACE**（DSH `tokenUsage` 直供） | ACE 已证：路由省钱 <2%，杠杆在上下文与次数 |

### 5.5 与今天的关系

| | onecode（web，停更） | onecode-desktop（活） | OneCode OS |
|---|---|---|---|
| 用户操作的物体 | 一个终端 | 一个终端（多槽/多 provider） | 一个 **Slot 花名册** |
| 持久的东西 | PTY 环形缓冲 | SQLite（**仅配置**） | **事件溯源会话账本** |
| "角色"是什么 | 提示词宏（**且不派发**） | 同（侧栏列表） | **能力组合（Playbook）** |
| 引擎 | 只有 Claude Code | 多 provider（都是 Claude 系） | **任意（DSH 原生 / 异种 CLI / ACP）** |
| 隔离 | Docker + `bypassPermissions` | 同 + CSP `unsafe-eval` | 容器 + 沙箱档位 + 审批（**默认 `read-only`、失败关闭**） |
| 审查 | 无 | 无 | **证据化审查队列 + 可执行验收** |
| 成本可见性 | 无 | statusline 有上下文/成本条 | **¥/任务、命中/未命中输入、子代理占比、Verified Saving** |
| 扩展 | 改 HTML/JS | 改前端 | **装 Playbook（签名、权限声明、可分发）** |
| 数据出境 | 无感知 | 无感知 | **可审计、默认不出境** |

---

## 6. 必须新建的机制

### 6.1 Slot 即工作单元：把"角色"从文本宏变成真正的派发

**问题（实测）**：`@mention` 只把 `'@dev '` 写进那**一个** PTY。没有路由、没有 prompt 隔离、没有会话分离、没有权限差异。所以"10 个 agent 协作"在机制层是假的。

**修复路径（这是最便宜的杠杆）**：desktop 已经有 `SlotConfig`（env 快照）、每槽状态、`pty_spawn`、托盘、健康循环。**让 `@architect` 触发一次"以该 agent 为身份的新 Slot"** —— 自己的槽、自己的 system prompt、自己的 transcript、自己的 provider/权限姿态。**这是一个 `pty_spawn` 参数变更，不是重写。** 这一步把"终端"变成"调度台"。

**同时把"终端"拆成两种**（DSH 已给出正确分野 **[实测]**）：

| | Human Terminal（逃生舱） | Agent Terminal / Slot（员工的工作台） |
|---|---|---|
| 权限 | 系统用户，**不受沙箱/审批限制** | 容器沙箱档位 + 审批策略 |
| 进账本 | **不进**（只记录"人开过一个终端"这一事实） | **全进**（`tool/call` + `tool/result`） |
| 用途 | "agent 的沙箱挡路了，我自己来" | 异种 CLI 引擎的宿主 + 原生引擎的会话容器 |
| 多端重放 | **需要**（手机看、桌面看同一个）→ **用 web 端 `pty.js` 的工程** | 不需要（用 desktop 的 Rust 多槽） |

**配套改造**：一个容器 = 一个 Workspace；一个 Workspace 承载 N 个 Slot；手机与桌面 attach 的是 **Slot**，不是终端；`pty.js` 的 UTF-8 边界守卫与驱逐策略**回灌进 `slot.rs`**（现在它丢了这个守卫，§0.6 #9）。

**验收判据（可执行）**：`@architect` 派发后，两个 Slot 的 transcript 互不可见、system prompt 不同、权限档不同；Human Terminal 的重放字节与原会话 md5 一致；Agent Terminal 的每次命令在账本里可查。

### 6.2 Playbook：可安装、可签名、可卖的能力包

```yaml
# playbook.yml
id: content.wechat-publisher
version: 1.3.0
engine: dsh                       # dsh | claude-code | codex | acp
preset:                           # ← 直接落成 DSH preset 的能力组合
  persona: "你是公众号主编，只输出可发布成品。"
  tools: [fs, bash, web, present, skill]
  skills: [wechat-formatting, image-qc]
  delegation: { subagents: true, maxChildren: 4, budgetCredits: 30 }
policy:
  sandbox: workspace-write
  approval: irreversible-only
  egress: deny-default            # ← 默认不出境；DSH 的 session-log 必须显式关掉
  budget: { credits: 200, hardStop: true }
ui:
  slots: [conversation.chat.turnTail]     # ← 直接挂 DSH 已观测到的 slot
  deliverableKinds: [wechat-html, cover-image]
eval:                              # ← 没有 eval 的 Playbook 就是提示词
  - name: 排版通过
    check: { kind: exec, cmd: "node tools/check-wechat-layout.js out.html" }
    must: pass
  - name: 图片尺寸合规
    check: { kind: exec, cmd: "node tools/check-cover.js cover.png" }
    must: pass
```

**五条纪律**：
1. **无 `eval` 的 Playbook 不许进注册表。** 把"证据化验收"变成入口门槛。
2. **必须声明 `policy`**（沙箱档 + 审批策略 + 出境策略 + 预算）。安装时展示"这个包会要什么权限"。
3. **必须声明 `egress`，且默认 `deny`**（理由见 §1.9）。
4. **签名与来源**：v0 做 checksum + 来源标记；市场阶段做签名与付费。**安装会跑第三方构建脚本**，所以"安装"本身要进审查队列。
5. **技能目录落在 `customSkillDirs`（DSH 根优先级 300）**，让 Playbook 自带技能优先于 `$DSH_HOME/skills`（400）而不污染内置（600）。

> **为什么这是最有价值的单元**：它是**唯一能把"手艺"变成可复制、可定价、可分发商品的东西**。ACE 修正版结论（"路由网关不应继续投入；评估闭环是最值得验证的商业假设"）在这里落到了具体形态 —— **Playbook 的 `eval` 就是评估闭环的产品形态。**

### 6.3 Review Queue：用证据换人的注意力

**规则**：
- 每个**不可逆**动作（写工作区外、发外部请求、付钱、发布、删数据、**装 Playbook**）生成一条 Review Item。
- 每条 Item 必须带**可执行证据**：diff / 测试结果 / 校验和 / 成本 / 截图。
- **可逆 + 证据全绿 → 自动放行**；不可逆或证据缺失 → 进队列。
- 支持**批量批准同类项**，但**不同类不合并**。
- 拒绝给**结构化理由**并回灌给 agent（DSH `tools/post-execute` 支持 block with feedback）。

**从 DSH 实测里学到的三条硬约束**：
1. **DSH 的审批没有记忆**（只有 `allowed-once`）。我们的队列必须**有记忆但按风险分层**，否则退化成"每次都问"= 批准疲劳。
2. **委派子代理的审批被钉成 `never`** → Playbook 一旦开了子代理，**子代理做的不可逆动作不会问人**。必须在策略层堵死：**子代理的不可逆动作上浮到 Lead 的审查队列**。
3. **plan mode 没有牙** → **不能把"计划已批准"当安全边界**。强制必须落在沙箱 + 审批。

**反模式**：给每个动作弹窗；把"模型说 OK"当证据；让 agent 自己批准自己的不可逆动作。

### 6.4 Cost/Outcome Ledger：把 ACE 装进内核（DSH 最大的空位）

**最强的事实支撑**：**全仓 grep `tokenBudget` / `budgetCredits` = 0 命中**；子代理"slots 不约束累计历史或 token 用量"；workflow 上限是"cooperative… 不是 host-enforced 配额" **[实测]**。
→ **DSH 有全世界最好的上下文计量，却没有任何预算护栏。** 对无人值守跑业务的系统，**无上限、无追踪的模型花费是最可能的失败模式** —— 而我们恰好有 ACE。

**数据来源（零额外埋点）**：DSH 的 `tokenMeter` 投影已提供四个成本计数器 + `contextBreakdown` **[实测]**。

> ⚠️ **计量的坑**：启发式是 **4 字符/token**，**CJK 与 JSON schema 被严重低估**，`contextBreakdown` **不是计费数据**。规则：**定价与硬闸用 provider 回报的用量；启发式只用于"该不该压缩"的决策。**（对中文主场景尤其重要。）

**要展示的四个数**：
1. **未命中输入占比**（ACE 实测 49.3%，最大头）
2. **子代理成本占比**（ACE 实测：**50.1% 成本 / 40% 调用量**）
3. **¥ / 已完成任务** 与 **¥ / 已验证通过任务**（北极星）
4. **Verified Saving**（用了上下文机制后省下的、且质量未退化的钱）

**要做的动作**：
- **预算是硬闸**：超限 → 中止并要求人加额。实现在 `tools/pre-execute` 的守卫里 —— DSH 的 guard 是**单调的**（返回拒绝后后续 listener 无法翻回允许），**正好适合做硬闸**。
- **按数据推荐默认值**：把 DSH 的 `spill-policy`/`compaction`/PTC 从"配置项"变成"默认就开"，因为 ACE 已证"上下文瘦身 ≈25% + 调用次数 ≈20%"是唯一大杠杆。
- **峰谷调度**（ACE 实测 5–10%）：非紧急任务排到低价时段（需自建持久调度，见 §9.1）。

### 6.5 上下文预算楔子：把 `model_context.rs` 一般化（本设计认定的差异化入口）

**为什么是它**：`onecode-desktop` 的 `model_context.rs` + `docs/incident-20260923-context-overflow.md` 是**全仓最有原创性、也最被低估的产物**。它从一个**真实生产事故**（两次 400）反推出：用户输入 → 模型后缀 → 有出处的表项 → 保守回落；阈值 = 窗口 − 32k 补全 − 25% 突发 − 5% 余量。

**为什么它是好生意**：
- **每个多 provider 的 agent 宿主都有这个问题** —— 窗口大小不确定；
- **错了不会立刻报错**：要么 400（浪费一轮），要么过度压缩（浪费钱），**是一个隐形失败**；
- **修法是一张"没人愿意维护的表"**（事件文档自己承认：表里只有 2 条，其余回落 128K，本地 Ollama 被算错）；
- **它与 DSH 的机制天然互补**：DSH 有 `contextWindow`、压缩阈值、`tokenUsage` 计量，**但没有"窗口本身从哪来"的推导**；OneCode 有推导，**但没有账本与机制**。

**产品化三件事**：
1. 在 provider 新增/编辑时**自动探测**窗口（事故文档自己点名的 Ollama 修法）。
2. **社区窗口表**（可提交、可验证、带出处），像 DriverDB 一样累积。
3. **可见的"未识别模型（保守估计）"标记** —— 不假装知道，而是把不确定性摊给用户。

> 这一条不性感、不显眼，但**可防守、会复利**。而且它给我们的账本提供了 DSH 缺的那半边：**没有正确的窗口，就没有正确的"该不该压缩"决策，也就没有可信的 Verified Saving。**

### 6.6 引擎适配层（薄，且必须薄）

**为什么薄**：DSH 是 **rc**（0.2.0-rc.2），且**已知扩展机制都带锐边** —— patch 整块替换、未知 id 只警告跳过、空 patch 文件启动失败、替换已装包必须重启、HMR 失败策略明确非事务性。

**做法**：锁版本 + 契约测试（拿 `dsh --profile <p> --dump-config` 做快照比对 —— 它与启动共用同一个 patch 函数，所以快照可信）；不自研 fork；把 DSH 特定知识集中在少数几个文件里。

---

## 7. 资产映射：留什么、改什么、弃什么

| 现有资产 | 路径 | 新形态角色 | 处置 |
|---|---|---|---|
| **原生多槽 PTY + generation counter** | `onecode-desktop/src-tauri/src/pty/{mod,slot}.rs` | **L1 核心（Slot 的物理载体）** | **保留为核心；把 `pty.js` 的 UTF-8 边界守卫回灌 `slot.rs`** |
| **web 端 PTY：环形缓冲 + UTF-8 安全裁剪 + 多端重放 + 慢客户端驱逐** | `onecode/agent-runtime/gateway/pty.js` | L1 的 **Human Terminal** 通道 | **保留并抽成独立包**（web 腿最值钱的零件） |
| 二进制分帧协议 + 客户端半 | `pty.js:25-51`、`term-ws.js`、`static/term.js` | 版本无关的协议契约 | 保留 |
| **多 provider 子系统 + `/model` + F2 + 连接测试** | `providers.rs`、`src/provider-switch.js` | L1 路由 | **保留**（修明文 key + curl argv + 两个 M1 P1） |
| **`model_context.rs` + 事故文档** | `src-tauri/src/model_context.rs`、`docs/incident-20260923-context-overflow.md` | **L4 的"窗口推导"服务；本设计的楔子** | **保留 + 一般化（§6.5）** |
| 会话存储 + 启动恢复 | `src-tauri/src/session/`、`commands.rs` | L1 持久层骨架 | **保留，扩展到存内容**（原本的 P1-1） |
| 健康循环 / 托盘 / 原生菜单 / keep-awake | `pty/health.rs`、`tray/`、`menu/`、`keep_awake.rs` | Studio 的常驻能力 | 保留（健康循环要真自愈，不只是 warn） |
| 容器生命周期 CLI + 健康门控启动 | `onecode/agent-runtime/bin/oc:540-583,983-1101` | L1 容器编排 | **重构**（修注入/引用/`pkill`） |
| 安装器 UX + `github_fetch` 降级链 | `bin/install.sh:143-197,199-212,820-880` | L1 分发 | **重构**：保 UX，**删 `-k` 档与 5 处 `eval`，补 SHA-256** |
| **code-server 集成 + 补丁台账** | `proxy.js:79-158`、`inject.js`、`setup-code-server.sh`、`CODE-SERVER-PATCHES.md` | Studio 的 IDE（可选 Playbook） | **保留**；台账当"实践"复用到所有上游补丁 |
| 角色加载器 + `cc-status` + 无依赖 `cronNext` | `gateway/cc-status.js:8-129,286-336` | L2 Playbook 的**降级兼容输入** | 保留；**`:286-336` 的角色加载要接到真正的派发（§6.1）** |
| statusline（上下文/成本/git）+ `flock` cron 追踪 | `agent-runtime/cc-statusline-command/*` | Studio 的状态条 + 调度记录 | **逐字保留**（小而可移植，现在就有用） |
| 移动端 UI：4-tab、虚拟键盘、**CJK IME 修复**、惯性滚动 | `onecode.html`、`term.js:127-425,930-1030` | **Pocket 表面** | **保留**（WKWebView/IME 是硬碰出来的知识） |
| 零依赖测试运行器 + 测试指南 | `onecode/tests/run.js`、`tests/README.md` | 测试模板 | 保留为模板 |
| `@mention` 弹窗 | `onecode.html:431-466`、`src/terminal/mention.js` | — | **重写为派发**（§6.1） |
| **web 网关胶水**（单 PTY/单租户写死） | `gateway/{index,router,static}.js` | — | **重写**（或降级为零件来源） |
| `onecode-cli` | `onecode-cli/CLAUDE.md` | — | **保留那 20 行规格，删掉"这个仓存在"的错觉** |
| 死的 `auth()/X-Auth` 代码 | `onecode.html:547-550` | — | **删**（它暗示了一个不存在的安全） |
| Vendor 的 Monaco / VS Code 皮肤（14MB / 111 文件） | `desktop/src/static/vs/**` | — | **删**（没有已交付的表面） |
| ACE | `eng/ace/` | **L4 成本账本 + 审查证据引擎** | **集成**（`tokenUsage` 直供；`eval` 闭环复用其 replay 工具链） |

---

## 8. 路线图

> 每阶段必须有**可执行判据**（延续董事长铁律 7）。

### Phase 0 · 「止血 + 让角色真的派发」（现在 → 3 个月）

**做（顺序不可调）**：
1. **断安全链**：容器启动**生成 token 并强制校验**；**`docker.sock` 改为显式 opt-in**（它把"网页 bug"变成"宿主 root"）；`/ws/term` 加 **Origin 校验**；`/api/restart-terminal` 加 CSRF；去掉 `bypassPermissions` 默认，改权限档。
2. **修供应链与注入**：**从 README 与 `install.sh` 删掉 `curl -k | bash`**，并**发布 `install.sh.sha256`**（否则那个校验分支是死的）；把 5 处 `eval` 改成间接展开；密钥不再走 curl argv / docker `-e`。
3. **关掉 DSH 的默认出境**：`session-log-deepseek` 关闭并留证。
4. **让 `@mention` 派发**：`@architect` → 新 Slot（自己的槽 / system prompt / transcript / provider / 权限档）。**这是最便宜的杠杆。**
5. **拆两种终端**；把 `pty.js` 的 UTF-8 守卫回灌 `slot.rs`。
6. **接 DSH 账本 + 成本条**：先只取**会话账本 + `tokenUsage`** 两项；会话头部显示 未命中输入 / 命中 / 输出 / ¥。

**判据**：
- `curl` 到 7681 在无 token 时返回 **401**；从任意网页发起的 `ws://localhost:7681/ws/term` **被拒**。
- 某次真实请求的日志证明**没有 transcript 出境**。
- `@architect` 派发后两个 Slot 的 transcript **互不可见**、system prompt 不同。
- 一次 30 分钟编码会话的分项成本与 `ace` 工具扫描误差 **<5%**。
- **测试可信**：`npm test` 进 CI，**失败就是失败**（不允许静默降级为 grep）；版本号四处一致。
- **不回归**：Docker 一键装仍是一条命令。

### Phase 1 · 「Playbook + Review Queue + 窗口表」（3 → 9 个月）

**做**：Playbook v0 格式 + 本地注册表；preset 生成器（Playbook → DSH YAML）；eval runner（`kind: exec` 优先，**不做 LLM 裁判**）；Review Queue（**Pocket 先做**，手机最适合审批）；ACE 接入账本；**预算硬闸**；**`model_context.rs` 一般化 + 社区窗口表 v0 + "未识别模型（保守）"标记**。

**判据**：
- 现有 `.claude/agents/*.md` 角色**全部**转成 Playbook，每个都有 ≥2 条可执行 eval。
- 连续用 7 天、跑 ≥3 个 Playbook，**因审查队列放行的不可逆动作数 > 0**。
- 无 eval 的 Playbook 进注册表**被拒**。
- **预算硬闸被真的触发过一次**（跑到上限被中止），账本有记录。
- **窗口表至少覆盖 5 个模型且每个都有出处**；一个不在表里的模型会显示"保守估计"而不是假装知道。

### Phase 2 · 「Fleet」（9 → 18 个月）

**做**：多用户/多租户（Workspace 隔离 + 身份 + 配额 + **登出**）；Team 视图（复用 DSH `agentTeam` 投影，但补齐它的短板：**ownership 自动释放**、跨进程 mailbox，或自建）；**持久化调度与作业**（替换 DSH 的内存 jobs 与单 Host 定时器）；审计导出；**会话存储的规模改造**（DSH 的冷扫描代价 ∝ 会话数，见 §9.1）。

**判据**：
- 5 人共用一台宿主，A 的 agent 无法读 B 的 Workspace（**越权测试留证**）。
- 一次"人睡觉时"的任务链：外部触发 → Slot → 审查队列 → 早上手机 3 次点击验收，**全过程有事件日志可回放**。
- **杀掉进程再拉起**：调度与作业记录**不丢**（DSH 原版会全丢）。
- 审计包能回答："这条对外发布是谁批准的、依据什么证据、花了多少钱"。
- **规模测试**：1,000 个会话下，冷启动列举耗时在可接受范围（否则先做索引/分区）。

### Phase 3 · 「Marketplace」（18 → 36 个月）

**做**：Playbook 签名与版本约束；第三方作者分成；垂直包（内容 / 量化 / 工程，直接对接 `content/` `quant/` `eng/` 的真实产出）；本地模型档（**只下沉 ACE 测出平价的判定任务**：情感 100%、工具选择 91.7%；**不碰**意图六分类与 agent loop 决策）。

**判据**：
- 第三方作者从市场拿到的钱 **> 我们自研 Playbook 的钱**。
- 装机 30 天内安装 ≥1 个第三方 Playbook 的用户占比 **> 40%**。
- 本地模型档在声明任务上相对 frontier 基线 **≥90%** 且 `Verified Saving` 为正。
- **签名绕过尝试被拒**（红队测试留证）。

### Phase 4 · 「Personal / Firm Agent OS」（3 → 10 年）

**做**：Agent 身份与授权契约；可移植工作区；agent 间委派与结算；合规包（本地优先 + 事件溯源审计 + 人工监督留痕）。

**判据**：**换掉 L0 引擎后，用户的 Playbook / 账本 / 审计 / 审查历史零迁移成本。** 这是"操作系统"名分的唯一证明。

---

## 9. 风险与非目标

### 9.1 风险（含缓解）

| 风险 | 严重度 | 缓解 |
|---|---|---|
| **OneCode 的安全链：未鉴权浏览器 → `/ws/term` → PTY → `docker.sock`(chmod 666) → 宿主 root** | 🔴 **最高** | Phase 0 第一步；`docker.sock` 改 opt-in；加 Origin/CSRF；上 token |
| **DSH 默认把 transcript 出境**（`enabled: true` / 8MiB，redaction 出厂为空） | 🔴 | Phase 0 关闭 + 留证；Playbook 必须声明 `egress` |
| **插件/Playbook 供应链 + 提权**（装后 Host 代码**在沙箱外**；安装跑第三方构建脚本） | 🔴 | 签名 + 权限声明；**"安装 Playbook"本身入审查队列**；`approvedBuilds` 必须人批 |
| **委派子代理的审批被钉成 `never`** | 🔴 | 策略层强制：子代理的不可逆动作**上浮到 Lead 队列**；不要依赖 DSH 默认 |
| **OneCode 自身的供应链与注入**（`curl -k \| bash` 被写成用户选项；5 处 `eval`；`pkill` 空端口杀全部 socat；密钥进 argv） | 🔴 | Phase 0 第二步逐条修；补 `install.sh.sha256` |
| **DSH 是 rc，扩展机制多锐边**（patch 整块替换 / 未知 id 只警告 / 换包要重启 / HMR 非事务） | 🟠 | 适配层**薄**；锁版本 + `--dump-config` 契约快照；不自研 fork |
| **DSH 自治底座不耐久**：`jobs` 纯内存、`schedule` 单 Host 定时器且关 app 不触发、workflow 无 journaling、webhook 无队列/去重 | 🟠 | Phase 2 自建持久调度/作业层；**补齐前不要把"夜间无人自治"当已交付** |
| **DSH 的 Team 是原型**（单进程、单 checkout、write scope 仅告警、无 ownership 自动释放、非跨进程 exactly-once） | 🟠 | 补到产品级或自建；**明确不承诺跨进程协作** |
| **DSH 会话存储的冷扫描代价 ∝ 会话数**（自认） | 🟠 | Phase 2 加规模测试与索引/分区 |
| **测试不可信**（web 宣称 277/0 实为 302/6；desktop 的 JS 测试测手写副本并把缺陷写成绿断言；`pty.test.js` 静默降级） | 🟠 | Phase 0 把 `npm test` 进 CI；删掉"静默降级"；把缺陷从绿断言里摘出来 |
| **直接解析 Claude Code 的转录是"未受保护的内部格式"**：官方明确说 `~/.claude/projects/*.jsonl` 的行格式属内部实现、**任何版本都可能变**；受支持的读法只有 `/export`、SDK、hook 的 `transcript_path`——而 `cc_sessions.rs` 正在直接解析它 **[研究]** | 🟠 | **账本不以它为准**：当作"尽力而为的导入源"；主账本用我们自己的事件日志；解析失败必须降级而不是崩 |
| **批准疲劳**（手机端审批没人理） | 🟠 | 只对不可逆动作入队；同类批量；按风险分层；用 ACE 数据判断"这条值不值得问人" |
| **生态位挤压**（Cursor / Claude Code / OpenAI 都往控制台走） | 🟠 | 差异化只押三件：**异种引擎共存**、**证据化审查**、**上下文预算 + 成本账本**。不做通用 chat、不做补全体验 |
| **ACE 的 No-Go 结论被忽视**（路由不该继续投入） | 🟡 | 明确：ACE 在本设计中**只做账本与证据**，不做网关/路由卖点 |

### 9.2 必须警惕的"默认值陷阱"（部署清单）

| 默认值 | 风险 | 处置 |
|---|---|---|
| onecode `TERM_TOKEN = ''` + `0.0.0.0` + 无 Origin | 无鉴权 shell | **强制 token + loopback + Origin** |
| onecode 挂 `/var/run/docker.sock` 并 `chmod 666` | 宿主 root | **改 opt-in** |
| onecode `bypassPermissions` | 无审批面 | 改权限档 |
| DSH `session-log-deepseek.enabled = true`，8MiB | transcript 出境 | **关**，留证 |
| DSH `approval/policy = 'ask'` + 只有 `allowed-once` | 批准疲劳 | 队列加风险分层记忆 |
| DSH 子代理 `approval = 'never'` | 子代理绕过审批 | 上浮到 Lead 队列 |
| DSH `maxDepth = 1`、`maxActiveSubagents = 8` | 大 fan-out **拒绝**不排队 | 显式规划并发 |
| DSH `sandbox` 默认 `read-only` | 正常写工作区被拒 | Playbook 显式声明 `workspace-write` |
| DSH Windows 沙箱 `partial`（读不受限） | 读侧无隔离 | 平台能力声明里降级说明 |
| DSH `fs-sandbox` 非内核边界（接受 TOCTOU） | 不是强隔离 | **强隔离靠容器** ← 我们的活 |
| DSH `jobs` 内存态 / `schedule` 单定时器 | 进程退出即丢 | 自建持久层 |
| DSH 无任何 token 预算 | 无上限花费 | **预算硬闸（§6.4）** |

### 9.3 非目标（明确不做）

1. **不做第 X 个 harness / 引擎。** 只做适配层。
2. **不做模型训练 / 对齐 / 微调。**（`models/` 线）
3. **不与 Cursor / Claude Code 在"代码补全与编辑体验"上竞争。** 也**不与 iTerm2 在"多 tab 终端"上竞争** —— 产品自己的审视文档已判定那不是购买理由。
4. **不做纯云 SaaS 托管用户数据与 API Key。**（延续 ACE 已拍板：本地 Runtime + localhost 为主）
5. **不用 LLM 当验收裁判。**（ACE 实测假通过率 100%）
6. **不让终端重新成为主界面。** raw mode 是逃生舱。
7. **不在 Phase 2 之前宣称"无人值守"。** DSH 的调度/作业/工作流都不耐久。
8. **不再同时维护 web 端与 desktop 端两套并行实现。**（这是本组合最大的浪费）

### 9.4 停止条件（Kill Criteria）

- **Phase 0 结束**：若 `@mention` 派发上线后，**自己连续 3 天仍只用 Human Terminal** → Slot 假设不成立，回退做"更好的单终端"，不要硬推花名册。
- **Phase 1 结束**：若 7 天自用中**审查队列放行次数 = 0** → **交互层假设错误**，回退定位。
- **Phase 1 结束**：若预算硬闸**从未触发** → 先查计量，不要急着做账本 UI。
- **Phase 1 结束**：若窗口表的**社区提交数为 0** → 楔子假设错误，转回机制层（DSH 的压缩/账本）。
- **Phase 2 结束**：若 5 人团队**无付费意愿**（按 ACE 判据："客户做完第一次评估后主动问能不能持续监控"）→ 转做**垂直 Playbook 自营**，放弃平台化。
- **任意阶段**：若 DSH 上游补齐"容器 + 多端 + 多租户 + 持久调度"→ 立即把 L1/L2 换成薄适配，资源全移到 **L4 账本与审查**。

---

## 10. 一页速查

**判断链**：
```
引擎免费了（MIT, 288 包, 社区 8 天 2600+ 插件）→ 框架不是护城河 → 竞争在框架之上
  → 价值迁移到：分发运维 / 策展评估 / 人的审查带宽 / 经济控制
  → DSH 缺分发运维（自己说"用容器请自便"，且拒绝 0.0.0.0、无多租户）
  → OneCode 缺账本与治理（web 端只有一个 10MB 环形缓冲；desktop 只存配置不存内容）
  → OneCode 的 @mention 是文本宏，不是派发 ← 最便宜的杠杆
  → ACE 已证钱在上下文与次数（未命中 49.3%，路由 <2%，本地判官假通过 100%）
  → DSH 的 tokenUsage 恰好是 ACE 要的四个计数器
  → DSH 有最好的计量、却全仓没有预算概念（grep = 0）← 这是我们的位置
  → 而 desktop 有 model_context.rs（窗口推导），DSH 没有 ← 这是我们的差异化楔子
  → 三者拼成一个东西：一人公司的 AI 员工调度台
```

**形态一句话**：三个表面（Pocket/Studio/Fleet）× 五个单元（Playbook / Slot-Session / Team / ReviewItem / ¥-per-verified-task）。

**最先做的三件事**：
1. **断安全链**（token + `docker.sock` opt-in + Origin），并关掉 DSH 的 transcript 默认出境。
2. **让 `@mention` 真的派发**（`pty_spawn` 参数变更），把终端拆成两种。
3. **接 DSH 账本 + 成本条 + 窗口表**（先看见钱，先知道窗口）。

**永远不要忘**：
> **证据要可执行，不要模型裁判。**
> **终端是逃生舱，不是主界面。**
> **引擎可以换，账本不能丢。**
> **没有预算的自治不是能力，是账单。**
> **接缝才是产品 —— 这句话董事长已经说过，本设计只是把它落到代码上。**

---

## 附录 A · DSH 关键机制速查（含出处）

| 机制 | 包 | 出处 / 关键值 |
|---|---|---|
| 插件模型（fiber DI） | `cordis` 4.0.4 | `Plugin.Function/Constructor/Object`、`inject`、六态 fiber、`ctx.effect` |
| profile/bundle/patch 优先级 | `dsh` + `dsh-app-boot` | `readProfilePatches` `lib/index.js:1023-1031` |
| patch 语义（整块替换 / 未知 id 警告跳过） | `cordis-plugin-include` | `applyEntryPatches` `src/index.ts:57-127` |
| 共享内核 94 行组合 | `dsh-base` | `cordis.patch.yml`（529 行含理由）；**刻意不挂 invariants** |
| 四种 preset（能力组合） | `dsh-web-app` | `presets/{standard,minimal,ptc,cordis}.patch.yml` |
| 会话事件溯源 + 4 版迁移 | `dsh-session` | `dsh-session-format-v0-to-v1…v4`；**58 条冻结日志事件词表**；JSONL+zstd 帧；迁移非破坏、build-static 查缺口 |
| 事件命名空间 | — | **Cordis 事件 ≠ 会话日志事件**（日志侧 58 条冻结，词表外需 `ignorable: true`） |
| 投影 / FTS5 / 缓存 | `dsh-session-{projection,query-sqlite,projection-cache,turn-outline,stats}` | CQRS 读模型；FTS5 无 bm25、手写排序 |
| 工具管线 + 三呈现模式 | `dsh-tools` | `native/ptc/both`；`maxParallelSubCalls: 10`；pre-execute → **单调 guard** → execute → post-execute → finalize → result |
| PTC（程序化工具调用） | `dsh-ptc-runtime-node` | `run_code` 120s/600s；**VM 非安全边界，网络不受文件策略限制** |
| workflow 编排 | `dsh-tool-workflow` / `dsh-workflow-ptc` | `agent/pipeline/parallel/phase/log/args`；1000 agents / 4096 items；**无 journaling/resume/预算** |
| Ralph 全新 agent 循环 | `dsh-tool-ralph` | `maxRounds: 256`；`maxHandoffChars: 16384`；**默认关闭** |
| 子代理 | `dsh-subagent` | `maxDepth` **1**、`maxActiveSubagents` **8**（拒绝不排队）；**子代理 approval 钉 `never`**；**无 token 预算** |
| Agent Teams | `dsh-experimental-agent-team` | 16 成员 / 256 任务 / 64 消息 / 64KiB；CAS `expectedRevision`；writeScope 仅告警；**自述原型** |
| Goal 与轮次驱动 | `dsh-goal` / `dsh-goal-round-driver` | `maxGoalRounds` 256；`blocked` 需连续 3 轮；**会话启动边即 disarmed** |
| Schedule | `dsh-schedule` | 一条记录 + **一个 Host 定时器**（非 cron）；错过只补最近；**关 app 不触发** |
| Webhook | `dsh-webhook`(+github) | HMAC 在 JSON 解析前；**无队列/重放/去重**；`permissionPreset` 由规则自选 |
| Jobs | `dsh-jobs-local` | **纯内存**，进程退出即丢；`maxConcurrentJobsPerOwner 10` |
| 审批 seam | `dsh-user-approval` | 只有 `allowed-once`；`ask`(默认)/`never`；失败关闭；需开着的轮次 |
| 权限预设 | `dsh-permission-presets` | `workspace-write→ask`、`danger-full-access→never`；无沙箱 shell 上挂载直接拒载 |
| plan mode | `dsh-plan-mode` | **无强制力**（"every tool stays callable"），纯 UX |
| auto-review | `dsh-experimental-auto-review` | 逐调用 LLM 授权，用当前 agent 自己的模型；**默认关**；自述不安全 |
| 沙箱 | `dsh-sandbox` / `-local` / `-policy` | 三档；bwrap/landlock/seatbelt=full，windows-acl=**partial**；**失败关闭** |
| fs 沙箱 | `dsh-fs-sandbox` | **非内核边界**，接受 TOCTOU |
| 计量 | `dsh-token-meter` | 四个成本计数器 + `contextPressure` + `contextBreakdown`；启发式 **4 字符/token**，**官方承认 CJK 低估** |
| 免费优先的裁剪 | `dsh-output-retention` / `dsh-compaction-tool-result-pruner` | 按字节 head/tail（不切码点）；`8192 → 4096 + 标记 + 1024`；无模型调用；**包描述原文 "Replay-safe"** |
| 付费压缩 | `dsh-compaction-basic` / `-image-offload` | `0.8` / `headroom 65536` / `retain 0.16`；**node 0 永不被遮蔽** |
| 溢出与回收 | `dsh-spill-policy` / `dsh-spill-local` | `maxInlineTokens 12500`；给真实文件路径；失败保留原文 |
| 重试与超时 | `dsh-llm-retry` / `dsh-tool-call-timeout-policy` | 默认 5 次退避 `500ms→10s`+10% 抖动，**先落盘再等待**；**超时无全局默认** |
| 落盘闸 | `dsh-session-checkpoint-policy` | **三个 fail-closed 闸**：适配器建流前 / 顶层工具体前 / 每 pre-step |
| **无预算** | — | 全仓 grep `tokenBudget`/`budgetCredits` = **0** |
| 只读内省 | `dsh-tool-cordis` | `cordis_inspect_list`/`_query`；**不能调服务方法、不能改运行时** |
| 持久扩展 | `dsh-plugin-manager` | `plugin_manager install_bundle`；跑 pnpm；快照/恢复 manifest+lock；`approvedBuilds` 人批；**装后 Host 代码在沙箱外** |
| 签名 cookie 鉴权 | `dsh-client-connection` | 每进程 token → `GET /?token=` → `HttpOnly`+`SameSite=Strict` cookie；401 在 RPC 前 |
| 用户终端 vs agent 终端 | `dsh-api-terminal-controller` | **用户终端：系统用户权限、不受沙箱/审批、不进 transcript**；8 终端/会话 |
| 交付物与 diff 审查 | `dsh-client-ui-deliverables` | 改动卡 / 审查 tab / 悬停 diff / 内联链接 / 原生打开 |
| 轨迹 | `dsh-client-ui-trajectory` | 纯投影；TTFT/decode 分离；可拖选过滤 |
| slot 体系 | `dsh-client-ui-slots` | 四类 slot；已观测字面量见 §1.8 |
| 默认出境 | `dsh-session-log-deepseek` | `enabled: true`、`maxBytes: 8MiB`、注入 `dsh_session_log`、at-least-once |
| 遥测同意 | `dsh-session-telemetry-otel` | 默认 `FEEDBACK_ONLY`，`FULL` 被拒；**redaction 规则出厂为空** |
| 凭据 | `dsh-credentials-local` | **明文 YAML**；"文件权限挡不住 agent，因为工具进程就是你的 OS 用户" |
| 互操作 | `dsh-acp` / `dsh-sdk-jsonrpc-server` / `dsh-mcp-client` / `dsh-hooks-{claude-code,codex}` | ACP 只服务标准 v1；SDK 无取消/无关闭；hooks 7/30 与 5/10 |
| 桌面载体 | `dsh-desktop-host` | `ELECTRON_RUN_AS_NODE`；`--port 19387` 硬编码；退出前锁准入 |

## 附录 B · 本设计的证据边界（诚实的部分）

1. 我**直接读取**了 DSH 发行版归档内 288 个包的 README、`lib/` 编译产物与配置（`[实测]`）；**没有**读过 DSH 仓库的 `src/`、`docs/subsystems/*.md`、`.agents/notes/*`（未随发行版发布）。
2. OneCode 侧：**实跑了两个仓的测试套件**（`npm test`，只写 `/tmp`），读了 `pty.js`/`index.js`/`config.js`/`term-ws.js` 全文与 desktop 的 `plans/`、`docs/`、CHANGELOG、Makefile；**没有逐行读** `providers.rs`(1171) / `pty/mod.rs`(1146) / `fs_explorer.rs`(549) 的全部实现。
3. **明确未验证**：`dsh-typert-generator` 内部（不在 288 包内）；`dsh-client-ui-dockkit`（右栏布局引擎，不在包集内）；`dsh-http-proxy` 是否被侧栏浏览器使用；**"profile" 没有独立 in-scope 包**，preset↔profile 关系只部分验证；`dsh-llm-deepseek` 只读到 README 层。
4. §3.1 的开源时间、提交数、社区插件数、Codex Harness、ARC-AGI-3 数据均为**外部公开报道**，未独立验证。
5. ACE 数据来自 `ace/docs/03–07`，是**自有语料**（本机 50,564 次调用 / 58 天），**外推性有限**，请按"我们自己"的样本理解。
6. 三次修订的记录在 §0.4 / §0.5 / §0.6，保留以便回溯"我原来判断错在哪"——**尤其是 §0.6：我把 web 当成产品、把 desktop 当成套壳，方向正好反了。**
7. 路线图判据都是**可测量的**，但**尚未执行** —— 它们是待验证假设，不是结论。
8. 本文的结论**继承而非另起**董事长的既有判断（`desktop-agents-ux-comparison.md`：接缝才是产品、组合而非替代、按任务选工具）。§5–§6 是把那条判断落到 DSH × OneCode 的具体形态。
