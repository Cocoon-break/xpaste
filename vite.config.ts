import { defineConfig, searchForWorkspaceRoot, type Plugin } from "vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import fs from "node:fs";
import path from "node:path";
const host = process.env.TAURI_DEV_HOST;

// pro/ 可能是符号链接（比如在 git worktree 里指向主检出的 pro/）。默认解析会换成真实路径：页面落到项目根目录外面，
// pro/ 里 `../../../src/…` 的相对引用还会指到主检出的 src/。所以只对 pro/ 里的文件保留链接所在的路径（别的依赖照常按 pnpm 的真实路径解析），
// pro/ 里的依赖也就从链接所在处往上找 node_modules
const proLink = path.resolve("pro");
const proReal = fs.existsSync(proLink) ? fs.realpathSync(proLink) : proLink;
const unlink = (id: string) => (id.startsWith(proReal + path.sep) ? proLink + id.slice(proReal.length) : id);
const proSymlink: Plugin = {
  name: "pro-symlink",
  enforce: "pre",
  async resolveId(id, importer, opts) {
    if (proReal === proLink) return null;
    const r = await this.resolve(id, importer && unlink(importer), { ...opts, skipSelf: true });
    return r && { ...r, id: unlink(r.id) };
  },
};

// https://vite.dev/config/
export default defineConfig(() => ({

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  plugins: [proSymlink],
  // 面板、截图遮罩（每屏一个）、贴图、录屏边框、划词卡片各一个页面；带 pro/ 时再加晾衣绳、传送带、搅拌机页面
  build: {
    rollupOptions: {
      input: {
        main: "index.html", shot: "shot.html", pin: "pin.html", recFrame: "rec-frame.html", card: "card.html",
        ...(fs.existsSync("pro/web/line.html") ? { line: "pro/web/line.html" } : {}),
        ...(fs.existsSync("pro/web/belt.html") ? { belt: "pro/web/belt.html" } : {}),
        ...(fs.existsSync("pro/web/mixer.html") ? { mixer: "pro/web/mixer.html" } : {}),
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
    // pro/ 是符号链接时真实路径在项目根目录外，开发服务器要放行
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), proReal] },
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
