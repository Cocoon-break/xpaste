// 贴图页面（pin.html），每张贴图一个窗口，窗口即图片（设计 §4.5）。交互对照原型 docs/specs/2026-10-28-截图与录屏-原型.html
// 的 makePin：拖动移动、滚轮以光标为锚缩放、双击或 Esc 关闭、右键菜单。坐标为屏幕逻辑坐标。
import './pin.css';
import {
  onPinMoved, pinClose, pinCopy, pinImageSrc, pinMenu, pinPosition, pinSave, pinSetBounds, pinStartDrag, pinWorkArea,
  type MenuEntry,
} from '../api/shot';
import { clampPos, zoomAt } from './zoom';

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
  void pinStartDrag();
});
addEventListener('pointerup', () => { down = null; pin.classList.remove('drag'); });

/* 系统拖动期间收不到指针事件：移动停下 200 ms 算拖完，拖出屏的拉回到至少留 40 px（自己改的位置也会触发，夹过的不再变） */
let moveT = 0;
void onPinMoved(() => {
  clearTimeout(moveT);
  moveT = window.setTimeout(() => void settle(), 200);
});
async function settle() {
  pin.classList.remove('drag');
  const [p, a] = await Promise.all([pinPosition(), pinWorkArea()]);
  if (!a) return;
  const w = Math.round(W0 * k), h = Math.round(H0 * k);
  const c = clampPos(p.x - a.x, p.y - a.y, w, h, a);
  const x = a.x + c.x, y = a.y + c.y;
  if (Math.abs(x - p.x) > 0.5 || Math.abs(y - p.y) > 0.5) await pinSetBounds({ x, y, w, h });
}

/* —— 滚轮缩放：每格 ×1.1，光标为锚；位置每次现取（系统拖动会改它），一次只改一次窗口 —— */
// ponytail: 按事件计格、两格至少隔 40 ms，触控板的连串小 delta 才不会一下缩到头；要按 delta 大小连续缩放再改
let busy = false, lastWheel = -Infinity;
pin.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (busy || e.deltaY === 0 || e.timeStamp - lastWheel < 40) return;
  lastWheel = e.timeStamp;
  busy = true;
  void zoom(e).finally(() => { busy = false; });
}, { passive: false });
async function zoom(e: WheelEvent) {
  const r = pin.getBoundingClientRect(), p = await pinPosition();
  const z = zoomAt(k, e.deltaY, { x: p.x + e.clientX - r.left, y: p.y + e.clientY - r.top }, { x: p.x, y: p.y, w0: W0, h0: H0 });
  await pinSetBounds(z);
  k = z.k;
  flash(`${Math.round(k * 100)}%`);
}

/** 恢复原始大小：左上角不动（同原型 resetPin）。 */
async function reset() {
  if (k === 1) return;
  const p = await pinPosition();
  await pinSetBounds({ x: p.x, y: p.y, w: W0, h: H0 });
  k = 1;
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
    ...(k !== 1 ? [{ text: '恢复原始大小', accel: 'CmdOrCtrl+0', run: () => void reset() }] : []),
    'sep',
    { text: '关闭', accel: 'Escape', run: () => close(true) },
  ];
  void pinMenu(items);
});
addEventListener('keydown', (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (e.key === 'Escape') close(false);
  else if (mod && e.code === 'KeyC') void copy();
  else if (mod && e.code === 'KeyS') void save();
  else if (mod && e.code === 'Digit0') void reset();
  else return;
  e.preventDefault();
});
