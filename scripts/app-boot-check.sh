#!/usr/bin/env bash
# 启动冒烟检查：用无头 Chrome 加载**真实的 index.html + main.js**，把
# `window.__TAURI__` 换成桩，记录所有 invoke 与任何启动期错误。
#
# 为什么需要它：十六刀里前端被反复改动（main/palette/index/styles/work-panel/
# ipc-bridge），而单元测试只覆盖纯函数 —— **没有任何东西能证明 app 还能启动**。
# 一个模块在加载时抛错、一个 import 名字写错、一个 DOM id 不存在，单元测试全绿
# 也照样发生。这个检查就是那道网。
set -euo pipefail

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
# 自动挑一个空闲端口：固定端口在连续跑时会撞上上一轮遗留的服务，
# 而那时 Chrome 会连到**旧的**服务上 —— 看起来"通过"，其实测的是上上轮的内容。
if [ -z "${PORT:-}" ]; then
  PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HARNESS="$ROOT/tests/render/_app-boot.html"

[ -x "$CHROME" ] || { echo "找不到 Chrome：$CHROME（可用 CHROME=... 覆盖）" >&2; exit 1; }

# 生成夹具：把桩脚本插到 main.js 的 module 标签之前
python3 - "$ROOT" > "$HARNESS" <<'PY'
import sys, pathlib
root = pathlib.Path(sys.argv[1])
html = (root / 'src/index.html').read_text()
stub = (root / 'tests/render/_tauri-stub.html').read_text()
marker = '<script type="module" src="main.js"></script>'
assert marker in html, 'index.html 里找不到 main.js 的 module 标签'
# 相对路径要改成从 tests/render/ 出发
html = html.replace('src="static/', 'src="../../src/static/')
html = html.replace(marker, stub + '\n  ' + marker.replace('src="main.js"', 'src="../../src/main.js"'))
sys.stdout.write(html)
PY

# 先清掉可能残留的同端口服务（否则上一轮的 server 会继续服务，看起来"通过"）
pkill -f "http.server $PORT" 2>/dev/null || true
sleep 0.3
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$ROOT" >/dev/null 2>&1 &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true; rm -f "$HARNESS"' EXIT
# 等端口**真的应答**再开 Chrome：`sleep 1` 在负载下不可靠，
# 页面没加载就没有 bootlog —— 那会让检查随机失败（看起来像产品问题）。
for _ in $(seq 1 50); do
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/tests/render/work-panel.html" && break
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/src/index.html" && break
  sleep 0.1
done

PROF="$(mktemp -d)"

# ── 观察与判定分开 ───────────────────────────────────────────────────
# `--dump-dom` 偶尔会在夹具把 bootlog 写进 DOM **之前**就落盘（观察失败）。
# 那种情况**不是产品失败**，重试是合理的；但一旦观察到了内容，就按内容严格判定，
# **不允许重试掩盖真实 FAIL** —— 否则这个检查就成了"多试几次总能过"的摆设。
ATTEMPTS=4
for attempt in $(seq 1 $ATTEMPTS); do
  DUMP="$("$CHROME" --headless=old --disable-gpu --no-sandbox --disable-dev-shm-usage \
    --disable-crash-reporter --no-first-run --user-data-dir="$PROF" \
    --virtual-time-budget=20000 --dump-dom \
    "http://127.0.0.1:$PORT/tests/render/_app-boot.html" 2>/dev/null || true)"
  VERDICT="$(printf '%s' "$DUMP" | python3 -c "
import re, html, sys
d = sys.stdin.read()
m = re.search(r'id=\"bootlog\"[^>]*>(.*?)</pre>', d, re.S)
if not m:
    print('NOT_OBSERVED')
    sys.exit(0)
out = html.unescape(m.group(1)).strip()
print(out)
bad = [l for l in out.splitlines() if l.startswith('ERROR') or l.startswith('FAIL') or l.startswith('UNHANDLED')]
ok = ('BOOT_ERRORS: 0' in out and 'INVOKES:' in out
      and 'RESULT: ALL_FLOW_CHECKS_PASS' in out)
print('EXITCODE: ' + ('0' if (ok and not bad) else '1'))
")"
  if [ "$VERDICT" = "NOT_OBSERVED" ]; then
    echo "（第 $attempt 次：没能观察到 bootlog —— 观察失败，重试）"
    sleep 0.5
    continue
  fi
  echo "$VERDICT" | grep -v '^EXITCODE:'
  code="$(echo "$VERDICT" | sed -n 's/^EXITCODE: //p')"
  exit "$code"
done

echo "观察失败 $ATTEMPTS 次：bootlog 始终没出现（**不能据此判定产品有问题**）" >&2
exit 2
