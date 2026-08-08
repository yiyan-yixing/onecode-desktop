# PRD · onecode-desktop 模型自动切换 Phase 1

> **状态**：设计定稿（Phase 1 需求冻结） · **日期**：2026-08-08 · **主理**：@vp-eng
> **交付对象**：@dev（据此开工） · **验收口径**：@qa（据此出测试用例）
> **配套文档**：`auto-switch-design.md`（技术方案草案）· `auto-switch-interaction-design.md`（交互 v0.1）· `ADR-arch-model-auto-switch.md`（架构 ADR，S1-S6 已签）
> **基准（董事长已拍板，不可改）**：目标方案 = **方案③全量**，本期只做 **Phase 1 = Layer A 多供应商切换 + 重启式 failover**；代理层（请求级无缝）Phase 2 后置，触发条件 = 「对话不打断」成为硬验收标准（ADR S2）。
> **预置试点**：两档 —— DeepSeek-V4-Flash + GLM-5.2。
> **外观方向**：A+B —— VS Code 设计语言（**只做 token 层，不做 Monaco**）+ 与 VS Code 双向轻桥接（**只做现状按钮升级**）。
> **合规基线**：参考 `reference/cc-switch`（MIT，本地只读），移植文件头保留版权声明 + 附 LICENSE 副本，@clo 复核。
>
> **⚠️ 本 PRD 为设计定稿阶段文档，不实现、不改 `src-tauri/` 任何代码**。开工令由 @vp-eng 在下游拆单时给出。

---

## 1. 范围界定

### 1.1 本期做（MVP 收）

| # | 能力 | 说明 |
|---|------|------|
| 1 | 多供应商管理 | 供应商增删改查 + 连通性测试 + 删除约束；含两档预设 DeepSeek-V4-Flash / GLM-5.2，预设仅作「新增时下拉可选、自动带出默认 base_url + 型号」，仍需补 API Key |
| 2 | 手动切换 | 状态栏模型芯片（可点击）+ `/model` 命令面板入口；切换可能重启会话时明确告知（活跃任务确认） |
| 3 | 重启式自动 failover | 失败检测（PTY 输出解析失败特征）→ 切 failover 队列下一家 → 显式刷新 slot.env → pty.restart → toast + 事件 |
| 4 | 切换历史 | 存 `providers.json` 内 `history` 环形字段，上限最近 50 条，FIFO 淘汰；字段 time/from/to/reason |
| 5 | 首次配置引导 | `wizard_completed=false` 或未配置 api_key 时进入 F1 引导；引导优先、预设退居下拉 |
| 6 | 状态常驻可见性 | 模型芯片 hover 出 F6 tooltip；切换中旋转 / 引导色 / 无可用红点复用交互设计 §5 状态规范 |

### 1.2 本期不做（Phase 2 或 backlog）

| # | 能力 | 去向 | 说明 |
|---|------|------|------|
| 1 | 代理层 / 请求级无缝 failover | Phase 2 | S1/S2 已拍板后置；本期「切换方式」只提供重启会话 |
| 2 | 用量 / 成本计费 | backlog | 依赖 DB 常量 + 代理层遥测，ADR P2 裁掉，成本归因缺失需明示 |
| 3 | SQLite 持久化 | backlog | ADR S6 已签：JSON 文件足够，熔断健康 v1 内存态 |
| 4 | Monaco 编辑内核 | 另一 ADR 范围 | 外观 A 只做 token 层，不引入 Monaco（ADR §7.2 明确无架构耦合） |
| 5 | VS Code 深度桥接 | backlog | 外观 B 只做现状「用 VS Code 打开」按钮升级，不做双向深度桥 |
| 6 | 高级字段（自定义供应商） | backlog | 自定义表单只做四要素：名称 / Base URL / API Key / 型号；代理、并发、上下文窗口等高级字段不做 |
| 7 | 预设全量 21 模型 | backlog | 本期仅两档试点，后续按需扩 |

### 1.3 范围裁定记录（PM 已收敛，不再是开放问题）

交互设计 §7 的 4 个待确认点已在 PM 层定稿，直接作为 Phase 1 范围基准：

