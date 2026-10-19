// 虚拟列表布局：分组 → 定高 cell 序列，按滚动位置算出要渲染的区间。纯函数。

export const ROW_H = 56;
export const HEAD_H = 34;
export const OVERSCAN = 8;

/** `index` 为该行在 `vis` 中的序号，供 1–9 序号与 `--i`。 */
export type Cell =
  | { kind: 'head'; name: string; count: number; top: number; gi: number }
  | { kind: 'row'; id: number; index: number; top: number; gi: number };

const height = (c: Cell) => (c.kind === 'head' ? HEAD_H : ROW_H);

export const layout = (groups: { name: string; rows: { id: number }[] }[]): { cells: Cell[]; total: number } => {
  const cells: Cell[] = [];
  let top = 0, index = 0;
  groups.forEach((g, gi) => {
    cells.push({ kind: 'head', name: g.name, count: g.rows.length, top, gi });
    top += HEAD_H;
    for (const r of g.rows) {
      cells.push({ kind: 'row', id: r.id, index: index++, top, gi });
      top += ROW_H;
    }
  });
  return { cells, total: top };
};

/** 要渲染的半开区间 `[start, end)`：与视口相交的 cell 上下各扩 `overscan` 个。 */
export const range = (cells: Cell[], scrollTop: number, viewportH: number, overscan = OVERSCAN): [start: number, end: number] => {
  // 二分找首个底边越过 scrollTop 的 cell（cells 按 top 升序）
  let lo = 0, hi = cells.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (cells[mid].top + height(cells[mid]) > scrollTop) hi = mid;
    else lo = mid + 1;
  }
  const first = lo, bottom = scrollTop + viewportH;
  let last = first - 1;
  while (last + 1 < cells.length && cells[last + 1].top < bottom) last++;
  return [Math.max(0, first - overscan), Math.min(cells.length, last + 1 + overscan)];
};

export const topOf = (cells: Cell[], id: number): number | undefined =>
  cells.find((c) => c.kind === 'row' && c.id === id)?.top;

/** 让 `[top, top + h]` 完整可见的最小滚动位置；已可见时原样返回。 */
export const scrollTo = (top: number, h: number, scrollTop: number, viewportH: number): number =>
  top < scrollTop ? top : top + h > scrollTop + viewportH ? top + h - viewportH : scrollTop;
