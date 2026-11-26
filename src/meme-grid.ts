// 面板“表情包”分类：网格里按格移动、排序、格子与右侧预览的 HTML。纯函数，不碰 DOM。
import type { TaskView } from './api/meme';
import type { RecordSummary } from './api/types';
import { esc, fmtInt, rel } from './format.ts';

export type Arrow = 'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight';

/** 按方向键移到的格子序号。前面有 `lead` 格不是表情包（“＋ 做一个”和任务格），表情包从第 `lead` 格起；不越过首尾 */
export function gridMove(index: number, count: number, cols: number, key: Arrow, lead: number): number {
  const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols }[key];
  return Math.max(0, Math.min(count + lead - 1, index + step));
}

/** 看得见的格子：同一套收成一格，显示这一套在排序里最靠前的那张，`count` 是这一套现有的张数；
 *  `openSet` 那一套摊开（`inSet`）。搜索时不叠，只剩一张的套也不叠 */
export function foldSets<T extends { id: number; set: string | null }>(list: T[], openSet: string | null, searching: boolean): { item: T; count: number; folded: boolean; inSet: boolean }[] {
  const sizes = new Map<string, number>();
  for (const it of list) if (it.set !== null) sizes.set(it.set, (sizes.get(it.set) ?? 0) + 1);
  const out: { item: T; count: number; folded: boolean; inSet: boolean }[] = [], seen = new Set<string>();
  for (const item of list) {
    const count = item.set === null ? 1 : sizes.get(item.set)!, grouped = !searching && count > 1;
    const inSet = grouped && item.set === openSet, folded = grouped && !inSet;
    if (folded && seen.has(item.set!)) continue;
    if (folded) seen.add(item.set!);
    out.push({ item, count, folded, inSet });
  }
  return out;
}

/** 网格里删一张时这一叠怎么换：删的是收着的一叠显示的那张（`restack`，重画时直接不画、不播淡出），这一叠换成这一套里
 *  排在后面、没在删的下一张，选中跟过去（`keep`；只剩两张时剩下那张单独成一格）。摊开的套、散的格子、一次删几张都不换，照常淡出 */
export function restackOnRemove<T extends { id: number; set: string | null }>(cells: { item: T; folded: boolean }[], list: T[], ids: number[], removing: Set<number>): { restack: number; keep: number } | null {
  if (ids.length !== 1) return null;
  const c = cells.find((c) => c.folded && c.item.id === ids[0]);
  const k = c && list.find((x) => x.id !== ids[0] && x.set === c.item.set && !removing.has(x.id));
  return k ? { restack: ids[0], keep: k.id } : null;
}

/** 用过的次数从多到少；次数相同按上次使用（copiedAt）从新到旧，从没用过的 copiedAt 就是创建时间 */
export const memeOrder = (a: RecordSummary, b: RecordSummary): number => b.uses - a.uses || b.copiedAt - a.copiedAt || b.id - a.id;

/** 网格上方的筛选：全部 / 做好的 / AI 出的 */
export type MemeSource = 'all' | 'final' | 'ai';
export const ofSource = (r: RecordSummary, s: MemeSource): boolean => s === 'all' || (s === 'ai') === (r.meme?.source === 'ai');

const sourceLabel = (r: RecordSummary) => (r.meme?.source === 'ai' ? 'AI 出的' : '做好的');
// 做好的没有文字时名字是“表情包”，这时下沿不写字
const caption = (r: RecordSummary) => (r.meme?.source === 'final' && r.meme.name !== '表情包' ? [...r.meme.name].slice(0, 6).join('') : '');

/** 点一格该做什么：`detail` > 1 是同一次双击的后几下，第一下可能已经摊开或收回、重排了网格，落点下面换了别的格子，
 *  所以忽略，双击贴的是第一下那张（`id`，忽略时沿用 `last`）。点到的格子所属的套和摊开的不同就换过去：
 *  属于一套就摊开它（open），不属于就收回（close）；相同时只选中 */
export function cellClick(hit: { id: number; set: string | null }, openSet: string | null, detail: number, last: number | null):
  { act: 'open' | 'close' | 'select' | 'ignore'; openSet: string | null; id: number | null } {
  if (detail > 1) return { act: 'ignore', openSet, id: last };
  if (hit.set === openSet) return { act: 'select', openSet, id: hit.id };
  return { act: hit.set === null ? 'close' : 'open', openSet: hit.set, id: hit.id };
}

/** 一格：静态缩略图（格子不播动图），左上 Mod+1–9 的编号，右上“AI”或“GIF”，做好的下沿写文字。
 *  `stack` 不为 0 时是收着的一套（右上写“N 张”，带叠起来的边）；`inSet` 是摊开的一套里的一格（一圈浅色边） */
