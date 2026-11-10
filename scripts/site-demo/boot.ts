// 官网页面与演示面板之间的桥。父页面用 postMessage 指挥：换主题、演示打字、点分类、缩放频谱、
// 把访客在页面上的复制记进来；面板把“访客上手了”告诉父页面（“准备好了”“粘贴了哪条”由 core.ts 发）。
import { fromPage, tell } from './bridge';
import { addCopied } from './core';

// 面板抢焦点时不带着外层页面滚动
const focus = HTMLElement.prototype.focus;
HTMLElement.prototype.focus = function (this: HTMLElement) {
  focus.call(this, { preventScroll: true });
};

// 同理，把某一行滚进视野时只动面板自己的滚动容器：原生的 scrollIntoView 会连外层页面一起滚
Element.prototype.scrollIntoView = function (this: Element) {
  let box = this.parentElement;
  while (box && !(box.scrollHeight > box.clientHeight + 1 && /auto|scroll/.test(getComputedStyle(box).overflowY))) box = box.parentElement;
  if (!box) return;
  const a = this.getBoundingClientRect(), b = box.getBoundingClientRect();
  if (a.top < b.top) box.scrollTop -= b.top - a.top;
  else if (a.bottom > b.bottom) box.scrollTop += a.bottom - b.bottom;
};

// 浏览器里没有系统材质：面板着色下垫一层接近系统材质的底色，圆角外保持透明
const style = document.createElement('style');
style.textContent = `
html, body { background: transparent !important; }
.window { background: linear-gradient(var(--glass), var(--glass)), #46464a !important; }
@media (prefers-color-scheme: light) { :root:not([data-theme="dark"]) .window { background: linear-gradient(var(--glass), var(--glass)), #f2f2f5 !important; } }
:root[data-theme="light"] .window { background: linear-gradient(var(--glass), var(--glass)), #f2f2f5 !important; }
`;
document.head.append(style);

const sleep = (ms: number) => new Promise((ok) => setTimeout(ok, ms));
const click = (sel: string) => document.querySelector<HTMLElement>(sel)?.click();

/** 演示打字：一个字一个字填进搜索框。`run` 变了说明被新的指令或访客打断。 */
let run = 0;
async function type(text: string) {
  const id = ++run;
  const el = document.querySelector<HTMLInputElement>('#q');
  if (!el) return;
  const set = (v: string) => {
    el.value = v;
    el.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const chars = [...text];
  set('');
  for (let i = 1; i <= chars.length; i++) {
    await sleep(/^[\x20-\x7e]*$/.test(text) ? 170 : 340);
    if (id !== run) return;
    set(chars.slice(0, i).join(''));
  }
}

/** 演示按键：在搜索框上发一次 keydown（Tab 把来源建议变成胶囊，⌫ 去掉胶囊，M29） */
const key = (k: string) => document.querySelector('#q')?.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));

function theme(v: string) {
  const root = document.documentElement;
  if (v === 'light' || v === 'dark') root.dataset.theme = v;
  else delete root.dataset.theme;
}

addEventListener('message', (e: MessageEvent) => {
  const m = fromPage(e);
  if (!m) return;
  const v = String(m.value ?? '');
  if (m.type === 'theme') theme(v);
  else if (m.type === 'type') void type(v);
  else if (m.type === 'key') key(v);
  else if (m.type === 'filter') click(`#tabsBase button[data-f="${CSS.escape(v)}"]`);
  else if (m.type === 'span') click(`#spPresets button[data-span="${CSS.escape(v)}"]`);
  else if (m.type === 'copied') addCopied(v);
});

// 访客真的动手了（脚本触发的事件 isTrusted 为 false）：停掉演示打字，并告诉父页面别再自动演示
for (const name of ['pointerdown', 'keydown', 'wheel'] as const) {
  addEventListener(name, (e) => {
    if (!e.isTrusted) return;
    run += 1;
    tell({ type: 'interact' });
  }, { capture: true, passive: true });
}
