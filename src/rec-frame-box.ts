// 录屏边框的位置（M24 规则 7）：四个直角画在选区外 GAP px 处；超出屏幕的那边收回到屏幕边上，
// 全屏或贴边录制时角仍看得见（这时角落在录制范围内，靠录制排除边框窗口不进视频）。
export type Box = { x: number; y: number; w: number; h: number };

export const GAP = 3;

/** `r`：选区，屏内逻辑像素；`vw`/`vh`：边框窗口（即整块屏）的大小。 */
export function frameBox(r: Box, vw: number, vh: number): Box {
  const x0 = Math.max(r.x - GAP, 0);
  const y0 = Math.max(r.y - GAP, 0);
  const x1 = Math.min(r.x + r.w + GAP, vw);
  const y1 = Math.min(r.y + r.h + GAP, vh);
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}
