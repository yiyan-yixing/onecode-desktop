#!/usr/bin/env bash
#
# 从 node_modules 同步 xterm.js + addons + marked + highlight.js 到 src/static/。
# 前提：仓库根目录已执行 `npm install`。
#
# 用途：Tauri 打包时 frontendDist = src/，静态资源必须在其内。
# src/static/ 已 gitignore（生成物），不入库。
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/node_modules"
DST="$ROOT/src/static"

mkdir -p "$DST"

require() {
    if [ ! -e "$1" ]; then
        echo "✗ 缺失依赖文件：$1" >&2
        echo "  请先在仓库根目录执行：npm install" >&2
        exit 1
    fi
}

cp_if() {
    require "$1"
    cp "$1" "$2"
    echo "✓ $(basename "$2")"
}

echo "→ 同步静态资源到 src/static/ ..."

# ── xterm 核心（终端渲染） ──
cp_if "$SRC/@xterm/xterm/css/xterm.css" "$DST/xterm.css"
cp_if "$SRC/@xterm/xterm/lib/xterm.min.js" "$DST/xterm.min.js"

# ── addons ──
cp_if "$SRC/@xterm/addon-fit/lib/addon-fit.min.js" "$DST/addon-fit.min.js"
cp_if "$SRC/@xterm/addon-web-links/lib/addon-web-links.min.js" "$DST/addon-web-links.min.js"
cp_if "$SRC/@xterm/addon-webgl/lib/addon-webgl.min.js" "$DST/addon-webgl.min.js"

# ── markdown / 代码高亮（Preview 模块用，M2+） ──
cp_if "$SRC/marked/marked.min.js" "$DST/marked.min.js"
cp_if "$SRC/highlight.js/highlight.min.js" "$DST/highlight.min.js"

# hljs 主题（兼容 .min.css / .css 两种发布）
if [ -e "$SRC/highlight.js/styles/github-dark.min.css" ]; then
    cp "$SRC/highlight.js/styles/github-dark.min.css" "$DST/hljs.css"
    echo "✓ hljs.css"
elif [ -e "$SRC/highlight.js/styles/github-dark.css" ]; then
    cp "$SRC/highlight.js/styles/github-dark.css" "$DST/hljs.css"
    echo "✓ hljs.css"
fi

COUNT=$(find "$DST" -type f | wc -l | tr -d ' ')
echo "→ 完成：$COUNT 个文件已同步到 src/static/"