| # | 待确认点 | 裁定 | 影响 |
|---|---------|------|------|
| C1 | 模型芯片与 cc-status 并排的视觉 | 将现有 `ribbonModelLabel`（只读模型文本）升级为**可点击模型芯片**（statusbar-item），置于 CC 徽章旁；切换状态（⟳ 旋转 / 引导色 / 无可用红点）复用交互设计 §5 状态规范；**不新建独立工具栏**；芯片 hover 出 F6 tooltip | 前端状态栏单点改造，不新增 UI 容器 |
| C2 | 自定义供应商表单一期做吗 | **做（简化版）**。产品价值 = 「对接任意 OpenAI/Anthropic 兼容端点」，只做两档预设会锁死扩展。表单只做四要素（名称 / Base URL / API Key / 型号），不做高级字段；两档预设退居为「新增时下拉可选、自动带出默认 base_url + 型号」，仍需补 API Key | F3「新增」同时支持预设选择 + 自定义四要素表单 |
| C3 | 无缝转发一期开放吗 | **不开放**。F4「切换方式」一期只提供「重启会话」，无缝转发选项置灰并标注「Phase 2 开放（需代理层）」；交互层 UX 不感知差异（仅文案） | F4 单选项，杜绝误导 |
| C4 | 切换历史存哪 / 留多久 | 存 `~/.onecode/providers.json` 内新增 `history: Vec<SwitchEvent>` 环形字段，上限保留最近 50 条，FIFO 淘汰；**不建独立文件、不引入 SQLite**；字段含 time / from / to / reason（连续失败/超时/限流/手动） | 数据落在 ProviderStore，单文件双字段 |

---

## 2. 用户故事与验收标准

> 每条均给出可测验收点，@qa 可直接据此转测试用例。前置依赖：C1 芯片落位、`providers.json` 可用。

### F1 · 首次配置引导（引导优先、预设退居下拉）

**作为**首次使用 onecode-desktop 的用户，**我希望**启动后先理解「供应商 / Base URL / API Key / 型号」四要素再配置，**以便**不盲配、一次配对。

```
┌─ 首次使用 · 配置你的模型 ─────────────────────────────┐
│ 📖 先读一下（配置引导）                                  │
│  · 模型 = 一家「供应商」+ 一个「型号」                    │
│  · 供应商：提供 AI 接口的服务商（如 DeepSeek、GLM、自有网关）│
│  · Base URL：供应商的接口地址（HTTP 开头）                │
│  · API Key：去供应商后台申请的一串密钥（保密）             │
│  · 型号：该供应商下的具体模型名（如 deepseek-v4-flash）     │
│  ❓ 看不懂？ [查看完整指南] [示例截图]                     │
│                                                      │
│ 开始配置：                                             │
│  供应商    [ 选择或手动填写 ▼ ]  ← 预设在此可选，非默认     │
│  Base URL [______________________]                    │
│  API Key  [______________________]  👁                 │
│  型号      [______________________]                    │
│  [ 测试连接 ]      [ 完成配置 ]                          │
│  ───────────  或  ───────────                         │
│  [ 稍后配置 ]（先逛产品，之后从状态栏芯片随时可配）         │
└──────────────────────────────────────────────────────┘
```

**Given** 应用首次启动且 `wizard_completed=false`（或 `api_key` 为空）
**When** 进入首次引导流程
**Then** 引导文本 + 配置表单展示；供应商下拉含 DeepSeek-V4-Flash / GLM-5.2 预设项，但默认为空选（预设不替引导）

**验收标准（可测）**
- AC-F1.1 进入条件：`wizard_completed=false` 或 `api_key` 为空 → 展示引导，不展示空白状态栏。
- AC-F1.2 在下拉选中预设（如 DeepSeek-V4-Flash）→ Base URL 与 型号 自动带出默认值，API Key 仍为空，需用户补。
- AC-F1.3 「测试连接」复用 F3 连通性测试；**未通过**时「完成配置」按钮不可用，并展示具体原因（网络 / Key / 型号名）。
- AC-F1.4 点击「完成配置」→ 写入 `desktop.json` + `providers.json` → 启动会话 → 状态栏芯片显示当前模型。
- AC-F1.5 点击「稍后配置」→ 不写配置 → 状态栏芯片显示引导色 `[ 配置模型 ▼ ]`；点击该芯片随时可重新进入 F1。
- AC-F1.6 引导态下退出并重进应用 → 仍回到 F1（`wizard_completed` 未置 true 前不得跳过）。