export const cellHTML = (r: RecordSummary, i: number, sel: boolean, removing = false, stack = 0, inSet = false): string => {
  const m = r.meme!, cap = caption(r), tag = stack ? `${stack} 张` : m.source === 'ai' ? 'AI' : m.gif ? 'GIF' : '';
  const cls = (stack ? ' stack' : inSet ? ' inset' : '') + (removing ? ' is-removing' : '');
  const title = stack ? `一套 ${stack} 张，点开挑` : m.name;
  // 选项不是按钮：焦点留在搜索框（aria-activedescendant 指向选中的格子），点格子不会把键盘抢走
  return `<div class="mcell${cls}" role="option" id="mc-${r.id}" data-id="${r.id}" aria-selected="${sel}" aria-label="${esc(m.name)}，${sourceLabel(r)}${stack ? `，一套 ${stack} 张` : ''}" title="${esc(title)}">`
    + (r.thumb ? `<img src="${esc(r.thumb)}" alt="" draggable="false">` : '')
    + (cap ? `<span class="cp">${esc(cap)}</span>` : '')
    + (i < 9 ? `<span class="no">${i + 1}</span>` : '')
    + (tag ? `<span class="tag">${tag}</span>` : '')
    + '</div>';
};

/** 在画的尺子：60 秒走满一遍后从头走，按 30 格（2 秒一格）取整（同制作窗口）；返回百分比 */
const rulerPct = (ms: number) => (Math.floor(((ms / 1000) % 60) / 2) * 10) / 3;
const waitedMs = (t: TaskView, nowMs: number) => (t.startedMs === null ? 0 : Math.max(0, nowMs - t.startedMs));

/** 任务格上的字（换行是 \n）：在画写秒数，排队写前面几个，画好了只有缩略图和“新”（这句给读屏），没画出来叫人点开 */
export function taskCellText(t: TaskView, nowMs: number): string {
  if (t.state === 'run') return `AI 在画\n${Math.floor(waitedMs(t, nowMs) / 1000)} 秒`;
  if (t.state === 'queue') return `排队中\n${t.ahead ? `前面 ${t.ahead} 个` : '下一个'}`;
  return t.state === 'done' ? '画好了' : '没画出来\n点这里看';
}
const lines = (s: string) => esc(s).replace('\n', '<br>');
const CHECK = '<svg class="ok" viewBox="0 0 20 20" aria-hidden="true"><path d="m4.5 10.5 3.5 3.5 7.5-8"/></svg>';

/** 一个 AI 任务一格（按钮，不是选项）。`thumb` 是画好的结果的缩略图，拿不到时画一个对勾 */
export const taskCellHTML = (t: TaskView, nowMs: number, thumb: string | null): string => {
  const text = taskCellText(t, nowMs);
  const body = t.state === 'done' ? (thumb ? `<img src="${esc(thumb)}" alt="" draggable="false">` : CHECK) + '<span class="tag new">新</span>'
    : t.state === 'run' ? `<span class="ruler"><i style="width:${rulerPct(waitedMs(t, nowMs))}%"></i></span><span class="gt">${lines(text)}</span>`
    : `<span class="gt">${lines(text)}</span>`;
  const cls = t.state === 'done' ? ' done' : t.state === 'fail' ? ' fail' : '';
  return `<button class="mcell gen${cls}" type="button" data-task="${t.id}" aria-label="${esc(t.title)}，${esc(text.replace('\n', '，'))}" title="${esc(t.title)}">${body}</button>`;
};

/** 在画的格子每秒更新的部分：字和尺子的宽度 */
export const taskTick = (t: TaskView, nowMs: number): { html: string; width: string } =>
  ({ html: lines(taskCellText(t, nowMs)), width: `${rulerPct(waitedMs(t, nowMs))}%` });

/** 预览的元信息行：“做好的 · GIF · 240 × 240”或“AI 出的 · <风格> · PNG”；尺寸未知（0）时省略 */
export const memeMeta = (r: RecordSummary, w: number, h: number): string => {
  const m = r.meme!, fmt = m.gif ? 'GIF' : 'PNG';
  return (m.source === 'ai' ? ['AI 出的', m.style ?? '', fmt] : ['做好的', fmt, w && h ? `${w} × ${h}` : '']).filter(Boolean).join(' · ');
};

/** 右侧预览：大图（原文件，会动）、名字、元信息、用过几次与时间，按钮接着改、删除、粘贴。`nameHTML` 已转义并高亮 */
export const previewHTML = (r: RecordSummary, o: { nameHTML: string; src: string | null; w: number; h: number; verb: string; mod: string; now?: number }): string =>
  `<div class="mpv"><div class="mbig">${o.src ? `<img src="${esc(o.src)}" alt="${esc(r.meme!.name)}">` : ''}</div>`
  + `<h3 class="mnm">${o.nameHTML}</h3>`
  + `<p class="mmt">${esc(memeMeta(r, o.w, o.h))}<br>${r.uses ? `用过 ${fmtInt(r.uses)} 次 · ` : ''}${rel(r.copiedAt, o.now)}</p>`
  + `<div class="macts"><button class="btn" type="button" data-mact="edit">接着改<kbd>${o.mod} E</kbd></button>`
  + `<button class="btn" type="button" data-mact="del">删除</button>`
  + `<button class="btn primary" type="button" data-mact="paste"><span data-verb>${o.verb}</span><kbd>Enter</kbd></button></div></div>`;
