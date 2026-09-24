# 事故记录：两个供应商的上下文溢 400（2026-09-23）

> 一句话：**供应商没把「模型真实窗口」告诉 Claude Code**，于是会话一路涨过 provider 的上限，
> 直到撞墙报 400。修法是把这个数字变成**算出来的**，而不是手填、会漏、会过期的。

## 1. 现象（董事长实测原报）

```
阿里 qwen3.8-27b: 400 input length (392074 tokens) exceeds the model's maximum context length (262144 tokens)
deepseek v4 flash: 400 This model's maximum context length is 1048576 tokens.
                    However, you requested 1049426 tokens
                    (1017426 in the messages, 32000 in the completion)
```

两起形状不同、根因同一个：

| | 真窗口 | 出事时请求 | 差在哪 |
|---|---|---|---|
| qwen3.8-27b | 262,144 | 输入 392,074 | 涨过窗口 **13 万** |
| deepseek v4 flash | 1,048,576 | 101,7426 + 32,000 补全 = 1,049,426 | **只差 850 token**，补全预算没算进去 |

## 2. 诊断（一手证据，不是推测）

在这个 App 里，供应商的窗口原本靠 `extra_env` 手填 —— **而实际在用的供应商是用户「自定义新增」的，
`extra_env` 是空的**（预设里那个 `CLAUDE_CODE_AUTO_COMPACT_WINDOW=786432` 根本没生效）。
于是 Claude Code 只能按「未识别型号」的默认值办事。

两条 CLI 一手证据：

```bash
$ claude -p "..." --model 'deepseek-v4-flash[1m]'
[claude-code:unrecognized_model] {"model":"deepseek-v4-flash[1m]","query_source":"sdk"}
```

```
$ claude -p "/context"        # 修复前会显示 "default for an unrecognized model"
**Tokens:** 13.9k / 702k (2%)     ← 修复后：CLI 认我们声明的阈值
| Autocompact buffer | 33k | 4.7% |
```

CLI 二进制里也写明了它对用户的要求（`strings` 抓的原文，不是猜的）：

| 旋钮 | 作用 | CLI 原文 |
|---|---|---|
| `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | 声明**模型真实窗口** | 「set CLAUDE_CODE_MAX_CONTEXT_TOKENS to its real window」 |
| `CLAUDE_CODE_AUTO_COMPACT_WINDOW` / settings 键 `autoCompactWindow` | **何时自动压缩** | 「实际阈值 = 该设置与模型最大窗口取小」 |

## 3. 修法

新增 `src-tauri/src/model_context.rs`：**窗口只从这四种来源推导**，优先级从高到低——

1. **用户显式填的**（供应商表单新增「上下文窗口」字段，可留空）
2. **型号后缀**（`deepseek-v4-pro[1m]` → 1,048,576；`[256k]` → 262,144）——这是用户唯一能一眼写清的声明
3. **内置表**（只放**能指出出处**的数：当前两条都来自本次报错原文）
4. **保守默认 128K**，并在界面上标注「未识别型号（保守值）」

然后 `sync_active_to_claude_settings` 每次同步都写两个值：

- `env.CLAUDE_CODE_MAX_CONTEXT_TOKENS` = 真窗口
- settings 键 `autoCompactWindow` = **真窗口 − 补全预算 − 单轮暴涨余量 − 兜底余量**

### 压缩阈值为什么要留那么多

三项各有出处：

| 项 | 值 | 出处 |
|---|---|---|
| 补全预算 | 32,000 | deepseek 那起的报错原文（`32000 in the completion`） |
| 单轮暴涨余量 | 窗口的 25% | **量出来的**：阈值 786,432 而失败请求已到 1,017,426 —— 单轮超出阈值 23 万 token。旁证：DeepSeek 官方给 1M 窗口推荐的正是 768k = 75% |
| 兜底余量 | 窗口的 5% | 防抖动 |

结果：1M → 702,004（67%）；256K → 151,501（58%）。

### 两条自我约束

- **一方模型（Claude 系）什么都不声明。** CLI 的 `auto` 比我们的表准，我们插手只会把窗口调小
  （表里没有 Claude 型号 → 落到 128K 保守默认 → 200K 的模型被按 90K 压缩，纯亏）。
  判据：型号以 `claude` 开头，或 base_url 含 `anthropic.com`。
- **未识别型号不许装作知道。** 值是我们估的，界面必须显示「未识别型号（保守值）」并给出手填入口。
  偏高 → 400（本次的两起）；偏低 → 压缩得早、白花钱。两个方向都难受，所以让用户看得见。

## 4. 验证

| 验证 | 结果 |
|---|---|
| Rust 单测（含窗口解析、阈值数学、一方模型不插手、切换不残留） | 44 passed |
| JS 单测（含新增 2 条表单契约、1 条非法值拦截） | 268 passed |
| **CLI 实报**（`/context`，deepseek 1M） | `13.9k / 702k (2%)`、Autocompact buffer 33k |
| **CLI 实报**（隔离配置，qwen 256K） | `26.1k / 151.5k (17%)` |
| 线上配置已就地修复（当前激活的 deepseek 条目） | `MAX_CONTEXT_TOKENS=1048576`、`autoCompactWindow=702004` |

顺手修掉一个**长期红着的测试**：`PS-23` 的期望漏了 `extra_env`，从 `extra_env` 落地那天起就一直是红的
（红测试会掩盖后续回归，所以本轮一起修）。

## 5. 已知缺口（没做，别当成做了）

- **本地 Ollama 供应商**走 `default`（128K），但实测本机 Ollama 的运行时上下文是 **262,144**
  （`ollama ps` 的 CONTEXT 列；训练窗口也是 262144，num_ctx 未设时 Ollama 按训练窗口自动撑满）。
  这类供应商目前会压缩得偏早——在上面的手填框里填 262144 即可修正。
  没自动探测的原因：同步路径是同步函数，塞一次 HTTP 探测不合适；要做应该在异步的增改路径里做一次并落盘。
- **内置表只有两条**（都是本次报错原文里带的数）。其余第三方型号靠用户手填或后缀声明——
  这是刻意的：**猜出来的数字比没有更危险**（猜大了照样 400，而且看起来像是配好了）。
