# xPaste

macOS / Windows 剪切板历史工具。

- 功能规格说明：[docs/xPaste-功能规格说明.md](docs/xPaste-功能规格说明.md)
- 交互原型：[docs/xpaste-prototype.html](docs/xpaste-prototype.html)（用浏览器直接打开）
- 技术选型：[docs/ADR-001-技术选型.md](docs/ADR-001-技术选型.md)
- 实现设计：[docs/specs/2026-10-18-xPaste-实现设计.md](docs/specs/2026-10-18-xPaste-实现设计.md)（设计文档放 `docs/specs/`，实施计划放 `docs/plans/`）

## 开发

需要 Node.js、pnpm 和 Rust ≥ 1.90。

```sh
pnpm install
pnpm tauri dev     # 开发运行
pnpm tauri build   # 打包安装包
```
