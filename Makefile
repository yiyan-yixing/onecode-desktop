# OneCode Desktop — 构建/测试/打包的统一入口
#
# 用法：
#   make install-pkgs # 首次：装 npm 依赖
#   make build        # 开发构建（debug）
#   make release      # 生产构建（release + DMG）
#   make install      # 生产构建 + 安装到 /Applications
#   make dev          # 启动 tauri dev（热重载）
#   make test         # 运行 Rust 单元测试
#   make clean        # 清理构建产物
#   make check        # 快速检查：clippy + fmt + test
#
# ⚠️ 关键：~/.local/bin 下可能有 claude 创建的 cc/gcc 符号链接，
#    会遮蔽系统的 clang 导致链接报错（unknown option '-lSystem'）。
#    这里强制用 xcrun 的 clang，并把系统路径放在 PATH 最前。

# 强制使用 Apple 官方工具链（xcrun 解析正确的 clang/SDK）
export CC := $(shell xcrun --find clang 2>/dev/null || echo /usr/bin/cc)
export CXX := $(shell xcrun --find clang++ 2>/dev/null || echo /usr/bin/c++)
export SDKROOT := $(shell xcrun --sdk macosx --show-sdk-path 2>/dev/null)

# 确保系统工具链优先于 ~/.local/bin
export PATH := /usr/bin:/bin:/usr/sbin:/sbin:/Library/Developer/CommandLineTools/usr/bin:$(PATH)

ROOT := $(shell pwd)
TAURI_DIR := $(ROOT)/src-tauri
DMG := $(ROOT)/target/release/bundle/dmg/OneCode Desktop_0.1.0_aarch64.dmg

.PHONY: install-pkgs build release install dev test clean check static fmt fmt-check clippy help

help:
	@echo "OneCode Desktop Makefile"
	@echo ""
	@echo "  make install-pkgs 首次：装 npm 依赖"
	@echo "  make build        开发构建（debug）"
	@echo "  make release      生产构建（release）"
	@echo "  make install      生产构建 + 安装到 /Applications"
	@echo "  make dev          启动 tauri dev（热重载）"
	@echo "  make test         运行 Rust 单元测试"
	@echo "  make check        clippy + fmt 检查 + 测试"
	@echo "  make static       同步 xterm.js 等静态资源"
	@echo "  make clean        清理构建产物"
	@echo ""
	@echo "CC=$(CC)"

install-pkgs:
	cd $(ROOT) && npm install

static:
	cd $(ROOT) && npm run copy-static

build: static
	cd $(TAURI_DIR) && cargo build

dev: install-pkgs static
	cd $(ROOT) && npx tauri dev

test: static
	cd $(TAURI_DIR) && cargo test

fmt:
	cd $(TAURI_DIR) && cargo fmt

fmt-check:
	cd $(TAURI_DIR) && cargo fmt -- --check

clippy:
	cd $(TAURI_DIR) && cargo clippy -- -D warnings

check: fmt-check clippy test

release: static
	cd $(ROOT) && npx tauri build

install: release
	@echo ">>> 安装到 /Applications..."
	-hdiutil detach "/Volumes/OneCode Desktop" 2>/dev/null || true
	hdiutil attach "$(DMG)"
	cp -R "/Volumes/OneCode Desktop/OneCode Desktop.app" /Applications/
	hdiutil detach "/Volumes/OneCode Desktop"
	@echo ">>> 完成。可从启动台或 /Applications 打开 OneCode Desktop"

clean:
	cd $(TAURI_DIR) && cargo clean
