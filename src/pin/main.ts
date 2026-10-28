// 贴图页面（pin.html），每张贴图一个窗口，窗口即图片（M23 规则 9）。交互对照原型 docs/specs/2026-10-28-截图与录屏-原型.html
// 的 makePin：拖动移动、滚轮以光标为锚缩放、双击或 Esc 关闭、右键菜单。坐标为屏幕逻辑坐标。
import './pin.css';
import {
  onPinMoved, pinButtonDown, pinClose, pinCopy, pinImageSrc, pinMenu, pinPosition, pinSave, pinSetBounds, pinStartDrag, pinWorkArea,
  type MenuEntry,
} from '../api/shot';
import { clampPos, maxZoom, wheelSteps, zoomAt, type Wheel } from './zoom';

const $ = (id: string) => document.getElementById(id)!;
const pin = $('pin'), img = $('img') as HTMLImageElement, chip = $('zoom');
const q = new URLSearchParams(location.search);
/** 原始尺寸（选区的逻辑尺寸）；当前尺寸 = 原始 × k */
const W0 = Number(q.get('w')) || innerWidth, H0 = Number(q.get('h')) || innerHeight;
const isMac = /Mac/.test(navigator.userAgent);
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
let k = 1;

img.addEventListener('load', () => {
  pin.classList.add('lift');
  pin.addEventListener('animationend', () => pin.classList.remove('lift'), { once: true });
}, { once: true });
img.src = pinImageSrc(q);

let chipT = 0;
function flash(text: string) {
  chip.textContent = text;
  chip.classList.add('on');
  clearTimeout(chipT);
  chipT = window.setTimeout(() => chip.classList.remove('on'), 900);
}

/* —— 拖动：按下后移动超过 3 px 才交给系统拖动（在 pointerdown 里就开始会吞掉双击） —— */
let down: { x: number; y: number } | null = null;
pin.addEventListener('pointerdown', (e) => {
  pin.classList.remove('drag');
  down = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
});
pin.addEventListener('pointermove', (e) => {
  if (!down) return;
  if (!(e.buttons & 1)) { down = null; return; }
  if (Math.hypot(e.clientX - down.x, e.clientY - down.y) <= 3) return;
  down = null;
  pin.classList.add('drag');
  pinStartDrag().catch(() => pin.classList.remove('drag'));
});
addEventListener('pointerup', () => { down = null; pin.classList.remove('drag'); });

/* 改窗口的操作（缩放、恢复原始大小、拉回屏内）排成一队：每步都在上一步改完之后再读 k 和位置。
   失败（窗口正在关闭等）静默跳过，不影响后面的步骤。 */
let chain = Promise.resolve();
const run = (f: () => Promise<void>) => { chain = chain.then(f).catch(() => {}); };

/* 系统拖动期间收不到指针事件：移动停下 200 ms 后看主键，松开了才把拖出屏的拉回到至少留 40 px
   （拖动途中改位置会和系统的拖动打架）；自己改的位置也会触发，已在屏内的不再变 */
let moveT = 0;
const arm = () => {
  clearTimeout(moveT);
  moveT = window.setTimeout(() => run(settle), 200);
};
onPinMoved(arm).catch(() => {});
async function settle() {
  if (await pinButtonDown()) { arm(); return; }
  pin.classList.remove('drag');
  const [p, a] = await Promise.all([pinPosition(), pinWorkArea()]);
  if (!a) return;
  const w = Math.round(W0 * k), h = Math.round(H0 * k);
  const c = clampPos(p.x - a.x, p.y - a.y, w, h, a);
  const x = a.x + c.x, y = a.y + c.y;
  // 拖到缩放不同的屏上时后端可能按窗口尺寸上限缩小，k 以返回的系数为准（同缩放、恢复原始大小）
  if (Math.abs(x - p.x) > 0.5 || Math.abs(y - p.y) > 0.5) k *= await pinSetBounds({ x, y, w, h });
}

/* —— 滚轮缩放：每格 ×1.1，光标为锚。格数按 wheelSteps 累计；改窗口期间来的格攒着，下一步一起改 —— */
let wheel: Wheel = { acc: 0, t: -Infinity }, pend = 0, queued = false, at = { x: 0, y: 0 };
pin.addEventListener('wheel', (e) => {
  e.preventDefault();
  const r = wheelSteps(wheel, e.deltaY, e.deltaMode, e.timeStamp, innerHeight);
  wheel = r.next;
  if (!r.steps) return;
  pend += r.steps;
  const b = pin.getBoundingClientRect();
  at = { x: e.clientX - b.left, y: e.clientY - b.top };
  if (!queued) { queued = true; run(zoom); }
}, { passive: false });
/** 位置每次现取（系统拖动会改它）；攒下的几格算完只改一次窗口。 */
async function zoom() {
  queued = false;
  const n = pend;
  pend = 0;
  if (!n) return;
  const p = await pinPosition(), a = { x: p.x + at.x, y: p.y + at.y };
  // 上限按窗口每边的物理像素算，在这里夹住，锚点才对得上；后端返回实际的缩小系数（正常为 1），k 以它为准
  const max = maxZoom(W0, H0, devicePixelRatio || 1);
  let z = { k, x: p.x, y: p.y, w: 0, h: 0 };
  for (let i = 0; i < Math.abs(n); i++) z = zoomAt(z.k, n, a, { x: z.x, y: z.y, w0: W0, h0: H0 }, max);
  if (z.k !== k) k = z.k * (await pinSetBounds(z));
  flash(`${Math.round(k * 100)}%`);
}

/** 恢复原始大小：左上角不动（同原型 resetPin）。 */
function reset() {
  run(async () => {
    if (k === 1) return;
    const p = await pinPosition();
    k = await pinSetBounds({ x: p.x, y: p.y, w: W0, h: H0 });
  });
}

async function copy() {
  try { await pinCopy(); flash('已复制'); } catch { flash('复制失败'); }
}
async function save() {
  try { flash((await pinSave()).copied ? '已保存并复制' : '已保存'); } catch { flash('保存失败'); }
}

/** 双击、菜单关闭淡出 120 ms；Esc 等键盘触发不加动画（M20）。 */
let closing = false;
function close(animate: boolean) {
  if (closing) return;
  closing = true;
  const ms = animate && !reduced.matches ? 120 : 0;
  if (ms) pin.classList.add('out');
  window.setTimeout(() => pinClose().catch(() => { closing = false; pin.classList.remove('out'); }), ms);
}

pin.addEventListener('dblclick', () => close(true));
addEventListener('contextmenu', (e) => {
  e.preventDefault();
  const items: MenuEntry[] = [
    { text: '复制', accel: 'CmdOrCtrl+C', run: () => void copy() },
    { text: '保存', accel: 'CmdOrCtrl+S', run: () => void save() },
    ...(k !== 1 ? [{ text: '恢复原始大小', accel: 'CmdOrCtrl+0', run: reset }] : []),
    'sep',
    { text: '关闭', accel: 'Escape', run: () => close(true) },
  ];
  pinMenu(items).catch(() => {});
});
addEventListener('keydown', (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (e.key === 'Escape') close(false);
  else if (mod && e.code === 'KeyC') void copy();
  else if (mod && e.code === 'KeyS') void save();
  else if (mod && e.code === 'Digit0') reset();
  else return;
  e.preventDefault();
});
