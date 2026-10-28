// 截图遮罩的选区几何：逻辑像素，原点为本块屏左上角。纯函数，不碰 DOM。
// 数值取自原型 docs/specs/2026-10-28-截图与录屏-原型.html（norm / resize / hitWin / place / update / drawLoupe）。

export type Rect = { x: number; y: number; w: number; h: number };
export type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

const clamp = (v: number, a: number, b: number) => Math.min(Math.max(v, a), b);

/** 两个对角点 → 正宽高的矩形，夹在屏内，取整。 */
export function norm(x0: number, y0: number, x1: number, y1: number, W: number, H: number): Rect {
  const x = clamp(Math.min(x0, x1), 0, W), y = clamp(Math.min(y0, y1), 0, H);
  return {
    x: Math.round(x),
    y: Math.round(y),
    w: Math.round(clamp(Math.max(x0, x1), 0, W) - x),
    h: Math.round(clamp(Math.max(y0, y1), 0, H) - y),
  };
}

/** 拖动手柄调整大小：越过对边时翻转，最小 1 × 1。 */
export function resize(r: Rect, handle: Handle, dx: number, dy: number, W: number, H: number): Rect {
  let x1 = r.x, y1 = r.y, x2 = r.x + r.w, y2 = r.y + r.h;
  if (handle.includes('w')) x1 += dx;
  if (handle.includes('e')) x2 += dx;
  if (handle.includes('n')) y1 += dy;
  if (handle.includes('s')) y2 += dy;
  const n = norm(x1, y1, x2, y2, W, H);
  return { ...n, w: Math.max(n.w, 1), h: Math.max(n.h, 1) };
}

/** 吸附：由前到后第一个包含该点的窗口，都不包含时为整块屏。 */
export function hit(rects: Rect[], x: number, y: number, W: number, H: number): Rect {
  const r = rects.find((r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h);
  return r ? { x: r.x, y: r.y, w: r.w, h: r.h } : { x: 0, y: 0, w: W, h: H };
}

/**
 * 工具栏位置：选区下方放得下就放下方，否则放上方，上下都放不下时放进选区右下角；右边与选区对齐。
 * `subH` 是颜色粗细条的高度加 6 px 间距（没有时为 0）。
 */
export function placeBar(sel: Rect, bar: { w: number; h: number }, subH: number, W: number, H: number) {
  const g = 8;
  let y: number, above = false, inside = false;
  if (sel.y + sel.h + g + bar.h + subH + g <= H) y = sel.y + sel.h + g;
  else if (sel.y - g - bar.h - subH >= 0) { y = sel.y - g - bar.h; above = true; }
  else { y = sel.y + sel.h - g - bar.h - subH; inside = true; }
  return { x: clamp(sel.x + sel.w - bar.w, 8, W - bar.w - 8), y, above, inside };
}

/** 尺寸标签：选区左上角上方；工具栏占了上方或选区贴顶时放进选区内。 */
export function badgePos(sel: Rect, badgeW: number, barAbove: boolean, W: number) {
  const out = !barAbove && sel.y - 30 >= 4;
  return { x: clamp(out ? sel.x : sel.x + 6, 4, W - badgeW - 4), y: out ? sel.y - 30 : sel.y + 6 };
}

/** 放大镜（124 × 约 150）：光标右下方 22 px，靠近右边 / 下边时翻到左 / 上。 */
export function loupePos(mx: number, my: number, W: number, H: number) {
  let x = mx + 22, y = my + 22;
  if (x + 126 > W - 4) x = mx - 22 - 124;
  if (y + 160 > H - 4) y = my - 22 - 152;
  return { x, y };
}
