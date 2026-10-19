# xPaste

macOS / Windows 剪切板历史工具。

- 功能规格说明：[docs/xPaste-功能规格说明.md](docs/xPaste-功能规格说明.md)
- 交互原型：[docs/xpaste-prototype.html](docs/xpaste-prototype.html)（用浏览器直接打开）
- 技术选型：[docs/ADR-001-技术选型.md](docs/ADR-001-技术选型.md)
- 实现设计：[docs/specs/2026-10-18-xPaste-实现设计.md](docs/specs/2026-10-18-xPaste-实现设计.md)（设计文档放 `docs/specs/`，实施计划放 `docs/plans/`）

## 目录

- `crates/xpaste-core`：核心逻辑，不依赖 Tauri
- `crates/xpaste-platform`：macOS / Windows 系统接口
- `src-tauri`：Tauri 应用层
- `src`：面板前端，`src/api/` 是唯一调用 Tauri 的目录

## 开发

需要 Node.js、pnpm 和 Rust ≥ 1.90。统一通过 `make` 操作，`make help` 列出全部目标：

```sh
make install   # 安装依赖
make dev       # 开发运行
make check     # 提交前全量检查：格式、clippy、Rust + 前端单元测试、前端构建
make test      # 只跑单元测试
make release   # 打发布包（未签名）
```

## 分支

- `dev`：日常开发分支，所有改动先进这里。
- `main`：功能验收通过后才从 `dev` 合并。
