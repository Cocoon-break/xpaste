// 图中文字：信息栏字段与列表命中片段。纯函数。
import { esc, fmtInt } from './format.ts';

const LABELS = ['识别中…', '', '无文字', '识别失败', '已跳过（图片过大）', '本机不支持'];

/** 状态见 `RecordSummary.ocrState`；1（有文字）给字数，按字符计。 */
export const ocrLabel = (state: number, text?: string | null): string =>
  state === 1 ? `${fmtInt([...(text ?? '')].length)} 字` : (LABELS[state] ?? '');

/** 命中前保留的字符数：列表元信息一行放不下整段片段，关键词要落在省略号之前 */
const LEAD = 4;

/** 转义后把第一处 `q`（按字面、忽略大小写）包进 `<mark>`，命中前超过 `LEAD` 个字符时截成 `…` + 最后几个；
 *  `q` 只用来定位，输出里只有片段原文的转义。 */
export const snippetHTML = (snippet: string, q: string): string => {
  const needle = q.trim();
  const m = needle ? new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i').exec(snippet) : null;
  if (!m) return esc(snippet);
  const pre = [...snippet.slice(0, m.index)];
  const head = pre.length > LEAD ? '…' + pre.slice(-LEAD).join('') : pre.join('');
  return `${esc(head)}<mark>${esc(m[0])}</mark>${esc(snippet.slice(m.index + m[0].length))}`;
};