### F2 · 手动切换（≤2 步、活跃会话确认）

**作为**用户，**我希望**在状态栏芯片（或 `/model` 命令）两步内切换供应商/型号，**以便**换模型比配钥匙快，且不会被无声重启打断。

```
[◦] onecode   ~/my-project   ──  ──  ──   [ GLM-5.2 ▼ ]
                      点击芯片 ↓
┌─ 选择模型 ───────────────────────────┐
│ 🔍 搜索                                   │
│ ────────────────────────────────      │
│ ● GLM-5.2                  ← 当前      │
│ ● DeepSeek-V4-Flash   (推荐·快)        │
│ ────────────────────────────────      │
│ ▸ 管理供应商 …                          │
└───────────────────────────────────────┘

（选择非当前项 + 存在活跃任务时）
┌─ 切换模型 ───────────────────────┐
│ 将切到 DeepSeek-V4-Flash        │
│ 当前有 2 个活跃会话，切换会重启它们│
│                    [取消] [切换]  │
└──────────────────────────────────┘
```

**Given** 已配置 ≥2 家供应商
**When** 用户点击模型芯片 或 输入 `/model`
**Then** 弹出供应商选择列表（当前项标记）；选中非当前项时，有活跃会话则弹确认，无则直接切

**验收标准（可测）**
- AC-F2.1 入口可达：点击芯片（入口 A）与 `/model` 命令（入口 B）均能弹出选择列表，行为一致。
- AC-F2.2 列表内当前供应商带「当前」标记；切换选中项 → 写 `desktop.json`（新 provider creds + `active_provider_id`）→ 重启会话 → 芯片更新为「切换中 ⟳」→ 完成后高亮新模型。
- AC-F2.3 存在活跃任务时切换 → 弹确认；点「取消」不执行任何切换；点「切换」才执行。
- AC-F2.4 无活跃任务 → 不弹确认，直接切换。
- AC-F2.5 每次手动切换写入 `history`，`reason="manual"`。
- AC-F2.6 切换进行中防抖：同一供应商:模型切换未完成时，重复点击不触发第二次（`pending_switches` 语义）。

### F3 · 供应商管理（增删改测 + 删除约束）

**作为**用户，**我希望**在设置面板统一管理供应商（含自定义四要素表单），**以便**接入任意 OpenAI/Anthropic 兼容端点。

```
┌─ 模型供应商 ───────────────────────────────────────────────┐
│ [+ 新增]  [管理备用]   状态图例: ●使用中 ●可用 ●失败          │
│ ┌──────────────┬────────────┬────────┬────────┬──────────┐ │
│ │ 供应商/型号    │ 状态        │ 延迟    │ 上次结果│ 操作     │ │
│ ├──────────────┼────────────┼────────┼────────┼──────────┤ │
│ │ GLM-5.2      │ ● 使用中    │ 180ms  │ 正常   │ [编辑][×] │ │
│ │ DeepSeek-V4-Flash│ ● 可用  │ 90ms   │ 正常   │ [编辑][×] │ │
│ └──────────────┴────────────┴────────┴────────┴──────────┘ │
│ 自动切换  [ ● 开 ]  备用顺序见「管理备用」                      │
└────────────────────────────────────────────────────────────┘
```

**验收标准（可测）**
- AC-F3.1 新增：选择预设（自动带出 base_url + 型号，需补 Key）或自定义四要素（名称 / Base URL / API Key / 型号）→ 保存后列表出现新行。
- AC-F3.2 编辑：修改任意字段 → 保存生效；API Key 输入回显为掩码，仅展示/编辑不暴露明文到前端日志。
- AC-F3.3 测试：每行「测试」发一条最小 completion → 返回延迟 + 状态（正常 / 失败原因）；测试失败行进入「失败」状态图例。
- AC-F3.4 删除约束：**当前使用中的供应商不可删除**（「×」禁用或点击后要求先切换）；**全列表仅剩 1 家时不可删除**。
- AC-F3.5 删除非当前供应商 → 直接删除并同步清理 failover 队列与历史中对该 id 的引用。
- AC-F3.6 默认 `failover_queue` 随列表存在性自愈：首次开启自动切换且队列空 → 自动把当前供应商设为 P1。

