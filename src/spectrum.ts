// 复制频谱（M14）的纯计算：位置、高度、按列合并光柱、统计。DOM 留在 main.ts
import type { Kind, RecordSummary } from './api/types.ts';

export const DAY = 864e5;
/** 每根光柱占的最小列宽（px），条数再多也不超过 轨道宽度 / COL_PX */
export const COL_PX = 2;

/** 左侧 6% 是“更早”区（1–7 天），其余 94% 映射最近 24 小时；超过 7 天没有位置 */
export function xPct(age: number): number | null {
  if (age > 7 * DAY) return null;
  if (age > DAY) return Math.max(1, 5 - ((age - DAY) / (6 * DAY)) * 4);
  return Math.min(99.4, 6 + 94 * (1 - age / DAY));
}

export const hPct = (size: number) => 16 + Math.min(1, Math.log10(Math.max(10, size)) / 8) * 84;

// —— 时间窗（阶段 11）：放大后整条轨道映射过去 24 小时内的一段 ——
export const MIN_SPAN = 5 * 60e3;
export const HOUR = 36e5;
/** 放大后的时间窗；pinned：右端贴着 now，随时间滑动 */
export interface Win { start: number; end: number; pinned: boolean }
export const spanOf = (w: Win) => w.end - w.start;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** 夹紧：长度夹到 [MIN_SPAN, DAY]，范围夹到 [now − DAY, now]（保长度整体贴边）；长度达 DAY 返回 null。pinned = end ≥ now − 1000 */
export function clampWin(start: number, end: number, now: number): Win | null {
  const span = clamp(end - start, MIN_SPAN, DAY);
  if (span >= DAY) return null;
  start = end - span; // 补足最短长度时保住右端
  if (start < now - DAY) { start = now - DAY; end = start + span; }
  if (end > now) { end = now; start = end - span; }
  return { start, end, pinned: end >= now - 1000 };
}

/** pinned 窗口随时间走：end = now，start = now − 长度；非 pinned 原样返回 */
export const followNow = (w: Win, now: number): Win => (w.pinned ? { start: now - spanOf(w), end: now, pinned: true } : w);

// 默认视图下指针在 24 h 区内的比例（“更早”区按最左端）
const dayFrac = (fracX: number) => clamp((fracX - 0.06) / 0.94, 0, 1);

/** 轨道横向比例（0–1）对应的时间：默认视图只在 24 h 区内有意义，比例 < 0.06 按 now − DAY */
export function timeAt(w: Win | null, fracX: number, now: number): number {
  return w ? w.start + fracX * spanOf(w) : now - DAY * (1 - dayFrac(fracX));
}

/** 围绕 fracX 处的时间缩放：新长度 = 当前长度 × factor（默认视图当前长度为 DAY），指针下的时间保持在指针下 */
export function zoomWin(w: Win | null, fracX: number, factor: number, now: number): Win | null {
  // 先夹长度再定位，指针下的时间才不会因补足最短长度而偏走
  const span = clamp((w ? spanOf(w) : DAY) * factor, MIN_SPAN, DAY);
  const start = timeAt(w, fracX, now) - span * (w ? fracX : dayFrac(fracX));
  return clampWin(start, start + span, now);
}

/** 平移 dt 毫秒（正值向“现在”），经 clampWin */
export const panWin = (w: Win, dt: number, now: number) => clampWin(w.start + dt, w.end + dt, now);

/** 以 center 为中心、长度 span 的窗口，经 clampWin；center 在 now − DAY 之前时按 now − DAY 对待 */
export function centerWin(center: number, span: number, now: number): Win | null {
  const c = Math.max(center, now - DAY);
  return clampWin(c - span / 2, c + span / 2, now);
}

/** 记录时间 → 轨道百分比：w 为 null 用 xPct(now − t)；否则线性映射，落在 [−1, 101] 之外返回 null */
export function xOf(w: Win | null, copiedAt: number, now: number): number | null {
  if (!w) return xPct(now - copiedAt);
  const x = ((copiedAt - w.start) * 100) / spanOf(w); // 先乘后除，整分钟处得到整数
  return x < -1 || x > 101 ? null : x;
}

// [窗口长度上限, 主刻度, 次刻度]（设计 §4.5），主刻度数 ≤ 8
const STEPS: [number, number, number][] = [
  [8 * 60e3, 60e3, 15e3], [16 * 60e3, 2 * 60e3, 30e3], [40 * 60e3, 5 * 60e3, 60e3], [80 * 60e3, 10 * 60e3, 2 * 60e3],
  [2 * HOUR, 15 * 60e3, 5 * 60e3], [4 * HOUR, 30 * 60e3, 10 * 60e3], [8 * HOUR, HOUR, 15 * 60e3],
  [16 * HOUR, 2 * HOUR, 30 * 60e3], [DAY, 3 * HOUR, HOUR],
];
/** 主 / 次刻度步长（ms），主刻度数 ≤ 8：见设计 §4.5 的表 */
export function tickStep(span: number): [number, number] {
  const [, major, minor] = STEPS.find(([max]) => span <= max) ?? STEPS[STEPS.length - 1];
  return [major, minor];
}

