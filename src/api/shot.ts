// 截图遮罩（shot.html）、贴图（pin.html）与 Rust 侧 src-tauri/src/shot.rs、commands/shot.rs 的对接。
// 不在 Tauri 里（浏览器打开 `pnpm dev` 的 /shot.html、/pin.html）时换成开发替身：遮罩生成一张测试帧并立即开始一次截图，
// 供对照原型调界面；动作返回模拟结果，退出后 0.8 秒再开始一次。贴图见文末 devPin。
// 替身的查询参数：`?rec` 录屏模式，`&mic=none` 本机不支持麦克风，`&recfail` 开始录屏失败。
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { Menu } from '@tauri-apps/api/menu';
import { currentMonitor, getCurrentWindow, primaryMonitor } from '@tauri-apps/api/window';

import { translateEngines } from './commands';
import { TRANSLATE_DELTA } from './events';
import type { BeginPayload, Engines, LRect, ShotAction, ShotOutcome, TranslateDelta } from './types';

export type { BeginPayload, LRect, ShotAction, ShotOutcome };
/** 冻结帧：物理像素，RGBA（非预乘）。 */
export type Frame = { w: number; h: number; rgba: Uint8ClampedArray<ArrayBuffer> };
/**
 * 跨遮罩广播这块屏的状态：有了选区（其他屏放弃自己的选区）、正在识别文字（其他屏不能再框选）、
 * 动作已完成（其他屏的蒙层一起淡出）。
 */
export type ShotClaim = { generation: number; index: number; state: 'selected' | 'busy' | 'done' };

/** 开发替身：只在 `pnpm dev` 下、浏览器里（不在 Tauri 里）启用；生产构建里整段被摇掉 */
const stub = import.meta.env.DEV && !('__TAURI_INTERNALS__' in window);
const SHOT_BEGIN = 'shot-begin', SHOT_CLAIM = 'shot-claim', SHOT_END = 'shot-end';

/** 推送的 `shot-begin` 可能早于监听注册好（刚建的遮罩），注册后再拉一次；两路可能都到，回调需按代数去重。 */
export async function onShotBegin(cb: (p: BeginPayload) => void): Promise<void> {
  if (stub) {
    dev.begin = cb;
    setTimeout(() => cb(dev.payload()));
    return;
  }
  // 只收发给本遮罩的：不带 target 的 listen 会收到发给每块屏的载荷，先到的别屏载荷会占了这次的代数
  await listen<BeginPayload>(SHOT_BEGIN, (e) => cb(e.payload), { target: getCurrentWindow().label });
  const p = await invoke<BeginPayload | null>('shot_begin_payload');
  if (p) cb(p);
}

export async function shotFrame(): Promise<Frame> {
  if (stub) return dev.frame();
  const buf = await invoke<ArrayBuffer>('shot_frame');
  const v = new DataView(buf);
  const w = v.getUint32(0, true), h = v.getUint32(4, true);
  return { w, h, rgba: new Uint8ClampedArray(buf, 8, w * h * 4) };
}

export const shotReady = (generation: number) => (stub ? Promise.resolve() : invoke<void>('shot_ready', { generation }));

/** 遮罩页心跳；返回这次截图是否仍在进行（后端已结束时为 false）。 */
export const shotAlive = (generation: number) => (stub ? Promise.resolve(true) : invoke<boolean>('shot_alive', { generation }));

/** 退出截图；`generation` 不是当前这次时后端忽略（上一次截图迟到的退出不能结束新的一次）。 */
export const shotCancel = (generation: number) => (stub ? dev.cancel() : invoke<void>('shot_cancel', { generation }));

/** 合成好的 PNG 交给后端；`rect` 为显示器内逻辑像素（贴图用）。 */
export function shotFinish(png: Uint8Array, action: ShotAction, index: number, rect: LRect): Promise<ShotOutcome> {
  if (stub) return dev.finish(action);
  const r = [rect.x, rect.y, rect.w, rect.h].map((n) => Math.round(n * 100) / 100).join(',');
  return invoke<ShotOutcome>('shot_finish', png, { headers: { 'x-action': action, 'x-index': String(index), 'x-rect': r } });
}