### F4 · 自动切换设置（备用顺序 + 失败判定）

**作为**用户，**我希望**配置备用撤退路线与失败判定参数，**以便**供应商故障时系统自动切换。

```
┌─ 自动切换 · 备用顺序 ──────────────────────────────┐
│ 自动切换           [ ● 开 ]                        │
│ 说明: 当前供应商连续失败 N 次后，自动换到下一个备用      │
│ ──────────────────────────────────────────────  │
│ 撤退路线（从上到下）:                               │
│   1. GLM-5.2          [ 当前 ]  [↑][↓][✕]         │
│   2. DeepSeek-V4-Flash           [↑][↓][✕]        │
│   [+ 添加备用]                                    │
│ ──────────────────────────────────────────────  │
│ 失败判定:                                         │
│   连续失败   [ 4 ] 次触发                          │
│   单次超时   [ 60 ] s                             │
│ 切换方式: (●) 重启会话  ( ) 无缝转发(需代理) ·置灰·   │
│                                                 │
│         [ 保存 ]                                  │
└─────────────────────────────────────────────────┘
```

**验收标准（可测）**
- AC-F4.1 默认参数：连续失败阈值 `4` 次、单次超时 `60` s（对齐 cc-switch circuit_breaker 默认值）。
- AC-F4.2 「无缝转发」选项置灰，标注「Phase 2 开放（需代理层）」；仅「重启会话」可选。
- AC-F4.3 队列可上下调序（↑↓）与移除（✕），队列首项 = 当前供应商。
- AC-F4.4 开启自动切换时若队列空 → 自动把当前设为 P1（避免死锁，交互设计 §6.6）。
- AC-F4.5 保存 → 写 `providers.json` 的 `auto_failover_enabled` + `failover_queue`。
- AC-F4.6 关闭自动切换 → failover 状态机不触发；手动切换仍可用。

### F5 · failover 反馈（toast + 芯片更新 + 历史）

**作为**用户，**我希望**自动切换发生时被即时、非打扰地告知，**以便**失败是常态但必须可见可查。

```
触发瞬间（系统托盘级 toast，非模态、不抢焦点）:
┌──────────────────────────────────────────────┐
│ ⚠ GLM-5.2 连续失败 4 次                        │
│ → 已切到 DeepSeek-V4-Flash，当前会话已重启     │
│    [查看详情]             [关闭]              │
└──────────────────────────────────────────────┘
状态栏芯片同步更新为 DeepSeek-V4-Flash（短暂高亮闪烁）。
```

**验收标准（可测）**
- AC-F5.1 触发 failover → 弹 toast，文案含「连续失败 N 次 → 已切到 X」；重启式场景文案追加「当前会话已重启」。
- AC-F5.2 toast 非模态、不抢焦点；点击「查看详情」→ 进入切换历史；「关闭」→ 消失。
- AC-F5.3 芯片同步更新为新模型并短暂高亮闪烁；切换进行中显示「⟳ 旋转」态。
- AC-F5.4 每次 failover 写入 `history`，`reason` 区分：`consecutive_failure` / `timeout` / `rate_limited`。
- AC-F5.5 全部备用也失败 → 芯片进入 `[ ⚠ 无可用模型 ▼ ]` 红点态 + 立即模态提示 + 引导检查 Key/网络。
- AC-F5.6 同一事件不发生重复 toast（去重）。

### F6 · 状态常驻可见性（hover tooltip）

**作为**用户，**我希望**悬停芯片即可看到当前供应商/型号/健康/备用/最近切换，**以便**一眼掌握模型运行态。

