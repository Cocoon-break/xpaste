import type { LongProgress } from './api/shot';

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

/** 长截图提示放在边框哪一侧（M23 规则 13）：下方放得下放下方，否则上方，都放不下（贴满整屏）放在选区里的底部。
 *  提示在边框窗口里，不会被截进长截图。 */
export function infoSide(b: Box, vh: number, need = 48): 'below' | 'above' | 'inside' {
  if (vh - (b.y + b.h) >= need) return 'below';
  if (b.y >= need) return 'above';
  return 'inside';
}

/** 长截图预览的宽度（逻辑像素） */
export const PREVIEW_W = 240;
const PV_GAP = 14, EDGE = 8;

/** 长截图预览框的位置与最大高度（M23 规则 13）：放在边框右侧，右边放不下放左侧，都放不下（选区接近整屏宽）
 *  放在选区里的右上角（在边框窗口里，不会被截进去）。高与选区相同，至少 160、不超出屏幕；顶边与选区对齐。 */
export function previewBox(b: Box, vw: number, vh: number): Box {
  const h = Math.min(Math.max(b.h, 160), vh - 2 * EDGE);
  const y = Math.min(Math.max(b.y, EDGE), vh - h - EDGE);
  const w = PREVIEW_W + 2; // 含 1 px 描边
  let x = b.x + b.w + PV_GAP;
  if (x + w > vw - EDGE) x = b.x - PV_GAP - w;
  if (x < EDGE) x = b.x + b.w - w - EDGE;
  return { x, y, w, h };
}

const TEXT = {
  run: '滚动选区里的内容',
  lost: '滚得太快没接上，往回滚到预览里的位置',
  stuck: '这里一直接不上（画面在变？），↩ 保留已拼好的部分',
  full: '已到长度上限',
  // 自动滚动（M23 规则 13）
  auto: '自动滚动中，空格暂停',
  paused: '已暂停，空格继续',
  manual: '滚动选区里的内容 · 空格自动滚动',
  noperm: '滚动选区里的内容 · 授权辅助功能后可自动滚动',
  noresponse: '这个窗口不响应自动滚动，请自己滚动',
  pointer: '把指针移回选区继续',
  autostuck: '这里一直接不上（画面在变？），空格重试，↩ 保留已拼好的部分',
};
const SHORT = '选区较矮，慢一点滚';

/** 长截图选区旁的提示（M23 规则 13）。`short`：选区较矮，手动滚时提示慢一点滚（甩一下就超过整个选区，接不上）。 */
export function longTip(state: LongProgress['state'], short: boolean): string {
  if (short && state === 'run') return SHORT;
  if (short && state === 'manual') return `${SHORT} · 空格自动滚动`;
  return TEXT[state];
}
