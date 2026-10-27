# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

xPaste：Tauri 2 写的 macOS / Windows 剪切板历史工具。仓库里的文档、代码注释、日志和提交信息都用中文。

## 常用命令

所有操作走 `make`（CI 调用同样的目标），`make help` 列出全部目标。

```sh
make install        # pnpm install --frozen-lockfile
make dev-isolated   # 开发运行；数据目录隔离在 .xpaste-dev/，开启唤起与搜索计时
make check          # 提交前跑：fmt --check + clippy -D warnings（debug 与 release 各一遍）+ 版本号一致性 + Rust/前端单元测试 + 前端构建
make test           # 只跑单元测试（cargo test --workspace + pnpm test）
make build-check    # 编译应用但不打安装包（CI 跑这个）
make release        # 打未签名安装包，收拢到 target/installers/
```

跑单个测试：

```sh
cargo test -p xpaste-core pinyin::            # 按模块路径过滤
cargo test -p xpaste-core --lib store::tests::search_
cargo test -p xpaste-platform --test ocr      # 集成测试文件
node --test src/view.test.ts                  # 前端：Node 原生跑 .ts（类型剥离），没有构建步骤
```

- 运行应用只用 `make dev-isolated`，不要用 `make dev`：后者读写真实数据目录（`~/Library/Application Support/xPaste/`），还会和已安装的正式版抢单实例锁。
- `make lint` 在装了 `x86_64-pc-windows-msvc` target 时会额外对 `xpaste-platform` 做 Windows 交叉 clippy。改 `windows.rs` / `winfmt.rs` 后最好装上这个 target 跑一遍，否则只能等 CI 的 Windows 任务发现问题。
- 前端测试的 import 必须带 `.ts` 扩展名（`from './format.ts'`），否则 Node 找不到模块。

## 架构

Cargo workspace 有三个 crate，依赖方向是单向的：

- `crates/xpaste-core`：纯逻辑，不依赖 Tauri 和系统接口。
  - 内容类型识别（`detect`）、快照入库规则（`capture`）、数据模型（`model`）、拼音索引（`pinyin`）、SQLite 存储（`store`）。
  - `store` 放在默认开启的 `store` feature 后面。workspace 依赖声明里把它关掉了，所以 `xpaste-platform` 只拿到模型类型，Windows 交叉 clippy 不需要编译 SQLite。
- `crates/xpaste-platform`：系统接口，不依赖 Tauri。
  - 剪切板监听与读写、模拟粘贴、前台应用、系统文字识别（macOS Vision / Windows.Media.Ocr）、视频元数据。
  - 两端分别在 `macos.rs` 和 `windows.rs` 里，按 `cfg` 导出同名函数。`winfmt.rs` 解析 Windows 剪切板格式，平台无关，在 macOS 上也能测。
- `src-tauri`：应用层，负责把上面两层接起来。
  - 采集管道在 `clipboard.rs`：平台层回调 → 读快照 → 跳过自身写入（`is_self_write`）→ `capture` 入库 → 发事件给前端 → 把耗时任务投给后台线程。
  - 后台线程有两个：`enrich.rs` 生成缩略图、视频封面和应用图标，`ocr.rs` 做文字识别与补识别。
  - `panel.rs` 管面板窗口，macOS 上用 `tauri-nspanel`。`commands/` 是暴露给前端的命令，`events.rs` 定义事件名。

前端 `src/` 是原生 TypeScript + Vite，没有框架。

- `main.ts` 是面板的总控（状态 `S`、渲染、键盘处理）。
- 其余模块大多是可单测的纯函数（`view`、`format`、`keymap`、`virtual-list`、`spectrum` 等），各自配有 `*.test.ts`。
- **只有 `src/api/` 能调用 Tauri**（`commands.ts` 封装命令，`events.ts` 订阅事件，`types.ts` 与 Rust 的序列化结构对应）。改 Rust 侧结构时要同步改 `types.ts`。

### 存储与搜索（`crates/xpaste-core/src/store.rs`）

- **迁移**：迁移脚本按 `user_version` 排在数组里，第 i 项把版本从 i 升到 i+1。新增迁移只往数组末尾追加，并照着现有测试的写法补一个从旧版本升级的测试。
- **按长度分两条路**：3 个字符及以上的查询走 FTS5 trigram 外部内容表（由触发器维护）；1–2 个字符按顺序扫描。
- **拼音搜索**：
  - 走单独的 `fts_py` 表（`py_text` 列，由 `pinyin::index_text` 生成）。
  - 查询为全拼时，由 `pinyin::match_expr` 切分音节；恰好 2 个字母时，只匹配标题的首字母。
  - 命中后用 `SearchHit.mark` 告诉前端该高亮哪几个汉字。
  - 规则见规格说明 M05 规则 11、12。

### 横切约束

- **日志里不能出现剪切板内容、标题、文件路径和搜索词**，只记字节数、记录 id、来源应用名之类。需要排查细节时用 `log::debug!`，代码里已有这个用法。
- **代码注释引用规格编号**，如 “M01 规则 5”“AC-M18-01”，对应 `docs/xPaste-功能规格说明.md`。改行为时先查规格说明，行为变了要同步改规格说明。
- **Windows 的两个坑**，都已经修好，改动相关代码时别破坏：
  - `src-tauri/build.rs` 自己把 Common Controls v6 清单链进所有目标，包括测试程序。否则测试程序在 Windows 上会以 `STATUS_ENTRYPOINT_NOT_FOUND` 启动失败。
  - `windows.rs` 的 `ComGuard::init` 用 `CoIncrementMTAUsage` 保住进程级 MTA。否则反复初始化、释放 COM 后，缓存的 WinRT 工厂会变成悬垂指针。
- **版本号只写两处**：`package.json` 和根 `Cargo.toml` 的 `[workspace.package]`。`tauri.conf.json` 从 `package.json` 读版本，`pnpm check:version` 检查两处一致。

## 协作与发布

- **GitHub flow**：`main` 受保护，必须开 PR，并且 macOS、Windows 两个 CI 检查都通过才能合并。改动都从 `main` 拉短分支（`feat/…`、`fix/…`、`chore/…`、`docs/…`）。
- **发版**：在 `main` 上推送 `v<版本号>` 标签，由 `.github/workflows/release.yml` 打三个安装包，并另存一份不带版本号的文件名，供 `releases/latest/download/` 固定链接使用。
- **官网**：`website/` 是纯静态页面。改动合并到 `main` 后，由 `.github/workflows/pages.yml` 发布到 https://cocoon-break.github.io/xpaste/ 。
- **文档位置**：
  - 产品规格：`docs/xPaste-功能规格说明.md`，带版本记录。
  - 设计文档：放在 `docs/specs/`。
  - 实施计划：放在 `docs/plans/`，文件名以日期开头。
  - 真机验收：记在 `docs/验收记录.md`。
  - 已知限制：`docs/已知限制.md`，README 只列要点。行为或限制有变化时同步更新。
  - 开发与发版说明：`CONTRIBUTING.md`。