```
┌─ 当前模型 ────────────────────┐
│ 供应商   GLM (oneapi 网关)      │
│ 型号     GLM-5.2              │
│ 健康     ● 正常 · 180ms        │
│ 备用     DeepSeek-V4-Flash    │
│ 最近切换  08-08 14:32（失败切） │
│ [进入设置]                     │
└───────────────────────────────┘
```

**验收标准（可测）**
- AC-F6.1 hover 模型芯片 → 展示 tooltip，字段齐全：供应商 / 型号 / 健康（● 正常·延迟 或 ● 失败）/ 备用 / 最近切换（时间 + 原因）。
- AC-F6.2 「进入设置」→ 跳转 F3 供应商管理页。
- AC-F6.3 未配置态 tooltip 显示引导文案 + 快捷进入 F1。
- AC-F6.4 tooltip 随状态变化实时更新（failover 后不再显示旧供应商健康）。

---

## 3. 技术落地要点

> 本节承接 ADR（S1-S6 已签），**不是重新设计**，具体到 @dev 能直接开工。字段名 / 状态机 / 事件名以下文为准。

### 3.1 数据层：两个文件、双单一真相、互不侵入（ADR §5.1）

| 文件 | 单一真相 | 管什么 |
|------|---------|--------|
| `~/.onecode/desktop.json`（现有 `AppConfig`，**只加一个字段** `active_provider_id: Option<String>`） | 现有 `ConfigManager`（Arc\<RwLock\> + 2s watcher，**不动**） | 激活 creds（api_key/base_url/model）+ 当前激活 provider 指针 |
| `~/.onecode/providers.json`（**新文件**） | 新增 `ProviderStore`（Arc\<RwLock\<ProviderCatalog\>\>） | `Vec<Provider>` + `failover_queue` + `auto_failover_enabled` + `history` |

**为什么不在 desktop.json 里长 providers 列表**（ADR §5.1 裁定，S4）：`AppConfig` 是扁平单供应商结构，被 `ConfigUpdate.apply_to` / `get_config_schema` / `pty_spawn` / watcher 全链路消费；塞 `Vec<Provider>` 会让「激活态+目录」两种生命周期缠在一起，所有消费者与前端 schema 跟着改。切换层与运行时配置是两个关注点。

#### providers.json schema 草案（@dev 落盘依据）

```jsonc
{
  // 供应商目录（Vec<Provider>）
  "providers": [
    {
      "id": "glm-5.2",                    // 全局唯一 id（slug，稳定，历史引用用）
      "name": "GLM-5.2",                  // 展示名
      "base_url": "https://open.bigmodel.cn/api/paas/v4",
      "api_key": "sk-...",                // 明文持久化（S5：Phase 1 key 仍走子进程 env，Phase 2 才移内存）
      "model": "glm-5.2",
      "created_at": "2026-08-08T00:00:00Z"
    },
    {
      "id": "deepseek-v4-flash",
      "name": "DeepSeek-V4-Flash",
      "base_url": "https://api.deepseek.com",
      "api_key": "",
      "model": "deepseek-v4-flash",
      "created_at": "2026-08-08T00:00:00Z"
    }
  ],
  // failover 撤退路线（有序；队列首项 = 当前）
  "failover_queue": ["glm-5.2", "deepseek-v4-flash"],
  // 自动切换总开关
  "auto_failover_enabled": true,
  // 失败判定参数（对齐 cc-switch circuit_breaker 默认值）
  "failover_params": {
    "consecutive_failures": 4,
    "timeout_seconds": 60
  },
  // 切换历史（环形，上限 50 条，FIFO 淘汰）
  "history": [
    {
      "time": "2026-08-08T14:32:00Z",
      "from": "glm-5.2",
      "to": "deepseek-v4-flash",
      "reason": "consecutive_failure"   // consecutive_failure | timeout | rate_limited | manual
    }
  ]
}
```

- 两档预设的默认 `base_url` + `model` 作为常量预置（前端下拉带出用），不硬编码进用户数据。
- `history` 写前检查长度，超过 50 条移除最旧（FIFO）。

#### desktop.json 变更（最小侵入）

