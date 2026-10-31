// 划词翻译卡片（M26 规则 19、16、20）的整张 HTML，截图译文卡片也用 cardInner。纯函数，状态由各自的 main.ts 保存。
import { esc } from '../format.ts';
import { engineLabel, LANG_LABEL, type Engine } from '../translate.ts';

/** blocked：2 秒后消失的提示；其余为翻译卡片。idle：输入卡片还没翻译过（规则 20）。engine：翻译方式标签；
 *  other：能换用的另一种（otherEngine）；errorKind：失败时的 AppError.kind；copied：复制成功，900 ms 后关闭。
 *  只有划词卡片用：lang 实际翻成的语言代码；pick 这张卡片选的目标语言（空串为自动）；dirty 原文或目标语言改过、还没按它翻译 */
export type CardState =
  | { kind: 'blocked' }
  | {
    kind: 'idle' | 'busy' | 'done' | 'fail';
    source: string;
    engine: string;
    text: string;
    noAccess: boolean;
    other: Engine | null;
    error?: string;
    errorKind?: string;
    copied?: boolean;
    lang?: string;
    pick?: string;
    dirty?: boolean;
  };

const SOURCE_CHARS = 40;
const SWITCH = { system: '换用系统翻译', openai: '换用 AI 翻译' };
const btn = (act: string, label: string, extra = '') => `<button type="button" data-act="${act}"${extra}>${label}</button>`;

/** 原文提示：空白折成一个空格，取前 40 个字，超出加“…” */
export function clipSource(s: string): string {
  const cs = [...s.trim().split(/\s+/).join(' ')];
  return cs.length > SOURCE_CHARS ? cs.slice(0, SOURCE_CHARS).join('') + '…' : cs.join('');
}

/** `wide`：展开对照，左栏是可编辑的原文、右栏是译文（M26 规则 19、20）。`mod`：⌘↵ 的写法，Windows 为“Ctrl ↵” */
export function cardHTML(s: CardState, wide = false, mod = '⌘↵'): string {
  if (s.kind === 'blocked') return '<div class="card hint" role="status"><p class="c-msg">这个应用里的内容不翻译</p></div>';
  return `<div class="card${wide ? ' wide' : ''}" role="dialog" aria-label="译文">${cardInner(s, wide, mod)}</div>`;
}

/** 目标语言下拉框：自动（按设置的目标语言，规则 9）＋五种 */
function langSelect(pick: string): string {
  const opts = [['', '自动'], ...Object.entries(LANG_LABEL)]
    .map(([v, l]) => `<option value="${v}"${v === pick ? ' selected' : ''}>${l}</option>`).join('');
  return `<label class="c-to">翻成<select data-act="lang" aria-label="目标语言">${opts}</select></label>`;
}

/** 翻译卡片的内容（不含外层 .card）；截图译文卡片也用它，原文为空时不出原文提示（M26 规则 5、16）。
 *  `wide` 只有划词卡片传：标题行右侧出“对照原文 / 收起”；展开时标题行多目标语言下拉框，左栏是可编辑的原文，
 *  原文或语言改过时主按钮是“翻译 ⌘↵”（规则 20） */
export function cardInner(s: Exclude<CardState, { kind: 'blocked' }>, wide?: boolean, mod = '⌘↵'): string {
  // 截图卡片在遮罩里，不拖
  const drag = wide === undefined ? '' : ' data-tauri-drag-region';
  const way = engineLabel(s.engine);
  const head = s.kind === 'idle' ? '翻译' : s.kind === 'busy' ? '翻译中…' : s.kind === 'done' ? '译文' : s.text ? '翻译中断' : '翻译失败';
  const lang = wide && s.kind === 'done' && s.lang ? LANG_LABEL[s.lang] ?? s.lang : '';
  const title = [head, lang, way].filter(Boolean).join(' · ');
  let body = s.text ? `<div class="c-body">${esc(s.text)}</div>` : s.kind === 'busy' ? '<div class="c-body c-wait">翻译中…</div>' : '';
  if (wide) {
    const right = body || `<div class="c-body c-wait">${s.kind === 'idle' ? '译文显示在这里' : ''}</div>`;
    body = `<div class="c-cols"><textarea class="c-body c-orig" data-act="src" aria-label="原文" placeholder="输入要翻译的文字" spellcheck="false">${esc(s.source)}</textarea>${right}</div>`;
  }
  const err = s.kind !== 'busy' && s.error ? `<div class="c-err">${esc(s.error)}</div>` : '';
  const note = s.noAccess ? `<div class="c-note"><span>没有辅助功能权限，翻译的是剪切板里的文字</span>${btn('access', '前往授权')}</div>` : '';
  let foot: string;
  if (s.copied) foot = '<div class="c-foot"><span class="c-ok" role="status">已复制译文</span></div>';
  else {
    // 下载语言包、去设置只在失败后出现；换用翻译中也有（M26 规则 16）
    const extra = (s.kind === 'fail' && s.errorKind === 'translatePack' ? btn('pack', '下载语言包') : '')
      + (s.kind === 'fail' && (s.errorKind === 'translateConfig' || s.errorKind === 'translateOs') ? btn('settings', '去设置') : '')
      + (s.other && s.kind !== 'idle' ? btn('switch', SWITCH[s.other], ` data-engine="${s.other}"`) : '');
    // 主按钮：还没翻译或原文、语言改过时“翻译”（原文为空时不可用）；否则复制译文，收完才可用，失败时不放（M26 规则 19、20）
    const key = wide ? mod : '↵';
    const go = s.kind === 'idle' || s.dirty
      ? `<button type="button" class="go" data-act="go" aria-disabled="${!s.source.trim()}">翻译<kbd>${mod}</kbd></button>`
      : s.kind === 'fail' ? ''
        : `<button type="button" class="go" data-act="copy" aria-disabled="${s.kind !== 'done'}">复制译文<kbd>${key}</kbd></button>`;
    foot = `<div class="c-foot"><span class="c-more">${extra}</span>${btn('close', '关闭<kbd>Esc</kbd>')}${go}</div>`;
  }
  // 标题行与原文提示是拖动区：按住拖动卡片（规则 19；按钮、下拉框不算）
  const line = `<div class="c-title" title="${esc(title)}"${drag}>${esc(title)}</div>`;
  const toggle = s.kind === 'idle' ? '' : `<button type="button" data-act="wide" aria-pressed="${wide}">${wide ? '收起' : '对照原文'}</button>`;
  const top = wide === undefined || s.copied ? line : `<div class="c-head"${drag}>${line}${wide ? langSelect(s.pick ?? '') : ''}${toggle}</div>`;
  return (s.source && !wide ? `<div class="c-src"${drag}>${esc(clipSource(s.source))}</div>` : '') + top + body + err + note + foot;
}