/** 截图翻译的增量（M26 规则 5）：后端只发给发起的遮罩；面板翻译的广播（`id` 不为 null）不收。`engine` 为翻译方式标签。 */
export async function onShotTranslateDelta(cb: (text: string, engine: string) => void): Promise<void> {
  if (stub) { dev.delta = cb; return; }
  await listen<TranslateDelta>(TRANSLATE_DELTA, (e) => { if (e.payload.id === null) cb(e.payload.text, e.payload.engine); }, { target: getCurrentWindow().label });
}

/** 译文卡片“换用…”（M26 规则 16）：用本次截图识别出的原文按 `engine` 重翻，增量照旧发给本遮罩。 */
export const shotTranslateAgain = (generation: number, engine: 'system' | 'openai') =>
  stub ? dev.finish('translate') : invoke<ShotOutcome>('shot_translate_again', { generation, engine });

/** 两种翻译方式能不能用、按设置用哪种（M26 规则 16）。 */
export const shotEngines = (): Promise<Engines> =>
  stub ? Promise.resolve({ system: true, openai: true, model: 'gpt-4o-mini', current: 'system' }) : translateEngines();

/** 译文卡片“复制译文”：写剪切板（进历史）后后端进入提示条模式。 */
export const shotCopyText = (generation: number, text: string) => (stub ? Promise.resolve() : invoke<void>('shot_copy_text', { generation, text }));

/** 倒计时结束，开始录屏（M24）。成功时后端已结束这次截图、隐藏遮罩；`generation`、`index` 取自载荷。 */
export const recStart = (a: { generation: number; index: number; rect: LRect; systemAudio: boolean; microphone: boolean }) =>
  stub ? dev.recStart() : invoke<void>('rec_start', a);

/** 后端结束了这次截图（载荷为代数）：再按快捷键、看门狗、Alt+F4 等，页面自己不知道。 */
export const onShotEnd = async (cb: (generation: number) => void) => {
  if (!stub) await listen<number>(SHOT_END, (e) => cb(e.payload));
};

export const shotClaim = (c: ShotClaim) => (stub ? Promise.resolve() : emit(SHOT_CLAIM, c));
export const onShotClaim = async (cb: (c: ShotClaim) => void) => {
  if (!stub) await listen<ShotClaim>(SHOT_CLAIM, (e) => cb(e.payload));
};

/* —— 录屏边框（rec-frame.html）—— */
const REC_FRAME = 'rec-frame';
/** 边框位置（该屏内逻辑像素），`null` 淡出。推送可能早于监听注册好（刚建的窗口），注册后再拉一次当前值。 */
export async function onRecFrame(cb: (r: LRect | null) => void): Promise<void> {
  if (stub) {
    setTimeout(() => cb({ x: 120, y: 90, w: 640, h: 360 }));
    return;
  }
  await listen<LRect | null>(REC_FRAME, (e) => cb(e.payload));
  cb(await invoke<LRect | null>('rec_frame'));
}

/* —— 贴图：每张一个窗口 pin-<n>，命令都作用于调用方窗口；坐标为屏幕逻辑坐标 —— */
export type Box = { x: number; y: number; w: number; h: number };
export type MenuEntry = { text: string; accel: string; run: () => void } | 'sep';

/** 图片地址：后端拼好的 asset URL（`src` 参数，数据目录隔离时也对）；替身下是一张测试图。 */
export const pinImageSrc = (q: URLSearchParams) => (stub ? devPin.image(q) : q.get('src') ?? '');
export const pinClose = () => (stub ? devPin.close() : invoke<void>('pin_close'));
export const pinCopy = () => (stub ? devPin.outcome('copy') : invoke<ShotOutcome>('pin_copy'));
export const pinSave = () => (stub ? devPin.outcome('save') : invoke<ShotOutcome>('pin_save'));
/** 一次改位置和尺寸（后端在主线程里连着改，缩放时锚点不跳）。返回尺寸实际乘上的系数（超过窗口尺寸上限时小于 1）。 */
export const pinSetBounds = (b: Box) => (stub ? devPin.setBounds(b) : invoke<number>('pin_set_bounds', b));
export const pinStartDrag = () => (stub ? Promise.resolve() : getCurrentWindow().startDragging());

/** 窗口当前左上角：原生拖动会移动它，每次现取。 */
export async function pinPosition(): Promise<{ x: number; y: number }> {
  if (stub) return { x: devPin.b.x, y: devPin.b.y };
  const w = getCurrentWindow();
  const [p, k] = await Promise.all([w.outerPosition(), w.scaleFactor()]);
  return p.toLogical(k);
}

