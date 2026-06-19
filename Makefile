.PHONY: install copy-static dev build clean check fmt

# ── 首次准备：装依赖 + 同步静态资源 + 生成图标 ───────────────────────
install:
	npm install

copy-static:
	npm run copy-static

# ── 开发：热重载运行 ────────────────────────────────────────────────
dev:
	npm run dev

# ── 构建：产出 dmg / AppImage ───────────────────────────────────────
build:
	npm run build

# ── Rust 校验（无需 Tauri 系统依赖即可跑 check） ─────────────────────
check:
	cd src-tauri && cargo check

fmt:
	cd src-tauri && cargo fmt

# ── 清理 ────────────────────────────────────────────────────────────
clean:
	rm -rf src-tauri/target dist
	rm -f static/* && touch static/.gitkeep
