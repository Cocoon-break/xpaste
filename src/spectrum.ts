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

export function layoutBars(recs: Iterable<RecordSummary>, now: number, widthPx: number, isMatch: (r: RecordSummary) => boolean, freshId?: number): Layout {
  const cols = Math.max(1, Math.floor(widthPx / COL_PX));
  const groups = new Map<number, { best: Cand; ids: number[]; anyMatch: boolean; fresh: boolean }>();
  for (const r of recs) {
    const x = xPct(now - r.copiedAt);
    if (x === null) continue;
    const c: Cand = { r, x, h: hPct(r.size), m: isMatch(r) };
    const col = Math.min(cols - 1, Math.floor((x / 100) * cols));
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
