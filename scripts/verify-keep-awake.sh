#!/bin/bash
# 防熄屏功能自动验证脚本
#
# 通过 `npx tauri dev` 启动应用，用 pmset -g assertions 在各阶段检查 IOKit assertion。
# 因为是 GUI 应用，终端创建需用户在 app 里点击，但启动态和退出态可自动检查。

set -euo pipefail
cd "$(dirname "$0")"

APP_NAME="OneCode Desktop"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

pass() { echo "${GREEN}✅ PASS${NC}: $1"; }
fail() { echo "${RED}❌ FAIL${NC}: $1"; }
info() { echo "${YELLOW}ℹ️${NC} $1"; }

# ── 0. 确保没有残留进程 ──
pkill -f "OneCode Desktop" 2>/dev/null || true
sleep 1

# ── 1. 基线：应用未运行时不应有 OneCode 的 PreventUserIdleDisplaySleep ──
info "检查基线：应用未运行..."
BASELINE=$(pmset -g assertions 2>/dev/null | grep "PreventUserIdleDisplaySleep" | grep -c "OneCode" || true)
if [ "$BASELINE" -eq 0 ]; then
    pass "基线：无 OneCode 的 PreventUserIdleDisplaySleep（正确）"
else
    fail "基线：已有 OneCode 残留 PreventUserIdleDisplaySleep，请先退出 OneCode"
    pmset -g assertions | grep "PreventUserIdleDisplaySleep"
fi

# ── 2. 启动应用 ──
info "启动 OneCode Desktop (tauri dev)..."
export CC=$(xcrun --find clang 2>/dev/null || echo /usr/bin/cc)
export CXX=$(xcrun --find clang++ 2>/dev/null || echo /usr/bin/c++)
export SDKROOT=$(xcrun --sdk macosx --show-sdk-path 2>/dev/null)
export PATH=/usr/bin:/bin:/usr/sbin:/sbin:$PATH
# tauri dev 必须从项目根目录运行（需要 tauri.conf.json）
npx tauri dev -- &
TAURI_PID=$!
info "Tauri dev PID: $TAURI_PID"

# 等待应用启动（Tauri dev 首次启动较慢）
info "等待应用启动..."
for i in $(seq 1 60); do
    if pmset -g assertions 2>/dev/null | grep -q "PreventUserIdleSystemSleep.*OneCode"; then
        info "检测到 OneCode 的 PreventUserIdleSystemSleep assertion（应用已启动）"
        break
    fi
    if ! kill -0 $TAURI_PID 2>/dev/null; then
        fail "Tauri dev 进程已退出（可能构建失败）"
        exit 1
    fi
    sleep 2
done

sleep 3  # 额外等待确保 assertion 稳定

# ── 3. 检查启动态 ──
info "===== 阶段 1：应用启动，无终端 ====="
ASSERTIONS=$(pmset -g assertions 2>/dev/null)

# 应有 PreventUserIdleSystemSleep（全生命周期常驻）
SYS_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleSystemSleep" || true)
if [ "$SYS_SLEEP" -ge 1 ]; then
    pass "PreventUserIdleSystemSleep 已激活（系统不休眠）"
else
    fail "PreventUserIdleSystemSleep 未找到"
fi

# 不应有 PreventUserIdleDisplaySleep（改由 PTY 驱动，启动无终端）
DISP_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleDisplaySleep" || true)
if [ "$DISP_SLEEP" -eq 0 ]; then
    pass "PreventUserIdleDisplaySleep 未激活（正确：启动时无终端）"
else
    fail "PreventUserIdleDisplaySleep 已激活（预期：启动时不应有）"
    echo "$ASSERTIONS" | grep "PreventUserIdleDisplaySleep"
fi

# ── 4. 等待用户手动创建终端 ──
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  👉 请在 OneCode Desktop 中创建一个终端 tab"
echo "  然后回到此终端按 Enter 继续..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
read -r

