# QA 报告 · onecode-desktop M1 多供应商管理 + 手动切换（Go/No-Go 验收）

> **QA 判定**：🟢 **Go（建议）** — 待董事长签核
> **日期**：2026-08-09 · **QA**：@qa · **级联追踪**：cascade-20260809-ceo-eng-m1-qa
> **验收范围**：PRD Phase1 M1（AC-F2.1/2.2/2.3/2.4/2.6 + AC-F3.1~F3.6）
> **证据链**：测试用例设计（30 条）→ 代码审查（P0-P3）→ 专项测试实现 + 执行 → 独立回归复跑

---

## 1. 结论摘要

| 项 | 结果 |
|---|---|
| **M1 判定** | 🟢 **Go（建议）**，无 P0 blocker |
| 专项测试 | `tests/provider-switch.test.mjs` 新增 29 条，**29 pass / 0 fail** |
| 前端全量 | `npm test` **261 pass / 57 suites / 0 fail**（基线 232，无回归） |
| 后端 Rust | `cargo test --lib` **35 pass / 0 fail**（含 providers.rs 5 条） |
| 工作区 | 仅新增 `tests/provider-switch.test.mjs`，源码零改动 |
| 阻塞项 | 无 P0 |
| 待签核风险 | 2 个 P1（错误路径健壮性）+ 5 个 P2（建议 M2 前修） |

> **⚠️ Go 附带条件**：以下两条 P1 不阻断 M1 验收（不在验收口径的快乐路径上），但**强烈建议在 M2（failover 里程碑）开工前修复**，届时将成为关键路径：
> - **P1-1** `_switching` 防抖无看门狗：事件丢失/重启挂起 → 切换器永久锁死，需重启 app 恢复
> - **P1-2** `perform_switch` 双写非原子：providers.json 写失败 → 裂脑 + 重启后静默回滚

---

## 2. 测试矩阵（按 PRD 验收标准）

| 验收标准 | 覆盖用例 | 结果 |
|---|---|---|
| AC-F2.1 入口可达（芯片 / /model 行为一致） | PS-17, PS-20 | ✅ |
| AC-F2.2 切换写 desktop.json → 重启 → 芯片更新 | PS-06, PS-10, PS-22, PS-26 | ✅ |
| AC-F2.3 活跃任务确认（G1 取消/切换） | PS-11, PS-12, PS-14, PS-15 | ✅ |
| AC-F2.4 无活跃任务直接切 | PS-16 | ✅ |
| AC-F2.5 每次手动切换写 history（reason=manual，后端职责） | PS-22（前端 IPC 契约） | ✅ |
| AC-F2.6 切换防抖 | PS-35 | ✅ |
| AC-F3.1 新增（预设带出/自定义四要素/缺字段拦截） | PS-23, PS-42 | ✅ |
| AC-F3.2 编辑（改字段生效/Key 掩码留空不覆盖） | PS-24, PS-45 | ✅ |
| AC-F3.4 删除约束（后端强制） | PS-38, PS-40 + Rust 单测 | ✅ |
| AC-F3.5 删除非当前 + 清理引用 | PS-25 + Rust 单测 | ✅ |

### 测试域覆盖（专项测试文件）
| 测试域 | describe | 用例数 | 结果 |
|---|---|---|---|
| 多供应商列表 | PS 多供应商列表 | 4 | ✅ |
| 切换事件流 | PS 切换事件流 | 3 | ✅ |
| 活跃会话确认 G1 | PS 活跃会话确认 G1 | 5 | ✅ |
| F2/F3 入口 | PS F2/F3 入口 | 2 | ✅ |
| desktop.json/providers.json 写回（IPC 契约） | PS 写回 IPC 契约 | 4 | ✅ |
| 重启会话 | PS 重启会话 | 3 | ✅ |
| 边界/异常（空目录/容错/防抖/删除约束） | PS 边界/异常 | 8 | ✅ |
| **合计** | **7** | **29** | **29 pass / 0 fail** |

---

## 3. 执行证据

### 3.1 专项测试（独立实现者执行）
```
node --test tests/provider-switch.test.mjs
# tests 29 / suites 7 / pass 29 / fail 0
```

### 3.2 前端全量（独立发布守门员复跑）
```
npm test
# tests 261 / suites 57 / pass 261 / fail 0
# 基线 = 232（改动前）→ 增量恰为 29（新专项），无任何回归
```

### 3.3 后端 Rust（独立发布守门员复跑）
```
cargo test --lib
# 35 passed / 0 failed / 0 ignored
# 含 providers::tests：history_fifo_keeps_last_50 / slugify_basic /
#   delete_last_provider_blocked / delete_current_provider_blocked /
#   delete_non_current_cleans_refs
```

### 3.4 工作区核对
```
git status --short → ?? tests/provider-switch.test.mjs   （唯一变更，源码零改动）
```

---

## 4. 代码审查发现（QA 独立审查子任务）

> 审查范围：`src/provider-switch.js`（602 行）、`src/ipc-bridge.js` M1 段、`src-tauri/src/commands.rs` Provider 段（725-833）、`src-tauri/src/providers.rs`（696 行）、`src-tauri/src/lib.rs`（50-58 / 157-164）。

