// 贴图窗口的缩放与位置（M23 规则 9）：屏幕逻辑坐标。纯函数，不碰 DOM。
// 数值取自原型 docs/specs/2026-10-28-截图与录屏-原型.html（makePin 的 wheel / pointermove）。

const clamp = (v: number, a: number, b: number) => Math.min(Math.max(v, a), b);

/** 窗口每边不超过的物理像素（webview 的纹理上限），同后端 `MAX_PIN_PX`。 */
const MAX_PX = 16384;

/** 缩放上限：400%，且长边不超过 `MAX_PX` 物理像素（`dpr` 为窗口所在屏的缩放）。 */
export const maxZoom = (w0: number, h0: number, dpr: number) => Math.min(4, MAX_PX / dpr / Math.max(w0, h0));

/**
 * 滚轮一格：`deltaY < 0` 放大 ×1.1，否则缩小 ÷1.1，夹在 20%–`max`（默认 400%，见 `maxZoom`），离 100% 不到 4% 时吸附为 1。
 * `win` 为窗口当前左上角与原始尺寸（当前尺寸 = 原始 × k）；光标 `anchor` 在屏幕上保持不动。结果取整。
 */
export function zoomAt(
  k: number,
  deltaY: number,
  anchor: { x: number; y: number },
  win: { x: number; y: number; w0: number; h0: number },
  max = 4,
): { k: number; x: number; y: number; w: number; h: number } {
  const nk0 = clamp(k * (deltaY < 0 ? 1.1 : 1 / 1.1), 0.2, max);
  const nk = Math.abs(nk0 - 1) < 0.04 ? 1 : nk0;
  const fx = (anchor.x - win.x) / (win.w0 * k), fy = (anchor.y - win.y) / (win.h0 * k);
  const w = Math.round(win.w0 * nk), h = Math.round(win.h0 * nk);
  return { k: nk, x: Math.round(anchor.x - fx * w), y: Math.round(anchor.y - fy * h), w, h };
}

/** 拖不丢：左右、下方至少留 40 px 在屏内，顶边不出屏（同原型）。坐标相对该屏左上角。 */
export function clampPos(x: number, y: number, w: number, _h: number, screen: { w: number; h: number }): { x: number; y: number } {
  return { x: clamp(x, 40 - w, screen.w - 40), y: clamp(y, 0, screen.h - 40) };
}

/** 滚轮累计：`acc` 为未用完的像素，`t` 为上一个事件的时间（ms）。 */
export type Wheel = { acc: number; t: number };
const STEP_PX = 50, GAP_MS = 150;

/**
 * 滚轮事件 → 缩放格数（符号同 deltaY）。停顿超过 150 ms 后的第一下固定算一格：鼠标一格就是一格，不论 delta 大小；
 * 连续的事件（触控板、惯性）按距离累计，每 50 px 一格，余数留到下次。一个事件最多一格，超出的部分丢掉。
 * `deltaMode` 1 为行（×16 px），2 为页（×`page` px）。
 */
export function wheelSteps(s: Wheel, deltaY: number, deltaMode: number, t: number, page: number): { steps: number; next: Wheel } {
  const px = deltaY * (deltaMode === 1 ? 16 : deltaMode === 2 ? page : 1);
  if (t - s.t > GAP_MS) return { steps: Math.sign(px), next: { acc: 0, t } };
  const acc = s.acc + px;
  const steps = Math.abs(acc) >= STEP_PX ? Math.sign(acc) : 0;
  const rest = acc - steps * STEP_PX;
  return { steps, next: { acc: Math.abs(rest) < STEP_PX ? rest : 0, t } };
}