# ── 5. 检查有终端时的状态 ──
info "===== 阶段 2：有终端运行 ====="
ASSERTIONS=$(pmset -g assertions 2>/dev/null)

DISP_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleDisplaySleep" || true)
if [ "$DISP_SLEEP" -ge 1 ]; then
    pass "PreventUserIdleDisplaySleep 已激活（屏幕不熄灭）"
    # 验证是 OneCode 持有的
    OWNER=$(echo "$ASSERTIONS" | grep -A1 "PreventUserIdleDisplaySleep" | grep -o 'pid [0-9]*([^)]*)' || true)
    info "持有者: $OWNER"
else
    fail "PreventUserIdleDisplaySleep 未激活（预期：有终端时应激活）"
fi

SYS_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleSystemSleep" || true)
if [ "$SYS_SLEEP" -ge 1 ]; then
    pass "PreventUserIdleSystemSleep 仍激活"
else
    fail "PreventUserIdleSystemSleep 丢失"
fi

# ── 6. 等待用户切换到别的 app（验证失焦不释放 assertion） ──
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  👉 请 Cmd+Tab 切到别的应用（如 Finder）"
echo "  然后回到此终端按 Enter 继续..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
read -r

# ── 7. 检查失焦后 assertion 仍在 ──
info "===== 阶段 3：窗口失焦后 ====="
ASSERTIONS=$(pmset -g assertions 2>/dev/null)

DISP_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleDisplaySleep" || true)
if [ "$DISP_SLEEP" -ge 1 ]; then
    pass "PreventUserIdleDisplaySleep 失焦后仍在（核心修复验证通过！）"
else
    fail "PreventUserIdleDisplaySleep 失焦后消失（这正是原来的 bug）"
fi

# ── 8. 等待用户关闭所有终端 ──
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  👉 请关闭 OneCode Desktop 中所有终端 tab"
echo "  然后回到此终端按 Enter 继续..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
read -r

# ── 9. 检查所有终端关闭后的状态 ──
info "===== 阶段 4：所有终端关闭 ====="
ASSERTIONS=$(pmset -g assertions 2>/dev/null)

DISP_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleDisplaySleep" || true)
if [ "$DISP_SLEEP" -eq 0 ]; then
    pass "PreventUserIdleDisplaySleep 已释放（正确：无终端时屏幕可熄灭）"
else
    fail "PreventUserIdleDisplaySleep 仍激活（预期：所有终端关闭后应释放）"
    echo "$ASSERTIONS" | grep "PreventUserIdleDisplaySleep"
fi

SYS_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleSystemSleep" || true)
if [ "$SYS_SLEEP" -ge 1 ]; then
    pass "PreventUserIdleSystemSleep 仍激活（正确：应用仍在运行）"
else
    fail "PreventUserIdleSystemSleep 丢失（应用应该还在）"
fi

# ── 10. 清理：关闭应用 ──
info "关闭应用..."
# 通过托盘退出（kill_all_blocking 会清理 assertion）
kill $TAURI_PID 2>/dev/null || true
sleep 2

# ── 11. 最终检查 ──
info "===== 阶段 5：应用退出后 ====="
ASSERTIONS=$(pmset -g assertions 2>/dev/null)

DISP_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleDisplaySleep" || true)
SYS_SLEEP=$(echo "$ASSERTIONS" | grep -c "PreventUserIdleSystemSleep" || true)

# 检查是否还有 OneCode 残留 assertion
ONECODE_ASSERTIONS=$(echo "$ASSERTIONS" | grep -c "OneCode" || true)
if [ "$ONECODE_ASSERTIONS" -eq 0 ]; then
    pass "应用退出后无 OneCode 残留 assertion"
else
    fail "应用退出后仍有 OneCode 残留 assertion"
    echo "$ASSERTIONS" | grep "OneCode"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
info "验证完成！"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