### P0（阻断）— **无**
安全面核验干净：XSS 插值全部过 `esc()` / `textContent`；`test_connection` curl 全 `.arg()` 无 shell 拼接；日志不打 `api_key`。未发现系统崩溃 / 数据丢失 / 安全漏洞。

### P1（高影响，建议 M2 前必修）
| # | 问题 | 位置 | 触发场景 | 修复建议 |
|---|---|---|---|---|
| P1-1 | `_switching` 防抖无看门狗，事件丢失/重启挂起即永久锁死切换器 | provider-switch.js:274（守卫）+70-87（复位） | `restartTab` 挂起不 resolve，或 `perform_switch` 的 emit 失败被 `let _ =` 吞掉 → `provider-switched` 永不到达 → 芯片永久「⟳ 切换中…」，后续所有切换静默 return，只能重启 app | `_switching=true` 时挂定时兜底（如 30s 强制复位 + toast）；`restartSessions` 每个 `restartTab` 加 Promise.race 超时 |
| P1-2 | `perform_switch` 双写非原子：先写 desktop.json 后写 providers.json，后者失败=裂脑 | providers.rs:397-401 → :427 | providers.json 写盘失败（磁盘满/权限）→ 命令 Err，但 RwLock+desktop.json 已改新 creds → 新会话新供应商、存量旧 env、芯片旧 active；重启后 `reconcile_active_to_desktop` 把 desktop.json 静默回滚成旧供应商 | 先写 providers.json 再写 desktop.json（后者作提交点）；失败回滚；或单文件原子（临时文件+rename） |

### P2（功能受损，建议尽快）
| # | 问题 | 位置 |
|---|---|---|
| P2-1 | 编辑当前激活供应商不重启存量会话，key 轮换后存量会话静默断连 | providers.rs:265-269 + commands.rs:764-774 |
| P2-2 | `renderChip` 异步 fallback 无陈旧响应守卫，竞态可显示错误供应商 | provider-switch.js:132-142 |
| P2-3 | `destroy()` 订阅清理无效：Tauri v2 `listen()` 返回 Promise，`_unsubs` 存 Promise 调 `u?.()` 抛错被吞，热重载监听器叠加 | provider-switch.js:56,70,88,96-99 + ipc-bridge.js:286-293 |
| P2-4 | `providers.json` 默认权限 0644 明文存 N 个 API Key | providers.rs:175-185 |
| P2-5 | `providers_list` 把全部明文 api_key 回传渲染层 | commands.rs:729-732 |

### P3（体验/边界）
- 编辑可把 api_key 清空（缺守卫）providers.rs:252-254
- `reconcile_active_to_desktop` 遇悬空 active 不清理 providers.rs:300-306
- `overlay.remove()` 绕过 `_closeOverlay`（引用悬空 + 无动画）provider-switch.js:366/381/397
- 新增首个供应商自动激活 → 无 G1 确认直接重启全部会话 providers.rs:218-222 + commands.rs:750-757
- 后端允许 no-op 自切换（from==to 的 history 记录）providers.rs:407-427
- 空列表键盘导航 selIdx 可为 -1 provider-switch.js:227-229
- `_runningSessions` 把 restarting 态当运行中，可能打断自动重启 provider-switch.js:306-314

---

## 5. 需求 vs 实现偏差（需产品确认，非阻断）

1. **AC-F3.4 删除约束仅后端强制**：前端删除按钮对「当前供应商/唯一一家」**无本地禁用态**，直接发 `providers_delete` 靠后端拒绝 + toast 兜底。若 PRD 期望前端也禁用，属待确认差距。
2. **AC-F2.6 防抖粒度**：PRD 表述「同一 供应商:模型」，实现是**全局 `_switching` 标志**（任一在途切换阻塞所有切换）。若需按 (provider,model) 粒度，属需求与实现偏差。

---

## 6. 回退 / 可逆性（ADR 验收口径）

后端 `reconcile_active_to_desktop` 启动对账已实现（lib.rs:50-58）：删 `providers.json` + 移除 `active_provider_id` → 应用回退现状单供应商。删除约束 + 引用清理由 Rust 单测覆盖（`delete_*_provider_*` 3 条）。**ps 级 env 验证（`ps eww`）属 M2 坑①回归范围，本期未纳入自动测试。**

---

## 7. 最终建议

### 🟢 Go（建议）— 依据
- M1 验收口径（AC-F2.1/2.2/2.3/2.4/2.6 + AC-F3.1~F3.6）**全部通过**
- 前端 261 / 后端 Rust 35 全绿，**零回归**
- 无 P0 blocker；源码零改动；专项测试补齐（此前 tests/ 无 provider-switch 覆盖）

### ⚠️ Go 附带条件（请在签核时知悉）
1. **P1-1 / P1-2 建议 M2 开工前必修**（failover 里程碑使二者成为关键路径）
2. P2-4（providers.json 权限 0600）+ P2-5（key 回传）建议顺手加固
3. §5 两条需求/实现偏差请 @vp-eng 定夺是否纳入本期

### 阻断定义
若董事长对 P1/P2 不可接受 → 可改判 **No-Go**，QA 将把 P1-1/P1-2 升级为 M1 修复要求（预期 0.5-1 人日）。

---

*QA 证据路径：测试用例设计（子任务 A）→ 代码审查（子任务 B）→ 专项测试实现与执行（子任务 C）→ 独立发布回归复跑（子任务 D）。全部证据已由 @qa 独立核验。*