/** 窗口所在屏的工作区（不含菜单栏、程序坞、任务栏）；窗口整个拖出所有屏时取主屏。 */
export async function pinWorkArea(): Promise<Box | null> {
  if (stub) return { x: 0, y: 0, w: innerWidth, h: innerHeight };
  const m = (await currentMonitor()) ?? (await primaryMonitor());
  if (!m) return null;
  const p = m.workArea.position.toLogical(m.scaleFactor), s = m.workArea.size.toLogical(m.scaleFactor);
  return { x: p.x, y: p.y, w: s.width, h: s.height };
}

/** 鼠标主键此刻是否按着（系统拖动期间页面收不到指针事件，靠它判断拖完）。 */
export const pinButtonDown = () => (stub ? Promise.resolve(false) : invoke<boolean>('pin_button_down'));

/** 窗口被移动（含原生拖动）后回调。 */
export const onPinMoved = async (cb: () => void) => {
  if (!stub) await getCurrentWindow().onMoved(cb);
};

let lastMenu: Menu | null = null;
/** 原生右键菜单，弹在光标处。上一个菜单在这里才释放：popup 返回时点击事件可能还没送到。 */
export async function pinMenu(items: MenuEntry[]): Promise<void> {
  if (stub) return devPin.menu(items);
  void lastMenu?.close();
  lastMenu = await Menu.new({
    items: items.map((it) => (it === 'sep' ? { item: 'Separator' as const } : { text: it.text, accelerator: it.accel, action: it.run })),
  });
  await lastMenu.popup();
}

/* —— 开发替身：一块模拟桌面，窗口布局同原型 —— */
const dev = {
  gen: 0,
  begin: null as ((p: BeginPayload) => void) | null,
  delta: null as ((text: string, engine: string) => void) | null,
  wins(): LRect[] {
    const W = innerWidth, H = innerHeight, R = (x: number, y: number, w: number, h: number) => ({ x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h) });
    // 由前到后：菜单栏、浏览器、聊天、编辑器
    return [R(0, 0, W, 26), R(W * 0.33, H * 0.17, W * 0.44, H * 0.64), R(W * 0.745, H * 0.33, W * 0.22, H * 0.48), R(W * 0.04, H * 0.1, Math.max(W * 0.4, 420), H * 0.56)];
  },
  q: new URLSearchParams(location.search),
  payload(): BeginPayload {
    return {
      index: 0, generation: ++this.gen, rects: this.wins(), cursor: [innerWidth / 2, innerHeight / 2], scale: devicePixelRatio || 1, saveCopy: true,
      mode: this.q.has('rec') ? 'rec' : 'shot', systemAudio: false, microphone: this.q.get('mic') === 'none' ? null : false,
      recScale: 1, recFps: 30,
    };
  },
  frame(): Frame {
    const r = devicePixelRatio || 1, W = innerWidth, H = innerHeight;
    const cv = document.createElement('canvas');
    cv.width = Math.round(W * r); cv.height = Math.round(H * r);
    const c = cv.getContext('2d')!;
    c.scale(r, r);
    const g = c.createLinearGradient(0, 0, W, H);
    g.addColorStop(0, '#0B1030'); g.addColorStop(0.55, '#151B47'); g.addColorStop(1, '#22103A');
    c.fillStyle = g; c.fillRect(0, 0, W, H);
    const [bar, browser, chat, editor] = this.wins();
    const UI = '"Geist", "PingFang SC", sans-serif';
    for (const [b, dark, lines] of [[editor, true, 14], [chat, false, 5], [browser, false, 8]] as const) {
      c.save(); c.shadowColor = 'rgba(0,0,0,.5)'; c.shadowBlur = 44; c.shadowOffsetY = 18;
      c.beginPath(); c.roundRect(b.x, b.y, b.w, b.h, 11); c.fillStyle = dark ? '#151925' : '#FFFFFF'; c.fill(); c.restore();
      c.fillStyle = dark ? '#1D2230' : '#ECEEF3'; c.fillRect(b.x + 1, b.y + 1, b.w - 2, 32);
      ['#FF5F57', '#FEBC2E', '#28C840'].forEach((col, i) => { c.beginPath(); c.arc(b.x + 16 + i * 19, b.y + 16, 5.5, 0, 7); c.fillStyle = col; c.fill(); });
      c.font = `400 13px ${UI}`; c.fillStyle = dark ? '#D7DEF5' : '#3A3F4F';
      for (let i = 0; i < lines; i++) c.fillText(i % 3 ? '本季度预算总额 480 万元，较上季度增加 12%。' : '2026 年第四季度预算', b.x + 24, b.y + 62 + i * 24, b.w - 48);
    }
    c.fillStyle = 'rgba(8,10,22,.5)'; c.fillRect(bar.x, bar.y, bar.w, bar.h);
    c.font = `600 13px ${UI}`; c.fillStyle = 'rgba(255,255,255,.92)'; c.fillText('访达   文件   编辑   显示   前往   窗口   帮助', 40, 18);
    const d = c.getImageData(0, 0, cv.width, cv.height);
    return { w: cv.width, h: cv.height, rgba: d.data };
  },
  async finish(action: ShotAction): Promise<ShotOutcome> {
    await new Promise((r) => setTimeout(r, action === 'ocr' || action === 'translate' ? 900 : 60));
    if (action === 'translate') {
      // `?trfail` 中途失败
      const parts = ['The total budget for this quarter is ', '4.8 million yuan, ', 'an increase of 12% ', 'over the previous quarter.\n\n', 'Q4 2026 Budget'];
      for (const p of parts) { this.delta?.(p, 'system'); await new Promise((r) => setTimeout(r, 250)); if (this.q.has('trfail')) throw { message: '翻译超时' }; }
      const text = parts.join('');
      return { action, chars: text.length, file: null, copied: false, text, engine: 'system' };
    }
    const p = (n: number) => String(n).padStart(2, '0'), d = new Date();
    const file = `截图 ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}.${p(d.getMinutes())}.png`;
    return { action, chars: action === 'ocr' ? 42 : null, file: action === 'save' ? file : null, copied: action !== 'pin', text: null, engine: null };
  },
  async cancel() {
    setTimeout(() => this.begin?.(this.payload()), 800);
  },
  async recStart() {
    await new Promise((r) => setTimeout(r, 60));
    if (this.q.has('recfail')) throw { message: '正在录屏' };
    void this.cancel();
  },
};

