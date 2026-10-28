// 截图标注：数据结构、约束与绘制。坐标为逻辑像素，画到已按缩放系数变换的 canvas 上。
// 除 drawObj 外是纯函数；drawObj 只在 main.ts 里调用。参数取自原型（constrain / drawArrow / drawObj）。

export type Pt = [number, number];
type Size = 0 | 1 | 2;
/** 每种工具一个成员（按 `t` 收窄） */
type Of<T extends string, F> = { [K in T]: { t: K; color: string; size: Size } & F }[T];
export type Obj =
  | Of<'rect' | 'ellipse' | 'arrow', { x0: number; y0: number; x1: number; y1: number; shift: boolean }>
  | Of<'pen' | 'mosaic', { pts: Pt[] }>
  | Of<'text', { x: number; y: number; text: string }>
  | Of<'step', { x: number; y: number; n: number }>;
export type Tool = Obj['t'];

export const COLORS = ['#FF3B47', '#FF9F1A', '#22C55E', '#0EA5E9', '#8B5CF6', '#FFFFFF'];
export const STROKE = [2, 4, 7], FONT = [14, 20, 28], STEP_R = [11, 14, 18], MOSAIC = [14, 24, 40];
const UI = '"Geist", "PingFang SC", "Microsoft YaHei", sans-serif';

/** 按住 Shift：矩形 / 椭圆取正方形 / 正圆（保留拖动方向），箭头吸附到 45° 的倍数且长度不变。 */
export function constrain(o: { t: Tool; x0: number; y0: number; x1: number; y1: number; shift: boolean }) {
  let { x0, y0, x1, y1 } = o;
  if (o.shift) {
    const dx = x1 - x0, dy = y1 - y0;
    if (o.t === 'arrow') {
      const a = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4), len = Math.hypot(dx, dy);
      x1 = x0 + Math.cos(a) * len;
      y1 = y0 + Math.sin(a) * len;
    } else {
      const s = Math.max(Math.abs(dx), Math.abs(dy));
      x1 = x0 + Math.sign(dx || 1) * s;
      y1 = y0 + Math.sign(dy || 1) * s;
    }
  }
  return { x0, y0, x1, y1 };
}

/** 箭头的填充多边形：尾部细、杆身渐粗、箭头带倒钩。顺序为一侧尾 → 杆 → 倒钩 → 尖 → 另一侧对称回到尾。 */
export function arrowPolygon(x0: number, y0: number, x1: number, y1: number, s: number): Pt[] {
  const dx = x1 - x0, dy = y1 - y0, len = Math.hypot(dx, dy);
  if (len < 1) return [];
  const ux = dx / len, uy = dy / len, nx = -uy, ny = ux;
  const hl = Math.min(len * 0.7, 9 + s * 3.4), hw = hl * 0.52, bw = s * 0.62, tw = s * 0.22;
  const bx = x1 - ux * hl * 0.78, by = y1 - uy * hl * 0.78, wx = x1 - ux * hl, wy = y1 - uy * hl;
  return [
    [x0 + nx * tw, y0 + ny * tw], [bx + nx * bw, by + ny * bw], [wx + nx * hw, wy + ny * hw],
    [x1, y1],
    [wx - nx * hw, wy - ny * hw], [bx - nx * bw, by - ny * bw], [x0 - nx * tw, y0 - ny * tw],
  ];
}

/** 下一个序号：已有序号标注的个数 + 1。 */
export const nextStep = (objs: Obj[]) => objs.filter((o) => o.t === 'step').length + 1;

/** 画一个标注。`pix` 是马赛克底图（与 canvas 同为物理像素尺寸），马赛克笔迹用它做图案描边。 */
export function drawObj(c: CanvasRenderingContext2D, o: Obj, pix: CanvasImageSource) {
  c.save();
  c.strokeStyle = c.fillStyle = o.color;
  c.lineCap = c.lineJoin = 'round';
  const s = STROKE[o.size];
  if (o.t === 'rect' || o.t === 'ellipse' || o.t === 'arrow') {
    const { x0, y0, x1, y1 } = constrain(o);
    c.lineWidth = s;
    c.shadowColor = 'rgba(0,0,0,.22)'; c.shadowBlur = 3; c.shadowOffsetY = 1;
    c.beginPath();
    if (o.t === 'rect') { c.roundRect(Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0), 2); c.stroke(); }
    else if (o.t === 'ellipse') { c.ellipse((x0 + x1) / 2, (y0 + y1) / 2, Math.abs(x1 - x0) / 2, Math.abs(y1 - y0) / 2, 0, 0, 7); c.stroke(); }
    else {
      const p = arrowPolygon(x0, y0, x1, y1, s + 1);
      p.forEach(([x, y], i) => (i ? c.lineTo(x, y) : c.moveTo(x, y)));
      c.closePath(); c.fill();
    }
  } else if (o.t === 'pen') {
    const p = o.pts;
    c.lineWidth = s;
    c.beginPath(); c.moveTo(p[0][0], p[0][1]);
    if (p.length === 1) c.lineTo(p[0][0] + 0.1, p[0][1]);
    for (let i = 1; i < p.length - 1; i++) c.quadraticCurveTo(p[i][0], p[i][1], (p[i][0] + p[i + 1][0]) / 2, (p[i][1] + p[i + 1][1]) / 2);
    if (p.length > 1) c.lineTo(p[p.length - 1][0], p[p.length - 1][1]);
    c.stroke();
  } else if (o.t === 'mosaic') {
    // 底图按物理像素对齐：图案变换抵消当前的缩放
    const pat = c.createPattern(pix, 'no-repeat');
    if (pat) {
      pat.setTransform(c.getTransform().invertSelf());
      c.strokeStyle = pat;
      c.lineWidth = MOSAIC[o.size];
      const p = o.pts;
      c.beginPath(); c.moveTo(p[0][0], p[0][1]);
      c.lineTo(p.length === 1 ? p[0][0] + 0.1 : p[1][0], p.length === 1 ? p[0][1] : p[1][1]);
      for (let i = 2; i < p.length; i++) c.lineTo(p[i][0], p[i][1]);
      c.stroke();
    }
  } else if (o.t === 'text') {
    const fs = FONT[o.size];
    c.font = `600 ${fs}px ${UI}`; c.textBaseline = 'top';
    o.text.split('\n').forEach((ln, i) => c.fillText(ln, o.x, o.y + i * fs * 1.25));
  } else {
    const rad = STEP_R[o.size], white = o.color === '#FFFFFF';
    c.shadowColor = 'rgba(0,0,0,.3)'; c.shadowBlur = 6; c.shadowOffsetY = 2;
    c.beginPath(); c.arc(o.x, o.y, rad, 0, 7); c.fill();
    c.shadowColor = 'transparent';
    c.lineWidth = 2; c.strokeStyle = white ? 'rgba(0,0,0,.25)' : '#FFFFFF'; c.stroke();
    c.fillStyle = white ? '#11131A' : '#FFFFFF';
    c.font = `700 ${Math.round(rad * 1.15)}px ${UI}`; c.textAlign = 'center'; c.textBaseline = 'middle';
    c.fillText(String(o.n), o.x, o.y + 1);
  }
  c.restore();
}