- 仅加：`active_provider_id: Option<String>`（`#[serde(default)]`，缺省 None 回退现状单供应商）。
- 消费方式：**走现有 `ConfigManager` + `save_config` / `ConfigUpdate` 路径**，不新造配置通道。`ConfigUpdate` 增加 `active_provider_id: Option<String>` 一个可选字段（沿用 `apply_to` 只合并有值字段的既有模式）。
- `pty_spawn` 读 `config.api_key/base_url/model` → env 注入链路**零改动**（ADR §1.3 / §5.2，S4）。

### 3.2 激活路径（SwitchManager 双单一真相）

```
用户选 Provider / failover 切档
  → SwitchManager:
      ① 该 provider 的 api_key/base_url/model 写进 AppConfig（走现有 save_config / ConfigUpdate 路径）
      ② 同时 active_provider_id + failover_queue + enabled 写进 providers.json
  → commands.rs pty_spawn 照旧读 config.api_key/base_url/model → env 注入（一行不改）
  → 重启式场景：显式刷新 slot.env → pty.restart
```

### 3.3 重启式 failover 状态机（Phase 1 落地）

```
[监控] 解析 PTY 输出流，匹配失败特征（见 3.4 坑②）
   │
   ├─ 用户主动 cancel 的请求 / 会话正常退出 → 不计数、不触发（reset 计数）
   │
   ├─ 匹配失败特征（403/401 鉴权、429 限流、额度耗尽、模型不存在、连接拒绝）
   │     → 失败计数 +1（按 provider_id 独立计数；连续成功则清零）
   │
   ├─ 单次请求超时 ≥ 60s → 记 timeout
   │
   ▼  连续失败 ≥ 4 次 或 timeout 触发，且 auto_failover_enabled=true
[切换] 取 failover_queue 下一家（跳过当前；队列为空 → 进「无可用模型」态 + 模态提示）
   │
   ▼
[落盘] SwitchManager 写 desktop.json（新 creds + active_provider_id）
   │      + 写 providers.json（queue 轮转、history 追加、reason）
   ▼
[重启] 显式刷新 slot.env（重取 config 覆盖，勿用旧快照）→ pty.restart
   │
   ▼
[反馈] emit "provider-switched" → 前端：芯片更新 + 高亮 + toast + 历史刷新
```

- 熔断健康 v1 为**内存态**（S6）：重启进程后失败计数清零，可接受。
- 「开关关闭 / 队列空」任一 → 状态机不进入切换分支。

### 3.4 ⚠️ 两个实现坑（ADR §5.3，@dev 必须遵守）

1. **`pty.restart` 复用 slot 旧 env**：切档后直接 restart 不会自动带新 env。**必须在 failover 流程里显式刷新 `slot.env`**（重取 `config` 覆盖 env），再调 `pty.restart`。否则切了模型、进程还是旧 env，等于没切。
2. **失败检测不能只看退出码**：要解析 PTY 输出里的 **403/鉴权/额度特征** 才能区分「该切」与「不该切」。**用户主动 cancel 的请求不触发 failover**（只统计 API/网络错误，不统计 cancel）。只依赖退出码会漏判（服务端 HTTP 层错误进程可能仍存活）或误判（用户 cancel 也计数）。

### 3.5 事件名（Tauri emit）

| 事件 | 触发方 | 载荷 | 消费方 |
|------|--------|------|--------|
| `provider-switched` | SwitchManager（failover 或手动切换成功） | `{ from: provider_id, to: provider_id, reason: "manual"\|"consecutive_failure"\|"timeout"\|"rate_limited" }` | 前端：刷新芯片 + toast + 历史 |
| `providers-changed`（建议新增） | ProviderStore（目录 CRUD 后） | `{ }` 或轻量摘要 | 前端：F3 列表刷新 |
| `failover-state`（建议新增） | ProviderStore（开关/健康态变更） | `{ auto_failover_enabled: bool, healthy: bool }` | 前端：芯片红点/引导色 |
| `config-changed`（已有） | ConfigManager 2s watcher | `()` | 前端：配置热加载（复用，不新建） |

> `provider-switched` 为 ADR 已定事件名；`providers-changed` / `failover-state` 为建议新增，@dev 落地时可按最小集裁剪（至少 `provider-switched` 必须实现）。

