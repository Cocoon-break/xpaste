// 来源应用筛选（M29）：搜索框里的来源胶囊与“只看 xx”的建议
import type { RecordSummary } from './api/types.ts';

/** 只留来自 `app` 的历史记录；没有胶囊（null）时原样返回。片段不算来源（M29 规则 3） */
export const fromApp = (rs: RecordSummary[], app: string | null): RecordSummary[] =>
  app === null ? rs : rs.filter((r) => r.app?.name === app && r.snippetBody == null);

/** 各应用的记录数，按第一次出现的顺序 */
export const appCounts = (rs: RecordSummary[]): Map<string, number> => {
  const c = new Map<string, number>();
  for (const r of rs) if (r.app && r.snippetBody == null) c.set(r.app.name, (c.get(r.app.name) ?? 0) + 1);
  return c;
};

/** 建议哪个应用（M29 规则 1）：对得上的应用里记录最多的；一样多取名字短的；一条都没有的不建议 */
export const bestApp = (names: string[], counts: Map<string, number>): { app: string; n: number } | null => {
  const n = (a: string) => counts.get(a) ?? 0;
  const app = names.filter((a) => n(a) > 0).sort((a, b) => n(b) - n(a) || a.length - b.length || a.localeCompare(b))[0];
  return app === undefined ? null : { app, n: n(app) };
};
