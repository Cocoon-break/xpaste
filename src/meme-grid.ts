// 面板“表情包”分类：网格里按格移动、排序、格子与右侧预览的 HTML。纯函数，不碰 DOM。
import type { RecordSummary } from './api/types';
import { esc, fmtInt, rel } from './format.ts';

export type Arrow = 'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight';

/** 按方向键移到的格子序号。`hasAdd` 时“＋ 做一个”占第 0 格，表情包从第 1 格起；不越过首尾 */
export function gridMove(index: number, count: number, cols: number, key: Arrow, hasAdd: boolean): number {
  const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols }[key];
  return Math.max(0, Math.min(count - (hasAdd ? 0 : 1), index + step));
}

/** 用过的次数从多到少；次数相同按上次使用（copiedAt）从新到旧，从没用过的 copiedAt 就是创建时间 */
export const memeOrder = (a: RecordSummary, b: RecordSummary): number => b.uses - a.uses || b.copiedAt - a.copiedAt || b.id - a.id;

/** 网格上方的筛选：全部 / 做好的 / AI 出的 */
export type MemeSource = 'all' | 'final' | 'ai';
export const ofSource = (r: RecordSummary, s: MemeSource): boolean => s === 'all' || (s === 'ai') === (r.meme?.source === 'ai');

const sourceLabel = (r: RecordSummary) => (r.meme?.source === 'ai' ? 'AI 出的' : '做好的');
// 做好的没有文字时名字是“表情包”，这时下沿不写字
const caption = (r: RecordSummary) => (r.meme?.source === 'final' && r.meme.name !== '表情包' ? [...r.meme.name].slice(0, 6).join('') : '');

/** 一格：静态缩略图（格子不播动图），左上 Mod+1–9 的编号，右上“AI”或“GIF”，做好的下沿写文字 */
export const cellHTML = (r: RecordSummary, i: number, sel: boolean, removing = false): string => {
  const m = r.meme!, cap = caption(r), tag = m.source === 'ai' ? 'AI' : m.gif ? 'GIF' : '';
  // 选项不是按钮：焦点留在搜索框（aria-activedescendant 指向选中的格子），点格子不会把键盘抢走
  return `<div class="mcell${removing ? ' is-removing' : ''}" role="option" id="mc-${r.id}" data-id="${r.id}" aria-selected="${sel}" aria-label="${esc(m.name)}，${sourceLabel(r)}" title="${esc(m.name)}">`
    + (r.thumb ? `<img src="${esc(r.thumb)}" alt="" draggable="false">` : '')
    + (cap ? `<span class="cp">${esc(cap)}</span>` : '')
    + (i < 9 ? `<span class="no">${i + 1}</span>` : '')
    + (tag ? `<span class="tag">${tag}</span>` : '')
    + '</div>';
};

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