### 3.6 SwitchManager 职责说明（新增·编排核心）

| 职责 | 落点 | 依据 |
|------|------|------|
| ① 写激活态 → `AppConfig(desktop.json)` | 走现有 `save_config` / `ConfigUpdate` 路径 | ADR §5.2（S4） |
| ② 写目录 → `providers.json` | `ProviderStore`（新增，Arc\<RwLock\<ProviderCatalog\>\>） | ADR §5.1（S4） |
| ③ 编排重启式 failover | 读 failover 队列 → 落盘 → 刷新 slot.env → pty.restart → emit | ADR §2 架构图 |
| ④ 维护历史环形字段 | `history` 上限 50 FIFO | C4 裁定 |

> 不做：代理层路由 / 熔断持久化 / 用量统计（均 Phase 2 或 backlog）。

### 3.7 合规（MIT attribution）

- `reference/cc-switch`（MIT）：Layer A 的 `provider.rs` / `database/dao/providers.rs` / `app_config.rs` 及前端 presets 可移植/改造。
- **每个移植文件头保留 cc-switch 版权声明** + 项目内附 LICENSE 副本；@clo 复核 attribution 写法（ADR §10.2）。

---

## 4. 里程碑拆解

### M1 · 多供应商管理 + 手动切换

**目标**：用户在 app 内能管理供应商并手动切换（Layer A 落位，地基建好）。

**可交付物**
- `providers.json` + `ProviderStore`（CRUD、failover_queue、auto_failover_enabled、history 骨架）。
- `desktop.json` 加 `active_provider_id`；`ConfigUpdate` 支持该字段；激活路径走现有 save_config（pty_spawn 零改动）。
- 状态栏模型芯片（C1 升级 ribbonModelLabel）+ `/model` 命令 + F2 切换 + 活跃会话确认。
- F3 供应商管理（增删改测 + 预设两档 + 自定义四要素表单 + 删除约束）。

**Go/No-Go 验收口径（@qa 判定）**
- AC-F2.1 / AC-F2.2 / AC-F2.3 / AC-F2.4 / AC-F2.6、AC-F3.1~F3.6 全部通过。
- 切换后 `ps` 级验证：新会话进程 env 携带新 base_url/model（证明 pty_spawn 零改动链路正确）。
- 回退验证（ADR 可逆性）：删 `providers.json` + 移除 `active_provider_id` → 应用回退现状单供应商，无残留。

### M2 · 重启式 failover

**目标**：供应商故障自动切到备用（Phase 1 核心差异点）。

**可交付物**
- PTY 输出失败特征解析（403/401/429/额度/模型不存在/连接拒绝）。
- 失败计数状态机（按 provider_id 独立计数、连续成功清零、cancel 不计数、超时 60s）。
- failover 编排：落盘 → 显式刷新 slot.env → pty.restart → `provider-switched` emit。
- toast（F5）+ 芯片状态更新（⟳/高亮/红点）+ 队列空处理。

**Go/No-Go 验收口径（@qa 判定）**
- AC-F4.1~F4.6、AC-F5.1~F5.6 全部通过。
- **坑①回归**：mock 供应商 A 连续失败 → 切到 B 后，用 `ps eww <pid>` 验证新 env 已变（无旧 base_url 残留）。
- **坑②回归**：用户 cancel 场景不触发 failover；401/429 场景必须触发。
- 队列空 + 自动切换开 → 进入「无可用模型」态，不崩、不死锁。

### M3 · 引导 / 历史 / 打磨

**目标**：首次体验闭环 + 可查性 + 外观 token 化打磨。

**可交付物**
- F1 首次配置引导（含测试连接、稍后配置、引导色芯片）。
- F6 hover tooltip（供应商/型号/健康/备用/最近切换）。
- 切换历史完整链路（F5「查看详情」→ 历史列表；50 条 FIFO）。
- 状态规范全面对齐交互设计 §5（正常/切换中/failover 后/无可用/未配置五态）。
- VS Code token 层视觉打磨（不做 Monaco）。

