// 官网上那几块能操作的界面：仓库 src/ 的真实前端 + 这里的演示数据，打包进 website/demo/。
// 重新生成：make site-demo（前端有改动、或改了这里的演示数据之后跑一次，产物要一起提交）。
// 不 import vite，纯对象配置；Tauri 的几个模块换成本目录里的替身。
//
// 每个页面单独打包一次（SITE_DEMO_ENTRY=index | card | shot），样式和脚本全部内联进 html：
// 访客直接双击打开 website/index.html（file://）时，浏览器不让加载单独的模块脚本和带 crossorigin 的样式，
// 内联之后不再有这类请求，双击打开和发布到 GitHub Pages 的效果一样。
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const entry = process.env.SITE_DEMO_ENTRY || 'index';
const boot = { index: 'boot.ts', card: 'boot-card.ts', shot: 'boot-shot.ts' };
if (!boot[entry]) throw new Error(`SITE_DEMO_ENTRY 只能是 ${Object.keys(boot).join(' / ')}，收到 ${entry}`);

/** 把打包出的 js、css 塞回 html，删掉单独的文件 */
const inline = {
  name: 'site-demo-inline',
  enforce: 'post',
  generateBundle(_opts, bundle) {
    for (const html of Object.values(bundle).filter((f) => f.fileName.endsWith('.html'))) {
      let src = String(html.source);
      src = src.replace(/<link rel="modulepreload"[^>]*>\n?/g, '');
      src = src.replace(/<script type="module" crossorigin src="\.\/([^"]+)"><\/script>/g, (_m, name) => {
        const code = bundle[name].code.replace(/<\/script/gi, '<\\/script');
        delete bundle[name];
        return `<script type="module">${code}</script>`;
      });
      src = src.replace(/<link rel="stylesheet" crossorigin href="\.\/([^"]+)">/g, (_m, name) => {
        const css = String(bundle[name].source);
        delete bundle[name];
        return `<style>${css}</style>`;
      });
      if (/src="\.\/assets\/|href="\.\/assets\//.test(src)) throw new Error(`${html.fileName} 里还有没内联的 assets 引用`);
      html.source = src;
    }
  },
};

/** 私有组件（pro/）不进官网：开发机上有 pro/ 时，指向它的 glob 换成空对象，其余引用解析成空模块；
 *  打包结束时还有 pro/ 里的模块就报错。make site-demo 打包后再 grep 一遍产物，见 Makefile */
const proDir = path.join(repo, 'pro') + path.sep;
const noPro = {
  name: 'site-demo-no-pro',
  enforce: 'pre',
  transform: (code, id) => (id.startsWith(repo) && code.includes('../pro/')
    ? code.replace(/import\.meta\.glob(<[\s\S]*?>)?\(\s*(['"])\.\.\/pro\/[^'"]*\2\s*\)/g, '({})') : null),
  resolveId: (src, from) => (from && path.resolve(path.dirname(from), src).startsWith(proDir) ? '\0site-demo-empty' : null),
  load: (id) => (id === '\0site-demo-empty' ? 'export {};' : null),
  buildEnd() {
    for (const id of this.getModuleIds()) if (id.startsWith(proDir)) this.error('pro/ 里的私有组件混进了官网演示');
  },
};

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
    noPro,
    {
      // 演示用的桥接脚本排在应用入口之前
      name: 'site-demo-boot',
      transformIndexHtml: {
        order: 'pre',
        handler: () => [{ tag: 'script', attrs: { type: 'module', src: `/scripts/site-demo/${boot[entry]}` }, injectTo: 'head-prepend' }],
      },
    },
    inline,
  ],
  build: {
    outDir: path.join(repo, 'website/demo'),
    // 三个页面依次打包进同一个目录，由 make site-demo 先清空
    emptyOutDir: false,
    modulePreload: false,
    cssCodeSplit: false,
    rollupOptions: {
      input: { [entry]: path.join(repo, `${entry}.html`) },
      output: { inlineDynamicImports: true, entryFileNames: 'assets/[name].js', assetFileNames: 'assets/[name][extname]' },
    },
  },
};
