// 官网上那个可以操作的面板：仓库 src/ 的真实前端 + 这里的演示数据，打包进 website/demo/。
// 重新生成：make site-demo（前端有改动、或改了这里的演示数据之后跑一次，产物要一起提交）。
// 不 import vite，纯对象配置；Tauri 的几个模块换成本目录里的替身。
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const boot = { 'index.html': 'boot.ts', 'card.html': 'boot-card.ts', 'shot.html': 'boot-shot.ts' };

export default {
  root: repo,
  base: './',
  publicDir: path.join(here, 'public'),
  clearScreen: false,
  // 截图遮罩用 src/api/shot.ts 里“只在开发时启用”的替身（画出来的假桌面），演示包里要留着它
  define: { 'import.meta.env.DEV': 'true' },
  resolve: {
    alias: {
      '@tauri-apps/api/core': path.join(here, 'core.ts'),
      '@tauri-apps/api/event': path.join(here, 'event.ts'),
      '@tauri-apps/api/window': path.join(here, 'window.ts'),
      '@tauri-apps/plugin-log': path.join(here, 'log.ts'),
      '@tauri-apps/plugin-dialog': path.join(here, 'dialog.ts'),
    },
  },
  plugins: [
    {
      // 演示用的桥接脚本排在应用入口之前
      name: 'site-demo-boot',
      transformIndexHtml: {
        order: 'pre',
        handler: (_html, ctx) => {
          const f = boot[path.basename(ctx.filename)];
          return f ? [{ tag: 'script', attrs: { type: 'module', src: `/scripts/site-demo/${f}` }, injectTo: 'head-prepend' }] : [];
        },
      },
    },
  ],
  build: {
    outDir: path.join(repo, 'website/demo'),
    emptyOutDir: true,
    rollupOptions: {
      input: { index: path.join(repo, 'index.html'), card: path.join(repo, 'card.html'), shot: path.join(repo, 'shot.html') },
      // 文件名不带哈希：每次重新生成只改内容，不在仓库里留下一串改名
      output: { entryFileNames: 'assets/[name].js', chunkFileNames: 'assets/[name].js', assetFileNames: 'assets/[name][extname]' },
    },
  },
  server: { port: 18767, strictPort: true, host: '127.0.0.1' },
};
