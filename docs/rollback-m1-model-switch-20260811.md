# 回滚预案 · onecode-desktop M1 多供应商管理 + 手动切换

> **级联追踪**：cascade-20260811-ceo-devops-m1-deploy
> **维护**：@devops · **日期**：2026-08-11
> **范围**：M1（PRD Phase1 Layer A）部署回退 · 部署记录见 `.claude/blackboard/cascade-ceo-devops-m1-deploy-20260811.md`
> **可逆性**（ADR-arch-model-auto-switch.md §7）：Layer A = 🟢 完全可逆，1 天内拆除零残留

---

## 1. 回滚触发条件

- 部署后 M1 功能异常：切换不生效 / env 注入错误 / 崩溃 / 数据丢失
- 董事长或 @qa 在验收期判定回滚
- 目标：≤5 分钟回到部署前单供应商可用态

## 2. 回滚步骤

### 2.1 数据层回退单供应商
```bash
# 删除供应商目录（providers.json）
rm -f ~/.onecode/providers.json

# 移除 desktop.json 的 active_provider_id（用 python 保证 JSON 合法）
python3 - <<'EOF'
import json, os
p = os.path.expanduser("~/.onecode/desktop.json")
with open(p) as f: d = json.load(f)
d.pop("active_provider_id", None)
with open(p, "w") as f: json.dump(d, f, ensure_ascii=False, indent=2)
EOF
```

### 2.2 应用二进制回退（如需退回旧版本）
```bash
# 用部署前备份覆盖（路径见 §3）
ditto /Applications/OneCode.app.pre-m1.deploy.bak /Applications/OneCode.app
# 或从 pre-M1 dmg / app 重新安装
```

### 2.3 重启验证
```bash
open -a /Applications/OneCode.app
```
**预期**：应用回退单供应商态 —— 芯片显示 `desktop.json` 的 `config.model`（deepseek-v4-flash），spawn 用默认 creds；`reconcile_active_to_desktop` 对无 `active_provider_id` 的情况不干预（`lib.rs:50-58`），无残留不崩。

## 3. 备份产物清单

| 备份 | 路径 | 说明 |
|---|---|---|
| pre-M1 app | `eng/onecode-desktop/target/release/OneCode.app.pre-m1.bak` | 部署前备份（已有） |
| pre-M1 dmg | `eng/onecode-desktop/target/release/OneCode_0.3.0_aarch64.pre-m1.dmg` | 部署前备份（已有） |
| 部署前 app | `/Applications/OneCode.app.pre-m1.deploy.bak` | 本次部署流水线创建（2026-08-11 14:27） |
| M1 新构建 .app | `eng/onecode-desktop/target/release/bundle/macos/20260811/OneCode.app` | M1 部署产物副本 |
| M1 新构建 dmg | `eng/onecode-desktop/target/release/bundle/dmg/OneCode_0.3.0_aarch64.dmg` | M1 部署产物 |
| 用户配置 | `~/.onecode/providers.json.bak-20260811` · `~/.onecode/desktop.json.bak-20260811` | 验证前备份（验证后已恢复，与当前一致） |

## 4. 已知残留风险（排入 M2 / 后续，不阻塞回滚可用性）

| # | 风险 | 影响 | 去向 |
|---|---|---|---|
| P1-1 | `_switching` 防抖无看门狗 | 事件丢失/重启挂起 → 切换器永久锁死，需重启 app | M2 加超时看门狗 |
| P1-2 | `perform_switch` 双写非原子 | providers.json 写失败 → 裂脑 + 重启后静默回滚 | M2 改单文件原子写 |
| P2-4 | providers.json 明文 API Key | 权限 0644 明文存 key | 建议 0600 加固 |
| P2-5 | providers_list 明文 key 回传渲染层 | 前端内存暴露 | Phase 2 改占位符 |
| G1 | 预设 OpenAI 协议 vs claude-code 后端 | 切预设后 claude-code 会话可能无法真正对话 | M2/backlog 按后端感知补 /anthropic |
| — | 回滚「app 存活」待隔离复测 | 本次受并发实例干扰未定性 | 用户停用后单实例复测 |

---
*本文件为部署回滚预案的版本控制副本；部署级联记录见 blackboard。*
