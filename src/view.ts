// 视图状态：过滤、排序、分组、选中、撤销。纯函数，不碰 DOM。
import type { RecordSummary } from './api/types';
import { FILTERS, isFav, type Filter } from './format.ts';

/** `min`：进入“常用”的使用次数（设置 frequentMinUses）。 */
export const inFilter = (r: RecordSummary, f: Filter, min: number): boolean =>
  f === 'all' || (f === 'fav' ? isFav(r, min) : r.kind === f);

/** 分类 ∩ 搜索结果（`null` 表示无搜索），常用按次数排，其余置顶优先再按时间、id 倒序。 */
export const visible = (
  records: Map<number, RecordSummary>,
  f: Filter,
  matchIds: Set<number> | null,
  min: number,
): RecordSummary[] =>
  [...records.values()]
    .filter((r) => inFilter(r, f, min) && (matchIds === null || matchIds.has(r.id)))
    .sort(
      f === 'fav'
        ? (a, b) => b.uses - a.uses || b.copiedAt - a.copiedAt
        : (a, b) => Number(b.pinned) - Number(a.pinned) || b.copiedAt - a.copiedAt || b.id - a.id,
    );

const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();

export const groupOf = (r: RecordSummary, f: Filter, now = Date.now()): string => {
  if (f === 'fav') return '按使用次数排序';
  if (r.pinned) return '置顶';
  const t = new Date(r.copiedAt), d = new Date(now);
  if (sameDay(t, d)) return '今天';
  d.setDate(d.getDate() - 1);
  return sameDay(t, d) ? '昨天' : '更早';
};

/** 按 `groupOf` 把已排序的 `vis` 切成连续分组。 */
export const groups = (vis: RecordSummary[], f: Filter, now = Date.now()): { name: string; rows: RecordSummary[] }[] => {
  const out: { name: string; rows: RecordSummary[] }[] = [];
  for (const r of vis) {
    const name = groupOf(r, f, now);
    if (out[out.length - 1]?.name !== name) out.push({ name, rows: [] });
    out[out.length - 1].rows.push(r);
  }
  return out;
};

/** 各分类条数，基于全部记录，不受搜索影响。 */
export const counts = (records: Map<number, RecordSummary>, min: number): Record<Filter, number> => {
  const c = Object.fromEntries(FILTERS.map(([f]) => [f, 0])) as Record<Filter, number>;
  for (const r of records.values()) {
    c.all++;
    c[r.kind]++;
    if (isFav(r, min)) c.fav++;
  }
  return c;
};

/** 删除 `removedId` 后应选中的：下一条，否则上一条，否则 `null`。 */
export const nextSelection = (vis: RecordSummary[], removedId: number): number | null => {
  const i = vis.findIndex((r) => r.id === removedId);
  if (i < 0) return null;
  return vis[i + 1]?.id ?? vis[i - 1]?.id ?? null;
};

export const ensureSelection = (vis: RecordSummary[], sel: number | null): number | null =>
  vis.some((r) => r.id === sel) ? sel : (vis[0]?.id ?? null);

export const cycleFilter = (f: Filter, dir: 1 | -1): Filter => {
  const i = FILTERS.findIndex(([k]) => k === f);
  return FILTERS[(i + dir + FILTERS.length) % FILTERS.length][0];
};

/** 异步结果的序号早于最新请求时丢弃。 */
export const isStale = (seq: number, latest: number): boolean => seq < latest;

export const UNDO_MS = 5000;
export type UndoState = { id: number; title: string; expiresAt: number } | null;

/** 只保留最后一次删除：总是替换 `prev`。 */
export const armUndo = (_prev: UndoState, id: number, title: string, now: number): UndoState => ({
  id,
  title,
  expiresAt: now + UNDO_MS,
});

export const canUndo = (u: UndoState, now: number): boolean => u !== null && now < u.expiresAt;
