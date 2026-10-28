// 截图遮罩（shot.html）与 Rust 侧 src-tauri/src/shot.rs、commands/shot.rs 的对接。
// 不在 Tauri 里（浏览器打开 `pnpm dev` 的 /shot.html）时换成开发替身：生成一张测试帧并立即开始一次截图，
// 供对照原型调界面；动作返回模拟结果，退出后 0.8 秒再开始一次。
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';

/** 显示器内的逻辑像素矩形（左上为原点）。 */
export type LRect = { x: number; y: number; w: number; h: number };
export type BeginPayload = {
  index: number;
  generation: number;
  /** 与该屏相交的窗口，由前到后 */
  rects: LRect[];
  /** 只有光标所在那块屏有值 */
  cursor: [number, number] | null;
  scale: number;
};
export type ShotAction = 'copy' | 'save' | 'ocr' | 'pin';
export type ShotOutcome = { action: ShotAction; chars: number | null; file: string | null; copied: boolean };
/** 冻结帧：物理像素，RGBA（非预乘）。 */
export type Frame = { w: number; h: number; rgba: Uint8ClampedArray<ArrayBuffer> };
/** 跨遮罩广播：某块屏有了选区（其他屏放弃自己的选区），或动作已完成（`done`，其他屏的蒙层一起淡出）。 */
export type ShotClaim = { generation: number; index: number; done: boolean };

const tauri = '__TAURI_INTERNALS__' in window;
const SHOT_BEGIN = 'shot-begin', SHOT_CLAIM = 'shot-claim';

/** 推送的 `shot-begin` 可能早于监听注册好（刚建的遮罩），注册后再拉一次；两路可能都到，回调需按代数去重。 */
export async function onShotBegin(cb: (p: BeginPayload) => void): Promise<void> {
  if (!tauri) {
    dev.begin = cb;
    setTimeout(() => cb(dev.payload()));
    return;
  }
  await listen<BeginPayload>(SHOT_BEGIN, (e) => cb(e.payload));
  const p = await invoke<BeginPayload | null>('shot_begin_payload');
  if (p) cb(p);
}

export async function shotFrame(): Promise<Frame> {
  if (!tauri) return dev.frame();
  const buf = await invoke<ArrayBuffer>('shot_frame');
  const v = new DataView(buf);
  const w = v.getUint32(0, true), h = v.getUint32(4, true);
  return { w, h, rgba: new Uint8ClampedArray(buf, 8, w * h * 4) };
}

export const shotReady = (generation: number) => (tauri ? invoke<void>('shot_ready', { generation }) : Promise.resolve());

export const shotCancel = () => (tauri ? invoke<void>('shot_cancel') : dev.cancel());

/** 合成好的 PNG 交给后端；`rect` 为显示器内逻辑像素（贴图用）。 */
export function shotFinish(png: Uint8Array, action: ShotAction, index: number, rect: LRect): Promise<ShotOutcome> {
  if (!tauri) return dev.finish(action);
  const r = [rect.x, rect.y, rect.w, rect.h].map((n) => Math.round(n * 100) / 100).join(',');
  return invoke<ShotOutcome>('shot_finish', png, { headers: { 'x-action': action, 'x-index': String(index), 'x-rect': r } });
}

export const shotClaim = (c: ShotClaim) => (tauri ? emit(SHOT_CLAIM, c) : Promise.resolve());
export const onShotClaim = async (cb: (c: ShotClaim) => void) => {
  if (tauri) await listen<ShotClaim>(SHOT_CLAIM, (e) => cb(e.payload));
};

/* —— 开发替身：一块模拟桌面，窗口布局同原型 —— */
const dev = {
  gen: 0,
  begin: null as ((p: BeginPayload) => void) | null,
  wins(): LRect[] {
    const W = innerWidth, H = innerHeight, R = (x: number, y: number, w: number, h: number) => ({ x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h) });
    // 由前到后：菜单栏、浏览器、聊天、编辑器
    return [R(0, 0, W, 26), R(W * 0.33, H * 0.17, W * 0.44, H * 0.64), R(W * 0.745, H * 0.33, W * 0.22, H * 0.48), R(W * 0.04, H * 0.1, Math.max(W * 0.4, 420), H * 0.56)];
  },
  payload(): BeginPayload {
    return { index: 0, generation: ++this.gen, rects: this.wins(), cursor: [innerWidth / 2, innerHeight / 2], scale: devicePixelRatio || 1 };
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
    await new Promise((r) => setTimeout(r, action === 'ocr' ? 900 : 60));
    const p = (n: number) => String(n).padStart(2, '0'), d = new Date();
    const file = `截图 ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}.${p(d.getMinutes())}.png`;
    return { action, chars: action === 'ocr' ? 42 : null, file: action === 'save' ? file : null, copied: action !== 'pin' };
  },
  async cancel() {
    setTimeout(() => this.begin?.(this.payload()), 800);
  },
};