**Go/No-Go 验收口径（@qa 判定）**
- AC-F1.1~F1.6、AC-F6.1~F6.4 全部通过。
- 历史 FIFO：人为触发 51 次切换 → 历史仅保留最近 50 条。
- 五态视觉逐一截图比对交互设计 §5 表格。
- 全量回归 M1/M2 用例（不得引入回归）。

---

## 5. 风险清单

> 延续 ADR P0/P1/P2 分级。Phase 1 范围内，每项带规避。

### P0（阻断级）

| # | 风险 | 规避 |
|---|------|------|
| R1 | **PTY env 不刷新**：`pty.restart` 复用 slot 旧 env，切了模型进程仍是旧 env（ADR §5.3 坑①） | failover 流程中**显式刷新 `slot.env`**（重取 config 覆盖）再 restart；M2 Go/No-Go 用 `ps eww` 回归验证 |
| R2 | **失败检测误判/漏判**：只看退出码会漏判（HTTP 层错误进程可能存活）或误判（用户 cancel 也计数）（ADR §5.3 坑②） | 解析 PTY 输出失败特征（403/401/429/额度/模型不存在/连接拒绝）；cancel 不计数；M2 用 cancel/401/429 三场景回归 |

### P1（高影响，可绕）

| # | 风险 | 规避 |
|---|------|------|
| R3 | **删除当前供应商导致死锁**：当前使用中供应商被删 → 激活态悬空 | 删除约束：使用中不可删；仅剩 1 家不可删；若异常出现激活态悬空 → 自动退到队列首备用，队列空则「无可用模型」态（交互设计 §6.3） |
| R4 | **failover 队列空 + 自动切换开**：开了开关没有撤退路线 → 触发时无路可退 | 开启自动切换时若队列空 → 自动把当前设为 P1（交互设计 §6.6）；队列空触发时进「无可用模型」态，不崩 |
| R5 | **MIT 合规 attribution 缺失**：移植 cc-switch 代码未保留版权声明 → 法律风险 | 每个移植文件头保留版权声明 + 附 LICENSE 副本；@clo 复核 attribution 写法（ADR §10.2）；commit 前 @clo 检查 |
| R6 | **key 安全**：providers.json 明文持久化 API Key | S5 已签：Phase 1 key 走子进程 env 注入（现状不变）；Phase 2 才移内存 + providers.json 持久化改占位符。前端不落明文日志、回显掩码；`providers.json` 权限参考 desktop.json 现状 |

### P2（低影响，可暂缓）

| # | 风险 | 规避 |
|---|------|------|
| R7 | **切换历史无限增长** | `history` 环形字段上限 50 条 FIFO 淘汰（C4 裁定）；M3 Go/No-Go 用 51 次触发回归 |
| R8 | 熔断健康仅内存态，重启进程计数清零（S6 已知取舍） | 接受，v2 落盘；不阻塞本期 |
| R9 | 用量/成本归因缺失（ADR P2 裁掉） | 明示为已知限制，Phase 2 代理层遥测补上 |

---

## 6. 开放问题

**无阻断性开放问题。**

交互设计 §7 的 4 个待确认点已由 PM 收敛并写入 §1.3 范围裁定记录（C1-C4）；架构 6 项签字点（S1-S6）已由董事长 2026-08-08 全签。本 PRD 可作为 @dev 开工依据。

以下为非阻断观察项（不阻塞开工，出现时按列出的决策人拍板）：

| # | 观察项 | 触发条件 | 决策人 |
|---|--------|---------|--------|
| O1 | 预设供应商扩展节奏（21 模型何时全量预置） | 两档试点跑通后 | 董事长 |
| O2 | `providers.json` 跨设备迁移/同步 | 用户多设备诉求出现 | @architect |
| O3 | 自定义供应商高级字段（代理/并发/上下文窗口） | 有真实用户提诉求 | @vp-eng（PM） |
| O4 | 熔断健康 v2 落盘时机 | failover 误判率上升 / 需要跨会话统计 | @architect |

---

*本文档为设计定稿阶段产出，不实现、不改 `src-tauri/` 代码。开工令由 @vp-eng 拆单下发，@dev 实现，@qa 验收。*
