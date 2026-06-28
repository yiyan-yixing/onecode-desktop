#!/usr/bin/env bash
#
# 从 node_modules 同步 xterm.js + addons + marked + highlight.js 到 src/static/。
# 前提：仓库根目录已执行 `npm install`。
#
# 用途：Tauri 打包时 frontendDist = src/，静态资源必须在其内。
# src/static/ 已 gitignore（生成物），不入库。
#
# 注意：桌面应用不需要 source map，拷贝时自动剥离 sourceMappingURL 行，
#       消除浏览器控制台 404 警告。
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/node_modules"
DST="$ROOT/src/static"

mkdir -p "$DST"

# 拷贝 JS 文件并剥离 sourceMappingURL（桌面应用不需要 source map，
# 保留只会让浏览器按文件名查找 .map 而 404）。
cp_if() {
    if [ -e "${1%.js}.min.js" ]; then
        sed '/\/\/# sourceMappingURL=/d' "${1%.js}.min.js" > "$2"
    elif [ -e "$1" ]; then
        sed '/\/\/# sourceMappingURL=/d' "$1" > "$2"
    else
        echo "✗ 缺失依赖文件：$1" >&2
        echo "  请先在仓库根目录执行：npm install" >&2
        exit 1
    fi
    echo "✓ $(basename "$2")"
}

echo "→ 同步静态资源到 src/static/ ..."

# ── xterm 核心（终端渲染） ──
cp_if "$SRC/@xterm/xterm/lib/xterm.js" "$DST/xterm.min.js"
cp "$SRC/@xterm/xterm/css/xterm.css" "$DST/xterm.css"
echo "✓ xterm.css"

# ── addons ──
cp_if "$SRC/@xterm/addon-fit/lib/addon-fit.js" "$DST/addon-fit.min.js"
cp_if "$SRC/@xterm/addon-web-links/lib/addon-web-links.js" "$DST/addon-web-links.min.js"
cp_if "$SRC/@xterm/addon-webgl/lib/addon-webgl.js" "$DST/addon-webgl.min.js"

# ── markdown / 代码高亮（Preview 模块用，M2+） ──
sed '/\/\/# sourceMappingURL=/d' "$SRC/marked/marked.min.js" > "$DST/marked.min.js"
echo "✓ marked.min.js"
# 完整版 highlight.js（文件浏览器预览需要多语言高亮）
if [ -e "$SRC/highlight.js/lib/highlight.min.js" ]; then
  cp "$SRC/highlight.js/lib/highlight.min.js" "$DST/highlight.min.js"
  echo "✓ highlight.min.js (full)"
elif [ -e "$SRC/highlight.js/highlight.min.js" ]; then
  cp "$SRC/highlight.js/highlight.min.js" "$DST/highlight.min.js"
  echo "✓ highlight.min.js (full)"
else
  # 兜底：使用 core 版本
  cp "$SRC/highlight.js/lib/core.js" "$DST/highlight.min.js"
  echo "✓ highlight.min.js (core fallback)"
fi

# hljs 主题（兼容 .min.css / .css 两种发布）
if [ -e "$SRC/highlight.js/styles/github-dark.min.css" ]; then
    cp "$SRC/highlight.js/styles/github-dark.min.css" "$DST/hljs.css"
    echo "✓ hljs.css"
elif [ -e "$SRC/highlight.js/styles/github-dark.css" ]; then
    cp "$SRC/highlight.js/styles/github-dark.css" "$DST/hljs.css"
    echo "✓ hljs.css"
fi

# 清理上次拷贝遗留的 .map 文件（不再需要）
find "$DST" -name '*.map' -delete 2>/dev/null && echo "✓ 已清理 .map 文件"

COUNT=$(find "$DST" -type f | wc -l | tr -d ' ')
echo "→ 完成：$COUNT 个文件已同步到 src/static/"
