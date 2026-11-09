// 精简模式（M27）：单行列表每行显示什么、怎么分组。纯函数，不碰 DOM。
import type { RecordSummary } from './api/types';
import { esc, fmtDur, hl, host, isHttpUrl, isSnip, type Filter } from './format.ts';

/** 地址里原样的域名与其余部分：`URL.host` 会转小写、把中文域名转成 xn--，拿它去切原文会切错 */
const splitUrl = (url: string): [string, string] => {
  const rest = url.replace(/^https?:\/\//i, '');
  const i = rest.search(/[/?#]/);
  return i < 0 ? [rest, ''] : [rest.slice(0, i), rest.slice(i)];
};
import { snippetHTML } from './ocr-label.ts';

/** 行高、小标题高、置顶后细线占的高（M27 规则 4、6） */
export const COMPACT = { row: 36, label: 24, rule: 9 } as const;

/** `title`、`extra` 是已转义、已高亮的 HTML。`ocr`：标题位置是图中文字的命中片段（调用方在前面加“图中文字”标签）；
 *  `rev`：没有页面标题的链接，标题（域名）不截断、补充（路径）截断。 */
export type RowParts = { title: string; extra: string; ocr: boolean; rev: boolean };

/** 补充按类型（M27 规则 4），搜索命中时按命中位置换掉（规则 5）。`mark` 是拼音命中的汉字，`snippet` 是图中文字的命中片段。 */
export const rowParts = (r: RecordSummary, { q, mark, snippet }: { q: string; mark?: string; snippet?: string }): RowParts => {
  const term = (mark ?? q).trim();
  if (snippet) return { title: snippetHTML(snippet, term), extra: '', ocr: true, rev: false };
  let title = hl(r.title, term), extra = '', rev = false;
  if (isSnip(r)) extra = esc(r.snippetBody!.split('\n')[0]);
  else if (r.kind === 'link' && r.url) {
    if (r.title === r.url && isHttpUrl(r.url)) {
      const [h, path] = splitUrl(r.url);
      title = hl(h, term);
      extra = hl(path, term);
      rev = true;
    } else extra = esc(host(r.url));
  } else if ((r.kind === 'text' || r.kind === 'code') && (r.lines ?? 0) > 1) extra = `${r.lines} 行`;
  else if (r.kind === 'video' && r.durationSec !== null) extra = fmtDur(r.durationSec);
  else if (r.kind === 'file' && (r.fileCount ?? 0) > 1) extra = `等 ${r.fileCount} 个文件`;
  const qq = q.trim(), has = (s: string, n: string) => s.toLowerCase().includes(n.toLowerCase());
  if (qq && !has(r.title, term)) {
    const app = r.app?.name;
    extra = app && has(app, qq) ? `来自 ${hl(app, qq)}` : `内容含“<mark>${esc(qq)}</mark>”`;
  }
  return { title, extra, ocr: false, rev };
};

/** 不按日期分组（M27 规则 6）：片段、置顶、其余记录各一组；“常用”只有一组。`vis` 已按 `visible` 排好序。 */
export const compactGroups = (vis: RecordSummary[], f: Filter): { name: string; rows: RecordSummary[] }[] => {
  if (f === 'fav') return vis.length ? [{ name: '按使用次数排序', rows: vis }] : [];
  const out: { name: string; rows: RecordSummary[] }[] = [];
  for (const r of vis) {
    const name = isSnip(r) ? '片段' : r.pinned ? '置顶' : '记录';
    if (out[out.length - 1]?.name !== name) out.push({ name, rows: [] });
    out[out.length - 1].rows.push(r);
  }
  return out;
};

/** 第 `gi` 组标题的高：有片段组时每组都有小标题；否则第一组没有，之后的是一条细线。 */
export const compactHeadH = (gs: { name: string }[], gi: number): number =>
  gs.some((g) => g.name === '片段') ? COMPACT.label : gi === 0 ? 0 : COMPACT.rule;