/** 放大态刻度：t 按本地时间对齐到步长整数倍，x 为百分比；major 为主刻度。贴 now（end ≥ now − 1000）时末尾追加 { x: 100, t: now, major: true, now: true } */
export function ticks(w: Win, now: number): { x: number; t: number; major: boolean; now?: boolean }[] {
  const span = spanOf(w), [major, minor] = tickStep(span);
  const out: { x: number; t: number; major: boolean; now?: boolean }[] = [];
  // 主步长都是次步长的整数倍：沿次刻度走一遍，落在主步长整数倍上的就是主刻度（不再重复出次刻度）
  // 按本地时间对齐（+5:30 等时区整点不是 UTC 整点）；ponytail: 偏移只取窗口起点的，窗内跨夏令时切换时后半段刻度差一个偏移量
  const off = new Date(w.start).getTimezoneOffset() * 60e3;
  for (let t = Math.ceil((w.start - off) / minor) * minor + off; t <= w.end; t += minor)
    out.push({ x: ((t - w.start) * 100) / span, t, major: (t - off) % major === 0 });
  if (w.end >= now - 1000) out.push({ x: 100, t: now, major: true, now: true });
  return out;
}

/** 概览条窗框：相对 24 h 的 left / width 百分比，width 最小 0.6 */
export function miniWin(w: Win, now: number): { left: number; width: number } {
  const width = Math.max(0.6, (spanOf(w) * 100) / DAY);
  // 最窄宽度撑出来的部分不越过右端
  return { left: Math.min(100 - width, ((w.start - (now - DAY)) * 100) / DAY), width };
}

/** 放大态标题行文字：`HH:mm – HH:mm（N 小时 | N 分钟）`，整小时不带小数，非整小时一位小数 */
export function winLabel(w: Win, fmt: (t: number) => string): string {
  const span = spanOf(w);
  const dur = span % HOUR === 0 ? `${span / HOUR} 小时` : span >= HOUR ? `${(span / HOUR).toFixed(1)} 小时` : `${Math.round(span / 60e3)} 分钟`;
  return `${fmt(w.start)} – ${fmt(w.end)}（${dur}）`;
}

export interface Bar { id: number; kind: Kind; x: number; h: number; dim: boolean; fresh: boolean; ids: number[] }
/** colOf：记录 id → bars 下标 */
export interface Layout { bars: Bar[]; colOf: Map<number, number> }

type Cand = { r: RecordSummary; x: number; h: number; m: boolean };
// 代表记录：匹配筛选 > 新记录 > 更高 > 更新 > id 更大
const better = (a: Cand, b: Cand, freshId?: number) =>
  a.m !== b.m ? a.m
    : (a.r.id === freshId) !== (b.r.id === freshId) ? a.r.id === freshId
      : a.h !== b.h ? a.h > b.h
        : a.r.copiedAt !== b.r.copiedAt ? a.r.copiedAt > b.r.copiedAt
          : a.r.id > b.r.id;

export function layoutBars(recs: Iterable<RecordSummary>, now: number, widthPx: number, isMatch: (r: RecordSummary) => boolean, freshId?: number, win?: Win | null): Layout {
  const cols = Math.max(1, Math.floor(widthPx / COL_PX));
  const groups = new Map<number, { best: Cand; ids: number[]; anyMatch: boolean; fresh: boolean }>();
  for (const r of recs) {
    const x = xOf(win ?? null, r.copiedAt, now);
    if (x === null) continue;
    const c: Cand = { r, x, h: hPct(r.size), m: isMatch(r) };
    const col = clamp(Math.floor((x / 100) * cols), 0, cols - 1); // 窗内 x 可略出 [0, 100]
    const g = groups.get(col);
    if (!g) groups.set(col, { best: c, ids: [r.id], anyMatch: c.m, fresh: r.id === freshId });
    else {
      g.ids.push(r.id);
      g.anyMatch ||= c.m;
      g.fresh ||= r.id === freshId;
      if (better(c, g.best, freshId)) g.best = c;
    }
  }
  // 列号单调对应 x，按列排即按 x 升序
  const bars: Bar[] = [];
  const colOf = new Map<number, number>();
  for (const col of [...groups.keys()].sort((a, b) => a - b)) {
    const { best, ids, anyMatch, fresh } = groups.get(col)!;
    for (const id of ids) colOf.set(id, bars.length);
    bars.push({ id: best.r.id, kind: best.r.kind, x: best.x, h: best.h, dim: !anyMatch, fresh, ids });
  }
  return { bars, colOf };
}

/** 两次布局的列是否一样（列数与每列包含的记录都相同）：一样就能原地改光柱，让 dim/on 的过渡播出来 */
export const sameColumns = (a: Layout, b: Layout) =>
  a.bars.length === b.bars.length &&
  a.bars.every((x, i) => x.ids.length === b.bars[i].ids.length && x.ids.every((id, j) => id === b.bars[i].ids[j]));

/** 24 小时内条数，及按本地小时计的最忙时段（最多一次的小时 > 1 次才给；并列取较早的小时） */
export function spectrumStat(recs: Iterable<RecordSummary>, now: number): { count: number; peak: { hour: number; n: number } | null } {
  const hours = new Array<number>(24).fill(0);
  let count = 0;
  for (const r of recs) {
    if (now - r.copiedAt > DAY) continue;
    count++;
    hours[new Date(r.copiedAt).getHours()]++;
  }
  const n = Math.max(...hours);
  return { count, peak: n > 1 ? { hour: hours.indexOf(n), n } : null };
}

/** 首次打开时光柱升起的序号：条数多时压到 0–20，总时长不随条数增长 */
export const staggerIndex = (i: number, n: number) => (n <= 21 ? i : Math.round((i * 20) / (n - 1)));
