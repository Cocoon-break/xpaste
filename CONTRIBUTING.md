# 参与开发

欢迎提 Issue 和 Pull Request。下面是本地开发、提交改动和发版需要知道的事。

## 环境要求

- Node.js 和 pnpm（版本见 `package.json` 的 `packageManager`）。CI 用 Node 25；Node 25 已于 2026-06 停止维护，下一次升级选 24 LTS 或 26。
- Rust ≥ 1.90（`rustup` 安装，需要 `rustfmt`、`clippy`）。
- macOS 13+ 或 Windows 10 21H2 / 11。Tauri 的系统依赖见 [Tauri 前置条件](https://tauri.app/start/prerequisites/)。
- 在 macOS 上构建需要 macOS 26 及以上的 SDK（Xcode 26+ 或对应的 Command Line Tools）：系统翻译的 Swift 桥接要编译 Translation 框架的接口（规格说明 M26 规则 7）。CI 的 macOS 任务会选用 Xcode 26 及以上。编出的应用仍支持 macOS 13：`Translation` 与下载语言包用到的 `_Translation_SwiftUI` 都是弱链接，SwiftUI、AppKit 正常链接（`crates/xpaste-platform/build.rs`），改链接方式后用 `otool -L` 确认这两项仍是 weak。

## 常用命令

统一通过 `make` 操作，CI 跑的是同样的目标，`make help` 列出全部：

```sh
make install       # 安装依赖
make dev-isolated  # 开发运行，数据目录隔离在 .xpaste-dev/，开启唤起与搜索计时（推荐，不碰你真实的剪切板历史）
make dev           # 开发运行，使用真实数据目录
make check         # 提交前全量检查：格式、clippy、Rust + 前端单元测试、前端构建
make test          # 只跑单元测试
make measure       # macOS 性能与可靠性测量（先 make release-app；离开电脑时运行，约 15 分钟；终端要有“辅助功能”权限，脚本用 osascript 发按键）
make release       # 打发布包（本地默认不签名，见“签名与公证”），收拢到 target/installers/
make release-x64   # 在 Apple 芯片 Mac 上打 Intel 版 .dmg（需 rustup target add x86_64-apple-darwin；装了之后 make release 也会顺带打）
make release-win   # 在 macOS 上交叉打 Windows 安装程序（见下文）
```

## 排查长截图

开发构建里设 `XPASTE_LONG_DUMP=<目录> make dev-isolated`，长截图时每一帧按序号存成 PNG（正式构建不存）；用 `cargo run -p xpaste-core --example stitch_replay -- <目录> [--scale 2] [--out x.png]` 离线重放拼接，输出没接上的帧数、长图高度和最慢一帧的耗时，`--out` 把长图存成文件。存下的帧含屏幕内容，别提交进仓库。

## 目录结构

| 目录 | 内容 |
|---|---|
| `crates/xpaste-core` | 核心逻辑：类型识别、采集规则、SQLite 存储与搜索、拼音索引。不依赖 Tauri |
| `crates/xpaste-platform` | macOS / Windows 系统接口：剪切板、模拟粘贴、文字识别、视频元数据 |
| `src-tauri` | Tauri 应用层：把核心和平台接起来，管理面板窗口、托盘、快捷键、后台任务 |
| `src` | 面板前端（TypeScript + Vite，无框架），`src/api/` 是唯一调用 Tauri 的目录 |
| `website` | 官网（纯静态页面），合并到 `main` 后自动发布到 GitHub Pages |
| `docs` | 功能规格说明、技术选型、设计文档（`specs/`）、实施计划（`plans/`）、验收记录 |

改行为之前先看 [功能规格说明](docs/xPaste-功能规格说明.md)，代码注释里的 “M05 规则 11” 这类编号都指向它。

两条硬约束：

- **日志里不能出现剪切板内容、标题、文件路径和搜索词。**
- **数据只存在本机。** 不加联网、同步或统计功能。唯一的例外是用户手动触发的翻译（M26），只发送要翻译的那段文字。

## 分支与协作

采用 GitHub flow：

- `main` 是唯一的长期分支，随时可发布。它受保护，不能直接推送。
- 每个改动从 `main` 拉一个短分支（如 `feat/pinyin-search`、`fix/windows-ci`），推上去后开 Pull Request 到 `main`。外部贡献者先 fork 仓库再开 PR。
- PR 上的 CI 在 macOS 与 Windows 上各跑一遍 `make check`，两项都通过才能合并。发布版的完整编译（LTO，较慢）只在合并进 `main` 后由 `make release` 做；怀疑改动只影响发布版时，本地先跑 `make build-check`。合并后删掉分支。

## 发版

`.github/workflows/release.yml` 在推送 `v*` 标签时打 Apple 芯片、Intel、Windows 三个安装包，并发布 GitHub Release。每个安装包另存一份不带版本号的文件名（`xPaste_aarch64.dmg`、`xPaste_x64.dmg`、`xPaste_x64-setup.exe`），官网和 README 用 `releases/latest/download/<文件名>` 固定链接到最新版。

1. 同步改 `package.json` 与根 `Cargo.toml` 中 `[workspace.package]` 的版本号（`pnpm check:version` 检查两者一致），走 PR 合并到 `main`。
2. 在 `main` 上打标签并推送：`git tag v0.2.1 && git push origin v0.2.1`。标签必须等于 `v` + 版本号，不一致时流程直接失败。
3. 给已有标签补发或重发：Actions → Release → Run workflow，填标签名。

发布之后，流程里的 `tap` 任务会更新 [Cocoon-break/homebrew-tap](https://github.com/Cocoon-break/homebrew-tap) 里 `Casks/xpaste.rb` 的版本号和两个 dmg 的 sha256，用户 `brew upgrade --cask xpaste` 就能升到新版。

- 它用的是 tap 仓库上一把有写权限的 deploy key，私钥存在本仓库的 Secret `TAP_DEPLOY_KEY`。没配置时只发警告，不影响发布。
- 只在这次发布的就是最新版时更新；给旧标签补发不会把 tap 退回旧版本。
- cask 里的其他内容（最低系统版本、卸载时删除的数据目录等）在 tap 仓库里手动改，`tap` 任务只动 `version` 和 `sha256` 三处。

## 官网

`website/` 原样发布到 GitHub Pages，没有构建步骤。首页上那几块会动、能操作的界面不是仿制品，是 `src/` 的真实前端加演示数据：

- 演示数据和 Tauri 接口的替身在 `scripts/site-demo/`（面板、划词卡片、截图遮罩各一个入口；截图遮罩用的是 `src/api/shot.ts` 里的开发替身）。
- `make site-demo` 把它们打包进 `website/demo/`，产物要一起提交。改了面板、卡片或截图遮罩的界面，或者改了演示数据之后跑一次。
- `scripts/site-demo/` 在 `tsconfig.json` 的检查范围里：`src/api/types.ts` 的结构变了而演示数据没跟上，`make check` 会报错。
- 演示页面的样式和脚本全部内联进 html，直接双击打开 `website/index.html` 也能看；演示加载成功之前只显示占位图。

## 在 macOS 上打 Windows 安装包

`tauri build` 只给当前系统打包，所以 Mac 上默认只有 `.dmg`。装好下面的工具后，`make release-win`（或 `make release`）会用 Tauri 的交叉编译把 Windows 安装程序也打出来，和其他安装包一起放到 `target/installers/`（原始位置 `target/x86_64-pc-windows-msvc/release/bundle/nsis/`）：

```sh
rustup target add x86_64-pc-windows-msvc
brew install nsis llvm lld     # makensis、clang-cl / llvm-rc、lld-link
cargo install cargo-xwin        # MSVC 运行库与 Windows SDK（首次运行下载约 1.5 GB 到 ~/.cache/cargo-xwin）
```

这是 Tauri 标为实验性的路径：产物没有在 Windows 上签名，WebView2 引导下载和安装脚本的行为还要在真实 Windows 上验证一次。正式发布用的是 CI 的 Windows 机器打出来的包。

## 签名与公证

- **macOS**：安装包用 Developer ID Application 证书签名并公证。
  - CI：`release.yml` 把仓库 Secrets 里的六个值传给 `make release`，Tauri 签名并公证 `.app`；之后再把 dmg 本身提交公证、附上票据（Tauri 只公证 `.app`）。最后检查 dmg 和里面的 `.app` 都带公证票据、系统放行，否则失败，不发布。
    - `APPLE_CERTIFICATE`：证书连同私钥导出的 `.p12`，base64 编码
    - `APPLE_CERTIFICATE_PASSWORD`：`.p12` 的导出密码
    - `APPLE_SIGNING_IDENTITY`：证书名称，如 `Developer ID Application: <名字> (<TEAM ID>)`
    - `APPLE_ID`：Apple 账号
    - `APPLE_PASSWORD`：App 专用密码（在 account.apple.com 生成，不是账号密码）
    - `APPLE_TEAM_ID`：团队 ID
  - 本地：钥匙串里装好证书，设置后四个环境变量再运行 `make release`。不设置时打出来的是未签名包。本地只公证 `.app`，dmg 本身没有公证；正式发布以 CI 打出的包为准。
  - 签名后开启 Hardened Runtime，录屏的麦克风靠 `src-tauri/Entitlements.plist` 里的 `com.apple.security.device.audio-input`；以后用到摄像头、Apple 事件之类受限能力时在这里加对应的项。
- **Windows**：还没有签名，拿到证书后：
  1. 把代码签名证书导入“当前用户 → 个人”证书存储。
  2. 在 `src-tauri/tauri.windows.conf.json` 的 `bundle.windows` 下加三项：`certificateThumbprint`（证书指纹）、`digestAlgorithm: "sha256"`、`timestampUrl`（证书颁发机构提供的时间戳服务地址）。
  3. 运行 `make release`。
