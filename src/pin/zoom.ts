// 贴图窗口的缩放与位置（设计 §4.5）：屏幕逻辑坐标。纯函数，不碰 DOM。
// 数值取自原型 docs/specs/2026-10-28-截图与录屏-原型.html（makePin 的 wheel / pointermove）。

const clamp = (v: number, a: number, b: number) => Math.min(Math.max(v, a), b);

/**
 * 滚轮一格：`deltaY < 0` 放大 ×1.1，否则缩小 ÷1.1，夹在 20%–400%，离 100% 不到 4% 时吸附为 1。
 * `win` 为窗口当前左上角与原始尺寸（当前尺寸 = 原始 × k）；光标 `anchor` 在屏幕上保持不动。结果取整。
 */
export function zoomAt(
  k: number,
  deltaY: number,
  anchor: { x: number; y: number },
  win: { x: number; y: number; w0: number; h0: number },
): { k: number; x: number; y: number; w: number; h: number } {
  const nk0 = clamp(k * (deltaY < 0 ? 1.1 : 1 / 1.1), 0.2, 4);
  const nk = Math.abs(nk0 - 1) < 0.04 ? 1 : nk0;
  const fx = (anchor.x - win.x) / (win.w0 * k), fy = (anchor.y - win.y) / (win.h0 * k);
  const w = Math.round(win.w0 * nk), h = Math.round(win.h0 * nk);
  return { k: nk, x: Math.round(anchor.x - fx * w), y: Math.round(anchor.y - fy * h), w, h };
}

/** 拖不丢：左右、下方至少留 40 px 在屏内，顶边不出屏（同原型）。坐标相对该屏左上角。 */
export function clampPos(x: number, y: number, w: number, _h: number, screen: { w: number; h: number }): { x: number; y: number } {
  return { x: clamp(x, 40 - w, screen.w - 40), y: clamp(y, 0, screen.h - 40) };
}
