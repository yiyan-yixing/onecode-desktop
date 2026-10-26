#!/usr/bin/env bash
# 渲染检查：把工作台面板放进真实右栏结构里跑一遍，量化 + 视觉验证。
#
# 为什么需要它：单元测试只能证明"字符串排对了"，证明不了"页面上看起来是对的"。
# 第 11 刀留下了"渲染实测未做"，第 12 刀补上时立刻发现三个只有渲染才暴露的问题
# （markdown 星号进了 HTML、agent 自报完成的任务整个消失、空项目显示了陈旧内容）。
#
# 需要：Google Chrome（headless）+ python3（起本地静态服务；file:// 下
# Chrome 禁止 ES module 导入，所以必须走 HTTP）。
set -euo pipefail

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
# 自动挑一个空闲端口（理由同 app-boot-check.sh）
if [ -z "${PORT:-}" ]; then
  PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ ! -x "$CHROME" ]; then
  echo "找不到 Chrome：$CHROME（可用 CHROME=... 覆盖）" >&2
  exit 1
fi

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$ROOT" >/dev/null 2>&1 &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true' EXIT
# 等端口**真的应答**再开 Chrome：`sleep 1` 在负载下不可靠，
# 页面没加载就没有 bootlog —— 那会让检查随机失败（看起来像产品问题）。
for _ in $(seq 1 50); do
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/tests/render/work-panel.html" && break
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/src/index.html" && break
  sleep 0.1
done

PROF="$(mktemp -d)"
OUT="${OUT:-/tmp/onecode-work-panel.png}"

"$CHROME" --headless=old --disable-gpu --no-sandbox --disable-dev-shm-usage \
  --disable-crash-reporter --no-first-run --user-data-dir="$PROF" \
  --virtual-time-budget=5000 --dump-dom \
  "http://127.0.0.1:$PORT/tests/render/work-panel.html" 2>/dev/null \
  | python3 -c "
import re, html, sys
d = sys.stdin.read()
m = re.search(r'id=\"measure\"[^>]*>(.*?)</pre>', d, re.S)
out = html.unescape(m.group(1)).strip() if m else '(没找到 measure — 夹具没跑起来)'
print(out)
sys.exit(0 if 'ALL_CHECKS_PASS' in out else 1)
"

"$CHROME" --headless=old --disable-gpu --no-sandbox --disable-dev-shm-usage \
  --disable-crash-reporter --no-first-run --user-data-dir="$PROF" \
  --window-size=1000,780 --virtual-time-budget=5000 \
  --screenshot="$OUT" "http://127.0.0.1:$PORT/tests/render/work-panel.html" 2>/dev/null

echo "截图：$OUT"
