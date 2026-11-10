// 来源应用筛选（M29）：搜索框里的来源胶囊与“只看 xx”的建议
import type { RecordSummary } from './api/types.ts';
import { isSnip } from './format.ts';

/** 只留来自 `app` 的历史记录；没有胶囊（null）时原样返回。片段不算来源（M29 规则 3） */
export const fromApp = (rs: RecordSummary[], app: string | null): RecordSummary[] =>
  app === null ? rs : rs.filter((r) => r.app?.name === app && !isSnip(r));

/** 各应用的记录数，按第一次出现的顺序 */
export const appCounts = (rs: RecordSummary[]): Map<string, number> => {
  const c = new Map<string, number>();
  for (const r of rs) if (r.app?.name && !isSnip(r)) c.set(r.app.name, (c.get(r.app.name) ?? 0) + 1);
  return c;
};

/** 建议哪个应用（M29 规则 1）：对得上的应用里记录最多的；一样多取名字短的；一条都没有的不建议 */
export const bestApp = (names: string[], counts: Map<string, number>): { app: string; n: number } | null => {
  const n = (a: string) => counts.get(a) ?? 0;
  const app = names.filter((a) => n(a) > 0).sort((a, b) => n(b) - n(a) || a.length - b.length || a.localeCompare(b))[0];
  return app === undefined ? null : { app, n: n(app) };
};

export type Sug = { app: string; n: number; /** 这条建议是为哪段输入（去首尾空白）算的 */ q: string };

/** 建议是否还对得上当前输入：比对结果异步回来，输入变了而新结果没到时旧建议不算数，Tab 不接受（M29 规则 2） */
export const freshSug = (sug: Sug | null, q: string): Sug | null => (sug && sug.q === q.trim() ? sug : null);