/* —— 贴图开发替身：浏览器视口当作屏幕，#pin 当作贴图窗口（真实窗口里它铺满窗口） —— */
const devPin = {
  b: { x: 120, y: 90, w: 360, h: 220 },
  image(q: URLSearchParams): string {
    const w = Number(q.get('w')) || this.b.w, h = Number(q.get('h')) || this.b.h, r = devicePixelRatio || 1;
    Object.assign(this.b, { w, h });
    const cv = Object.assign(document.createElement('canvas'), { width: Math.round(w * r), height: Math.round(h * r) });
    const c = cv.getContext('2d')!;
    c.scale(r, r);
    c.fillStyle = '#FFFFFF'; c.fillRect(0, 0, w, h);
    c.fillStyle = '#ECEEF3'; c.fillRect(0, 0, w, 32);
    c.font = '400 13px "Geist", "PingFang SC", sans-serif'; c.fillStyle = '#3A3F4F';
    for (let i = 0; i < 6; i++) c.fillText(i % 3 ? '本季度预算总额 480 万元，较上季度增加 12%。' : '2026 年第四季度预算', 20, 58 + i * 24, w - 40);
    queueMicrotask(() => this.setBounds(this.b));
    return cv.toDataURL('image/png');
  },
  async setBounds(b: Box) {
    this.b = { ...b };
    const el = document.getElementById('pin');
    if (el) Object.assign(el.style, { position: 'fixed', left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
    return 1;
  },
  async close() {
    document.getElementById('pin')?.remove();
  },
  async outcome(action: 'copy' | 'save'): Promise<ShotOutcome> {
    return { action, chars: null, file: action === 'save' ? '截图 2026-10-28 10.24.png' : null, copied: true, text: null, engine: null };
  },
  async menu(items: MenuEntry[]) {
    console.log('贴图菜单', items.map((it) => (it === 'sep' ? '—' : `${it.text} ${it.accel}`)).join(' | '));
  },
};
