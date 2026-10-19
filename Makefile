# xPaste 开发入口。CI 调用同样的目标，本地与流水线行为一致。
# 用法：make help

SHELL := /bin/bash
.DEFAULT_GOAL := help

# Windows 交叉检查只在装了该 target 时执行（rustup target add x86_64-pc-windows-msvc）
WIN_TARGET := x86_64-pc-windows-msvc
HAS_WIN_TARGET := $(shell rustup target list --installed 2>/dev/null | grep -c $(WIN_TARGET))

.PHONY: help install dev dev-isolated fmt lint test-rust test-web test check build build-check release release-app icons fonts clean

help: ## 列出可用目标
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{ printf "  %-14s %s\n", $$1, $$2 }'

install: ## 安装前端依赖
	pnpm install --frozen-lockfile

dev: ## 启动开发版（热更新）
	pnpm tauri dev

dev-isolated: ## 启动开发版，数据目录隔离在 .xpaste-dev/（自动化验收必须用这个）
	XPASTE_DATA_DIR=$(CURDIR)/.xpaste-dev pnpm tauri dev

fmt: ## 格式化 Rust 代码
	cargo fmt --all

lint: ## 格式检查 + clippy（含 Windows 交叉检查，若已安装 target）+ 版本号一致性
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --workspace --release -- -D warnings
ifneq ($(HAS_WIN_TARGET),0)
	cargo clippy -p xpaste-platform --all-targets --target $(WIN_TARGET) -- -D warnings
endif
	pnpm check:version

test-rust: ## Rust 单元测试（三个 crate）
	cargo test --workspace

test-web: ## 前端单元测试（自动发现 src/**/*.test.ts）
	pnpm test

test: test-rust test-web ## 全部单元测试

check: lint test build ## 全量检查：lint + 测试 + 前端构建（提交前跑这个）

build: ## 构建前端（tsc + vite）
	pnpm build

build-check: ## 编译应用但不打安装包（CI 用）
	pnpm tauri build --no-bundle

# CI=true：Tauri 打 dmg 时检测到它会给 bundle_dmg.sh 加 --skip-jenkins，跳过 Finder AppleScript
# 窗口布局（阶段 1 在这里超时 -1712），无界面环境也能完成；代价是 dmg 打开后没有图标摆位，
# “应用程序”快捷方式仍在。
release: ## 打发布包（未签名）：macOS 产出 .dmg（Tauri 打完会清掉 bundle/macos 下的 .app），Windows 产出 NSIS 安装程序
	CI=true pnpm tauri build
	@$(MAKE) --no-print-directory release-report

release-app: ## 只打 .app（macOS，测量用，比 release 快）
	pnpm tauri build --bundles app
	@$(MAKE) --no-print-directory release-report

release-report: ## 列出产物及大小
	@echo "== 产物 =="
	@find target/release/bundle -maxdepth 2 \( -name '*.app' -o -name '*.dmg' -o -name '*.exe' -o -name '*.msi' \) 2>/dev/null | while read -r f; do \
	  h=$$(du -sh "$$f" | cut -f1); \
	  if [ -d "$$f" ]; then echo "$$f  $$h  $$(du -sk "$$f" | cut -f1) KiB"; \
	  else echo "$$f  $$h  $$(wc -c < "$$f" | tr -d ' ') B"; fi; \
	done || true

icons: ## 从 docs/brand 的 SVG 重新生成应用图标和托盘图标（需要 rsvg-convert、ImageMagick）
	@tmp=$$(mktemp -d) && trap 'rm -rf $$tmp' EXIT && \
	rsvg-convert -w 1024 -h 1024 docs/brand/xpaste-logo.svg -o $$tmp/logo.png && \
	pnpm tauri icon $$tmp/logo.png && \
	rm -rf src-tauri/icons/android src-tauri/icons/ios src-tauri/icons/64x64.png && \
	for s in 16 24 32; do rsvg-convert -w $$s -h $$s docs/brand/tray-windows.svg -o $$tmp/s$$s.png; done && \
	for s in 48 64 256; do rsvg-convert -w $$s -h $$s docs/brand/xpaste-logo.svg -o $$tmp/s$$s.png; done && \
	magick $$tmp/s{16,24,32,48,64,256}.png src-tauri/icons/icon.ico && \
	mkdir -p src-tauri/icons/tray && \
	for f in macos macos-paused windows windows-paused; do rsvg-convert docs/brand/tray-$$f.svg -o src-tauri/icons/tray/$$f.png; done

fonts: ## 重新下载内置字体（latin 子集 woff2）与 OFL 许可证到 src/fonts
	bash scripts/fetch-fonts.sh

clean: ## 清理构建产物
	cargo clean
	rm -rf dist
