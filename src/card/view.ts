// 划词翻译卡片（设计 §3.2、§3.4）的整张 HTML，截图译文卡片也用 cardInner。纯函数，状态由各自的 main.ts 保存。
import { esc } from '../format.ts';
import { engineLabel, type Engine } from '../translate.ts';

/** empty / blocked：2 秒后消失的提示；其余为翻译卡片。engine：翻译方式标签；other：能换用的另一种（otherEngine）；
 *  errorKind：失败时的 AppError.kind；copied：复制成功，900 ms 后关闭 */
export type CardState =
  | { kind: 'empty'; hotkey: string | null }
  | { kind: 'blocked' }
  | {
    kind: 'busy' | 'done' | 'fail';
    source: string;
    engine: string;
    text: string;
    noAccess: boolean;
    other: Engine | null;
    error?: string;
    errorKind?: string;
    copied?: boolean;
  };

const SOURCE_CHARS = 40;
const SWITCH = { system: '换用系统翻译', openai: '换用 AI 翻译' };
const btn = (act: string, label: string, extra = '') => `<button type="button" data-act="${act}"${extra}>${label}</button>`;

/** 原文提示：前 40 个字，超出加“…”（后端已截过一次，再截结果不变） */
export function clipSource(s: string): string {
  const cs = [...s];
  return cs.length > SOURCE_CHARS ? cs.slice(0, SOURCE_CHARS).join('') + '…' : s;
}

export function cardHTML(s: CardState): string {
  if (s.kind === 'empty' || s.kind === 'blocked') {
    const msg = s.kind === 'empty' ? `没有可翻译的文字：先选中文字再按 ${s.hotkey ?? '翻译快捷键'}` : '这个应用里的内容不翻译';
    return `<div class="card hint" role="status"><p class="c-msg">${esc(msg)}</p></div>`;
  }
  return `<div class="card" role="dialog" aria-label="译文">${cardInner(s)}</div>`;
}

/** 翻译卡片的内容（不含外层 .card）；截图译文卡片也用它，原文为空时不出原文提示（设计 §3.4） */
export function cardInner(s: Extract<CardState, { kind: 'busy' | 'done' | 'fail' }>): string {
  const way = engineLabel(s.engine);
  const head = s.kind === 'busy' ? '翻译中…' : s.kind === 'done' ? '译文' : s.text ? '翻译中断' : '翻译失败';
  const title = way ? `${head} · ${way}` : head;
  const body = s.text ? `<div class="c-body">${esc(s.text)}</div>` : s.kind === 'busy' ? '<div class="c-body c-wait">翻译中…</div>' : '';
  const err = s.kind !== 'busy' && s.error ? `<div class="c-err">${esc(s.error)}</div>` : '';
  const note = s.noAccess ? `<div class="c-note"><span>没有辅助功能权限，翻译的是剪切板里的文字</span>${btn('access', '前往授权')}</div>` : '';
  let foot: string;
  if (s.copied) foot = '<div class="c-foot"><span class="c-ok" role="status">已复制译文</span></div>';
  else {
    // 换用、下载语言包、去设置只在收完或失败后出现（增量不带序号，翻译中不给重翻的入口）
    const extra = s.kind === 'busy' ? ''
      : (s.kind === 'fail' && s.errorKind === 'translatePack' ? btn('pack', '下载语言包') : '')
      + (s.kind === 'fail' && (s.errorKind === 'translateConfig' || s.errorKind === 'translateOs') ? btn('settings', '去设置') : '')
      + (s.other ? btn('switch', SWITCH[s.other], ` data-engine="${s.other}"`) : '');
    // 复制译文收完才可用；失败时只能关闭，不放复制（设计 §3.2）
    const copy = s.kind === 'fail' ? ''
      : `<button type="button" class="go" data-act="copy" aria-disabled="${s.kind !== 'done'}">复制译文<kbd>↵</kbd></button>`;
    foot = `<div class="c-foot"><span class="c-more">${extra}</span>${btn('close', '关闭<kbd>Esc</kbd>')}${copy}</div>`;
  }
  return (s.source ? `<div class="c-src">${esc(clipSource(s.source))}</div>` : '')
    + `<div class="c-title" title="${esc(title)}">${esc(title)}</div>${body}${err}${note}${foot}`;
}
