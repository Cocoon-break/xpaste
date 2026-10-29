// 面板译文区块（M26 规则 3）。纯函数，状态由 main.ts 按记录 id 保存。
import type { RecordSummary } from './api/types';
import { esc } from './format.ts';

export type Tr = { text: string; lang: string; state: 'busy' | 'done' | 'cut' | 'fail'; error?: string };

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

const TITLE = { busy: '翻译中…', cut: '翻译中断', fail: '翻译失败' };

/** 默认展开、不可折叠；翻译中没有按钮，中断与失败只有「重新翻译」 */
export function trBlockHTML(tr: Tr, mono: boolean): string {
  const title = tr.state === 'done' ? `译文 · ${LANG_LABEL[tr.lang] ?? tr.lang}` : TITLE[tr.state];
  const again = '<button class="btn sm" type="button" data-tr="again">重新翻译</button>';
  const acts = tr.state === 'busy' ? ''
    : `<div class="tr-actions">${tr.state === 'done' ? '<button class="btn sm" type="button" data-tr="copy">复制译文</button>' : ''}${again}</div>`;
  const body = tr.text ? `<pre class="tr-text${mono ? ' mono' : ''}">${esc(tr.text)}</pre>` : '';
  const err = tr.state === 'cut' || tr.state === 'fail' ? `<p class="tr-err">${esc(tr.error ?? '')}</p>` : '';
  return `<section class="tr-block" data-state="${tr.state}"><header class="tr-head"><span>${esc(title)}</span>${acts}</header>${body}${err}</section>`;
}

export const trAfterDelta = (prev: Tr | undefined, lang: string, piece: string): Tr =>
  ({ text: (prev?.text ?? '') + piece, lang, state: 'busy' });

/** 成功以返回的完整译文为准；失败时已收到文字算中断（保留已收到的部分），否则算失败 */
export function trAfterEnd(prev: Tr | undefined, res: { ok: true; text: string; lang: string } | { ok: false; error: string }): Tr {
  if (res.ok) return { text: res.text, lang: res.lang, state: 'done' };
  const text = prev?.text ?? '';
  return { text, lang: prev?.lang ?? '', state: text ? 'cut' : 'fail', error: res.error };
}
