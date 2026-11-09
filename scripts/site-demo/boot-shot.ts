// 官网上的截图遮罩：真实的 shot.html，跑在它自带的开发替身上（冻结的是一张画出来的假桌面）。
// 没人动手时自己演一遍：拖出选区 → 画矩形 → 画箭头 → 回车复制，然后重来；访客一动手就把鼠标交给访客。
import { fromPage, tell } from './bridge';

const style = document.createElement('style');
style.textContent = `
html, body { background: transparent !important; }
#demo-cursor { position: fixed; left: 0; top: 0; z-index: 99999; width: 20px; height: 20px; pointer-events: none; opacity: 0; transition: opacity .2s; }
#demo-cursor.on { opacity: 1; }
`;
document.head.append(style);

// 脚本派发的指针事件没有真实指针，捕获会抛错：演示里忽略
for (const name of ['setPointerCapture', 'releasePointerCapture'] as const) {
  const real = Element.prototype[name];
  Element.prototype[name] = function (this: Element, id: number) {
    try { real.call(this, id); } catch { /* 脚本派发的事件 */ }
  };
}

const sleep = (ms: number) => new Promise((ok) => setTimeout(ok, ms));
const ov = () => document.getElementById('ov') as HTMLElement;

const cursor = document.createElement('div');
cursor.id = 'demo-cursor';
cursor.innerHTML = '<svg viewBox="0 0 20 20" width="20" height="20"><path d="M3 2l13 7-5.5 1.5L8 16z" fill="#fff" stroke="#111" stroke-width="1.2" stroke-linejoin="round"/></svg>';

let run = 0;
let at = { x: 0, y: 0 };
const fire = (type: string, x: number, y: number) =>
  ov().dispatchEvent(new PointerEvent(type, { bubbles: true, clientX: x, clientY: y, button: 0, buttons: type === 'pointerup' ? 0 : 1, pointerId: 1, pointerType: 'mouse', isPrimary: true }));
// 遮罩按 e.code 认数字键（Digit1…Digit7）
const key = (k: string) => dispatchEvent(new KeyboardEvent('keydown', { key: k, code: /^\d$/.test(k) ? `Digit${k}` : k, bubbles: true }));
const place = (x: number, y: number) => { at = { x, y }; cursor.style.transform = `translate(${x - 3}px, ${y - 2}px)`; };

/** 把假鼠标从当前位置移到 (x, y)；`down` 为真时一路派发拖动 */
async function glide(id: number, x: number, y: number, ms: number, down: boolean) {
  const from = { ...at }, steps = Math.max(8, Math.round(ms / 16));
  for (let i = 1; i <= steps; i++) {
    if (id !== run) return false;
    const t = i / steps, e = 1 - Math.pow(1 - t, 3);
    place(from.x + (x - from.x) * e, from.y + (y - from.y) * e);
    fire('pointermove', at.x, at.y);
    await sleep(ms / steps);
  }
  return down || true;
}
async function drag(id: number, x0: number, y0: number, x1: number, y1: number, ms: number) {
  if (!(await glide(id, x0, y0, 420, false))) return false;
  fire('pointerdown', x0, y0);
  const ok = await glide(id, x1, y1, ms, true);
  fire('pointerup', at.x, at.y);
  return ok;
}

async function play() {
  const id = ++run, W = innerWidth, H = innerHeight;
  document.body.append(cursor);
  place(W * 0.5, H * 0.55);
  cursor.classList.add('on');
  const step = async (ms: number) => { await sleep(ms); return id === run; };
  if (!(await step(900))) return;
  if (!(await drag(id, W * 0.3, H * 0.13, W * 0.8, H * 0.85, 1000))) return;
  if (!(await step(600))) return;
  key('1');
  if (!(await drag(id, W * 0.355, H * 0.3, W * 0.66, H * 0.46, 700))) return;
  if (!(await step(350))) return;
  key('3');
  if (!(await drag(id, W * 0.74, H * 0.66, W * 0.64, H * 0.49, 600))) return;
  if (!(await step(1100))) return;
  cursor.classList.remove('on');
  key('Enter');
}

// 一次截图结束（演示按了回车，或访客自己完成 / 按了 Esc）：遮罩撤下后重新开始
// 内联的模块脚本在 WebKit 里可能晚于 load 才执行：已经加载完就下一轮直接开始
const start = () => setTimeout(() => {
  let seen = false;
  new MutationObserver(() => {
    if (!ov().hidden) seen = true;
    else if (seen) setTimeout(() => location.reload(), 900);
  }).observe(ov(), { attributes: true, attributeFilter: ['hidden'] });
  tell({ type: 'ready' });
});
if (document.readyState === 'complete') start();
else addEventListener('load', start);

addEventListener('message', (e: MessageEvent) => {
  const m = fromPage(e);
  if (!m) return;
  if (m.type === 'theme') {
    const root = document.documentElement;
    if (m.value === 'light' || m.value === 'dark') root.dataset.theme = m.value;
    else delete root.dataset.theme;
  } else if (m.type === 'play') void play();
  else if (m.type === 'stop') { run += 1; cursor.classList.remove('on'); }
});

for (const name of ['pointerdown', 'keydown'] as const) {
  addEventListener(name, (e) => {
    if (!e.isTrusted) return;
    run += 1;
    cursor.classList.remove('on');
    tell({ type: 'interact' });
  }, { capture: true });
}
