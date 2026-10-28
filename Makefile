# xPaste 开发入口。CI 调用同样的目标，本地与流水线行为一致。
# 用法：make help

SHELL := /bin/bash
.DEFAULT_GOAL := help

# Windows 交叉检查只在装了该 target 时执行（rustup target add x86_64-pc-windows-msvc）
WIN_TARGET := x86_64-pc-windows-msvc
HAS_WIN_TARGET := $(shell rustup target list --installed 2>/dev/null | grep -c $(WIN_TARGET))
# macOS 上交叉打 Windows 安装包的工具：brew install nsis llvm lld；cargo install cargo-xwin。
# llvm 是 keg-only，clang-cl / llvm-rc 不在 PATH 上，打包时临时加进去
LLVM_BIN := /opt/homebrew/opt/llvm/bin
# Apple 芯片上打 Intel 包只需 rustup target add x86_64-apple-darwin
MAC_X64 := x86_64-apple-darwin
HAS_MAC_X64 := $(shell test "$$(uname -sm)" = "Darwin arm64" && rustup target list --installed 2>/dev/null | grep -c $(MAC_X64) || echo 0)
HAS_XWIN := $(shell command -v cargo-xwin >/dev/null 2>&1 && command -v makensis >/dev/null 2>&1 && test -x $(LLVM_BIN)/clang-cl && command -v lld-link >/dev/null 2>&1 && echo 1 || echo 0)

.PHONY: help install dev dev-isolated fmt lint test-rust test-web test check build build-check release release-win release-x64 release-app release-report measure icons fonts clean

help: ## 列出可用目标
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{ printf "  %-14s %s\n", $$1, $$2 }'

install: ## 安装前端依赖
	pnpm install --frozen-lockfile

dev: ## 启动开发版（热更新）
	pnpm tauri dev

dev-isolated: ## 启动开发版，数据目录隔离在 .xpaste-dev/（自动化验收必须用这个）
	XPASTE_DATA_DIR=$(CURDIR)/.xpaste-dev XPASTE_TIMING=1 pnpm tauri dev

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

build-check: ## 按发布配置编译应用但不打安装包
	pnpm tauri build --no-bundle

# CI=true：Tauri 打 dmg 时检测到它会给 bundle_dmg.sh 加 --skip-jenkins，跳过 Finder AppleScript
# 窗口布局（阶段 1 在这里超时 -1712），无界面环境也能完成；代价是 dmg 打开后没有图标摆位，
# “应用程序”快捷方式仍在。
release: ## 打发布包（未签名）：macOS 产出 .dmg（Apple 芯片装了 x86_64 target 时再打 Intel 包，装了交叉工具时再打 Windows 安装程序），Windows 产出 NSIS 安装程序
	CI=true pnpm tauri build
ifeq ($(HAS_MAC_X64),1)
	CI=true pnpm tauri build --target $(MAC_X64)
endif
ifeq ($(HAS_XWIN),1)
	PATH="$(LLVM_BIN):$$PATH" CI=true pnpm tauri build --runner cargo-xwin --target $(WIN_TARGET)
endif
	@$(MAKE) --no-print-directory release-report

release-x64: ## 在 Apple 芯片 Mac 上打 Intel 版 .dmg（需 rustup target add x86_64-apple-darwin）
	CI=true pnpm tauri build --target $(MAC_X64)
	@$(MAKE) --no-print-directory release-report

release-win: ## 在 macOS 上交叉打 Windows NSIS 安装程序（需 brew install nsis llvm lld 与 cargo install cargo-xwin；首次会下载约 1.5 GB 的 Windows SDK 到 ~/.cache/cargo-xwin）
	PATH="$(LLVM_BIN):$$PATH" CI=true pnpm tauri build --runner cargo-xwin --target $(WIN_TARGET)
	@$(MAKE) --no-print-directory release-report

release-app: ## 只打 .app（macOS，测量用，比 release 快）
	pnpm tauri build --bundles app
	@$(MAKE) --no-print-directory release-report

INSTALLERS := target/installers
release-report: ## 把各目标的安装包收拢到 target/installers/ 并列出大小
	@rm -rf $(INSTALLERS) && mkdir -p $(INSTALLERS)
	@find target/release/bundle target/$(MAC_X64)/release/bundle target/$(WIN_TARGET)/release/bundle -maxdepth 2 \( -name '*.app' -o -name '*.dmg' -o -name '*.exe' -o -name '*.msi' \) 2>/dev/null | while read -r f; do \
	  if [ -d "$$f" ]; then echo "$$f  $$(du -sh "$$f" | cut -f1)"; else cp "$$f" $(INSTALLERS)/; fi; \
	done || true
	@echo "== 安装包（$(INSTALLERS)/）=="
	@ls -1 $(INSTALLERS) 2>/dev/null | while read -r f; do echo "  $$f  $$(du -sh $(INSTALLERS)/$$f | cut -f1)"; done

measure: ## macOS 性能与可靠性测量（先 make release-app；离开电脑时运行，约 15 分钟；终端要有辅助功能权限）
	bash scripts/measure.sh all

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
