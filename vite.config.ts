import { defineConfig } from "vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import fs from "node:fs";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // pro/ 可能是符号链接（比如在 git worktree 里）：按链接所在的路径解析，不换成真实路径，否则会落到项目根目录外面。
  // 这样 pro/ 里的依赖也从链接所在处往上找 node_modules；@tauri-apps 插件的间接依赖要靠 pnpm 提升到根目录才找得到（@tauri-apps/api 本身是直接依赖）
  resolve: { preserveSymlinks: true },
  // 面板、截图遮罩（每屏一个）、贴图、录屏边框、划词卡片各一个页面；带 pro/ 时再加晾衣绳、传送带页面
  build: {
    rollupOptions: {
      input: {
        main: "index.html", shot: "shot.html", pin: "pin.html", recFrame: "rec-frame.html", card: "card.html",
        ...(fs.existsSync("pro/web/line.html") ? { line: "pro/web/line.html" } : {}),
        ...(fs.existsSync("pro/web/belt.html") ? { belt: "pro/web/belt.html" } : {}),
      },
    },
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
