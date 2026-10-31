// 面板译文区块（M26 规则 3）。纯函数，状态由 main.ts 按记录 id 保存。
import type { Engines, RecordSummary } from './api/types';
import { esc } from './format.ts';

/** engine：翻译方式标签 `system` / `openai:{模型 ID}`（设计 §3.4） */
export type Tr = { text: string; lang: string; engine: string; state: 'busy' | 'done' | 'cut' | 'fail'; error?: string };
export type Engine = 'system' | 'openai';

export const LANG_LABEL: Record<string, string> = {
  'zh-Hans': '简体中文',
  'zh-Hant': '繁體中文',
  en: 'English',
  ja: '日本語',
  ko: '한국어',
};

/** 文本、代码（含片段）可翻译；图片、文件在「图中文字」有文字时翻译识别文字（M26 规则 1） */
export const translatable = (r: Pick<RecordSummary, 'kind' | 'ocrState'>): boolean =>
  r.kind === 'text' || r.kind === 'code' || ((r.kind === 'image' || r.kind === 'file') && r.ocrState === 1);

/** 翻译方式标注，规则同 Rust 的 `engine_label`（设计 §3.4） */
export function engineLabel(tag: string): string {
  if (tag === 'system') return '系统翻译';
  if (!tag.startsWith('openai:')) return '';
  const m = tag.slice(7);
  return m ? `AI（${m}）` : 'AI';
}

/** 能换用的另一种：只在另一种能用时给出，否则 null（设计 §3.4） */
export function otherEngine(tag: string, avail: Engines): Engine | null {
  if (tag === 'system') return avail.openai ? 'openai' : null;
  if (tag.startsWith('openai:')) return avail.system ? 'system' : null;
  return null;
}

const TITLE = { busy: '翻译中…', cut: '翻译中断', fail: '翻译失败' };
const MODEL_MAX = 24;
const SWITCH = { system: '换用系统翻译', openai: '换用 AI 翻译' };

/** 默认展开、不可折叠；翻译中没有按钮。标题行带翻译方式，模型 ID 太长截断、悬停看完整值；
 *  other 为能换用的另一种，收完、失败、中断后多一个换用按钮（设计 §3.4） */
export function trBlockHTML(tr: Tr, mono: boolean, other: Engine | null): string {
  const full = engineLabel(tr.engine);
  const model = [...tr.engine.slice(7)];
  const label = tr.engine.startsWith('openai:') && model.length > MODEL_MAX ? `AI（${model.slice(0, MODEL_MAX).join('')}…）` : full;
  const title = (tr.state === 'done' ? `译文 · ${LANG_LABEL[tr.lang] ?? tr.lang}` : TITLE[tr.state]) + (label ? ` · ${label}` : '');
  const tip = label !== full ? ` title="${esc(full)}"` : '';
  const again = '<button class="btn sm" type="button" data-tr="again">重新翻译</button>';
  const sw = other ? `<button class="btn sm" type="button" data-tr="switch" data-engine="${other}">${SWITCH[other]}</button>` : '';
  const acts = tr.state === 'busy' ? ''
    : `<div class="tr-actions">${tr.state === 'done' ? '<button class="btn sm" type="button" data-tr="copy">复制译文</button>' : ''}${again}${sw}</div>`;
  const body = tr.text ? `<pre class="tr-text${mono ? ' mono' : ''}">${esc(tr.text)}</pre>` : '';
  const err = tr.state === 'cut' || tr.state === 'fail' ? `<p class="tr-err">${esc(tr.error ?? '')}</p>` : '';
  return `<section class="tr-block" data-state="${tr.state}"><header class="tr-head"><span${tip}>${esc(title)}</span>${acts}</header>${body}${err}</section>`;
}

export const trAfterDelta = (prev: Tr | undefined, lang: string, engine: string, piece: string): Tr =>
  ({ text: (prev?.text ?? '') + piece, lang, engine, state: 'busy' });

/** 成功以返回的完整译文为准；失败时已收到文字算中断（保留已收到的部分），否则算失败。失败时 engine 是请求时的方式 */
export function trAfterEnd(prev: Tr | undefined, res: { ok: true; text: string; lang: string; engine: string } | { ok: false; error: string; engine: string }): Tr {
  if (res.ok) return { text: res.text, lang: res.lang, engine: res.engine, state: 'done' };
  const text = prev?.text ?? '';
  return { text, lang: prev?.lang ?? '', engine: res.engine, state: text ? 'cut' : 'fail', error: res.error };
}
