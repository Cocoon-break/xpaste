// 划词翻译卡片页面（card.html，M26 规则 19、16、20）：状态 st、渲染、键盘与按钮。整张卡片由 view.ts 生成。
// 后端每次换卡片、重翻都换 seq：命令都带当前 seq，迟到的事件与调用按 seq 丢弃。
import './card.css';
import {
  cardBlur, cardClose, cardCopy, cardFocus, cardResize, cardTranslate, onCardDelta, onCardDone, onCardShow, openPanelSettings,
  requestAccessibility, translateDownload, translateEngines,
} from '../api/card';
import { error as logError } from '../api/log';
import type { AppError, Engines } from '../api/types';
import { otherEngine, staleDelta, type Engine } from '../translate';
import { composing } from '../ime';
import { cardHTML, type CardState } from './view';

addEventListener('error', (e) => void logError(`划词卡片页错误：${e.message}`));
addEventListener('unhandledrejection', (e) => void logError(`划词卡片页未处理的拒绝：${String(e.reason)}`));

/** 排除应用的提示停留多久；复制后“已复制译文”停留多久（M26 规则 19） */
const HINT_MS = 2000, COPIED_MS = 900;
/** 收起时的窗口宽度（后端 trcard.rs 的 CARD），比它宽就是展开对照 */
const NARROW = 360;
/** ⌘↵（Windows Ctrl + 回车）：翻译，或收完后复制译文（M26 规则 20） */
const MAC = navigator.userAgent.includes('Mac');
const MOD = MAC ? '⌘↵' : 'Ctrl ↵';

type Tr = Exclude<CardState, { kind: 'blocked' }>;
const root = document.body;
let seq = 0;
let st: CardState = { kind: 'blocked' };
/** 两种翻译方式能不能用，每次出卡片取一次 */
let engines: Engines | null = null;
/** 缺语言包时那次识别出的语言对，下载用 */
let pack: { source: string; target: string } | null = null;
let timer = 0, lastH = 0, lastWide = false;
/** 展开对照（M26 规则 19）：每张新卡片从收起开始 */
let wide = false;
/** 按住过标题行（拖动区）：之后改大小时后端不再按原来的位置摆，卡片留在拖到的地方 */
let moved = false;
/** 卡片刚上屏、用户还没按键或点鼠标：这期间的聚焦不是用户给的（见下方 focusin） */
let fresh = false;
/** 输入卡片的草稿（M26 规则 20）：原文框里的字随写随存，复制译文后清空；下次打开输入卡片填回。
 *  卡片窗口一直预建着，存在这里就跨卡片保留，退出 xPaste 后不留。input：这张卡片是输入卡片 */
let draft = '', input = false;
/** 正在等系统的下载确认或下载：再点“下载语言包”不理，免得失败分支把键盘从确认框抢回来 */
let downloading = false;
/** 这次请求的方式：翻译中换用后，被接替的请求迟到的增量据此丢掉（M26 规则 16） */
let want: Engine | undefined;
/** 这张卡片选的目标语言，空串为自动（M26 规则 20）；sent：上一次按什么原文和语言翻的，改过就是 dirty */
let pick = '';
let sent: { text: string; pick: string } | null = null;

const tr = (): Tr | null => (st.kind === 'blocked' ? null : st);
const dirty = (s: Tr) => !sent || s.source !== sent.text || pick !== sent.pick;
const kindOf = (tag: string): Engine | undefined => (tag === 'system' ? 'system' : tag.startsWith('openai:') ? 'openai' : undefined);
const src = () => root.querySelector<HTMLTextAreaElement>('textarea.c-orig');
/** 译文正文（展开对照时左栏原文也是 .c-body） */
const body = () => root.querySelector<HTMLElement>('.c-body:not(.c-orig)');
const bodies = () => [...root.querySelectorAll<HTMLElement>('.c-body')];

function render(pop = false) {
  const s = tr();
  // 另一种能用时就有换用按钮，翻译中也有（M26 规则 16）
  const view: CardState = s ? { ...s, other: engines ? otherEngine(s.engine, engines) : null, pick, dirty: dirty(s) } : st;
  // 新卡片（pop）不沿用上一张卡片的焦点：隐藏前点过的下拉框会被重新聚焦、画出焦点环
  const focused = pop ? undefined : (document.activeElement as HTMLElement | null)?.dataset.act;
  const tops = new Map(bodies().map((b) => [b.classList.contains('c-orig'), b.scrollTop]));
  const ta = src(), caret = ta && [ta.selectionStart, ta.selectionEnd];
  root.innerHTML = cardHTML(view, wide, MOD);
  if (pop) root.firstElementChild?.classList.add('pop-in');
  if (focused) root.querySelector<HTMLElement>(`[data-act="${focused}"]`)?.focus();
  if (caret) src()?.setSelectionRange(caret[0], caret[1]);
  for (const b of bodies()) b.scrollTop = tops.get(b.classList.contains('c-orig')) ?? 0;
  follow();
  fit();
}

/** 翻译中正文跟到最新一段 */
function follow() {
  const b = body();
  if (b && st.kind === 'busy') b.scrollTop = b.scrollHeight;
}

/** 按内容的完整高度改窗口高度（后端限到工作区的 60%，展开对照时 80%，超出部分正文在卡片内滚动）。
 *  提示也调，免得窗口留着透明的空条。展开或收起时先只换宽度，等窗口换过来（resize）再量高度：在旧宽度里量两栏会量得过高 */
function fit() {
  const card = root.firstElementChild as HTMLElement | null;
  if (!card) return;
  if (wide !== innerWidth > NARROW) {
    if (lastWide !== wide) {
      lastWide = wide;
      void cardResize(seq, lastH || card.offsetHeight, wide, moved);
    }
    return;
  }
  // 两栏整体被压低的部分（栏内框有最小高度）加上框内最多溢出的部分；只量框内会量少，窗口不长高，内容从底部透出来
  const cols = root.querySelector<HTMLElement>('.c-cols');
  const over = (cols ? cols.scrollHeight - cols.clientHeight : 0) + Math.max(0, ...bodies().map((b) => b.scrollHeight - b.clientHeight));
  const h = Math.ceil(card.offsetHeight + over);
  if (h !== lastH || wide !== lastWide) {
    lastH = h;
    lastWide = wide;
    void cardResize(seq, h, wide, moved);
  }
}

void onCardShow((p) => {
  clearTimeout(timer);
  seq = p.seq;
  lastH = 0;
  lastWide = wide = moved = false;
  fresh = true;
  pack = null;
  want = undefined;
  pick = '';
  sent = null;
  if (p.kind === 'blocked') {
    st = { kind: 'blocked' };
    render(true);
    timer = window.setTimeout(() => void cardClose(p.seq), HINT_MS);
    return;
  }
  input = p.kind === 'input';
  if (input) {
    // 输入卡片（M26 规则 20）：展开成两栏，填回上次没用完的草稿，光标在原文框末尾
    st = { kind: 'idle', source: draft, engine: '', text: '', noAccess: false, other: null };
    wide = true;
    render(true);
    src()?.focus();
    src()?.setSelectionRange(draft.length, draft.length);
  } else {
    st = { kind: 'busy', source: p.source ?? '', engine: p.engine ?? '', text: '', noAccess: p.noAccess, other: null };
    sent = { text: st.source, pick };
    render(true);
  }
  void translateEngines().then((e) => { engines = e; render(); }, () => {});
});

// 增量不带 seq：只收翻译中的；重翻前已清空，旧请求的后端也不再发，路上迟到的按方式丢掉
void onCardDelta((d, engine) => {
  const s = tr();
  if (s?.kind !== 'busy' || staleDelta(engine, want)) return;
  const placeholder = !s.text;
  s.text += d;
  if (placeholder) return render();
  body()?.append(d);
  follow();
  fit();
});

void onCardDone((p) => {
  const s = tr();
  if (!s || p.seq !== seq) return;
  // 成功以完整译文为准；失败保留已收到的部分（标题为“翻译中断”）
  if (p.ok) Object.assign(s, { kind: 'done', text: p.text ?? '', engine: p.engine, lang: p.lang ?? undefined });
  else {
    Object.assign(s, { kind: 'fail', engine: p.engine, error: p.error?.message ?? '翻译失败', errorKind: p.error?.kind });
    pack = p.error?.kind === 'translatePack' ? { source: p.error.source ?? '', target: p.error.target ?? '' } : null;
  }
  render();
});

/** 按卡片上的原文与目标语言翻译（M26 规则 16、20）：输入卡片 ⌘↵、改过原文或语言后 ⌘↵、换用、下载语言包后重试。
 *  翻译中也行（后端中止进行中的请求）。engine 没给时沿用卡片当前的方式，还没有就按设置。
 *  done 可能早于命令返回，所以先把 seq 换成新的 */
async function go(engine?: Engine) {
  const s = tr();
  if (!s || !s.source.trim()) return;
  const from = seq;
  const e = engine ?? kindOf(s.engine);
  seq = from + 1;
  pack = null;
  want = e;
  sent = { text: s.source, pick };
  Object.assign(s, {
    kind: 'busy', text: '', error: undefined, errorKind: undefined, copied: false, lang: undefined,
    engine: e === 'system' ? 'system' : e === 'openai' ? `openai:${engines?.model ?? ''}` : '',
  });
  render();
  try {
    const r = await cardTranslate(from, s.source, pick || null, e);
    if (r.seq === seq && st === s && s.kind === 'busy') { s.engine = r.engine; render(); }
  } catch {
    // 卡片已关闭或已被取代：窗口已撤下
  }
}

/** 复制译文（回车或按钮）：进历史，卡片显示“已复制译文”后关闭 */
async function copy() {
  const s = tr();
  if (s?.kind !== 'done' || s.copied) return;
  const at = seq;
  try {
    await cardCopy(s.text);
  } catch (e) {
    if (at === seq) { s.error = (e as AppError)?.message || '复制失败'; render(); }
    return;
  }
  if (at !== seq) return;
  if (input) draft = '';
  s.copied = true;
  render();
  timer = window.setTimeout(() => void cardClose(at), COPIED_MS);
}

/** 下载语言包（M26 规则 17）：装好后用系统翻译重试；取消或失败时把键盘拿回来（下载期间失焦被忽略，不拿回来 Esc 和点别处都不再生效） */
async function downloadPack() {
  const s = tr();
  if (!s || !pack || downloading) return;
  const at = seq;
  let r: 'installed' | 'download' = 'download';
  downloading = true;
  try {
    r = await translateDownload('tr-card', pack.source, pack.target);
  } catch (e) {
    const err = e as AppError;
    // 已有一个语言包在下载（面板或设置里点的）：后端拒绝第二个，不报
    if (at === seq && !(err.kind === 'invalid' && err.message === '正在下载语言包')) { s.error = err.message; render(); }
  } finally {
    downloading = false;
  }
  if (at !== seq) return;
  if (r === 'installed') void go('system');
  else void cardFocus(at);
}

root.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('button[data-act]');
  if (!b || b.getAttribute('aria-disabled') === 'true') return;
  const at = seq;
  switch (b.dataset.act) {
    case 'copy': return void copy();
    case 'go': return void go();
    case 'close': return void cardClose(at);
    case 'wide': wide = !wide; return render();
    case 'switch': return void go(b.dataset.engine as Engine);
    case 'pack': return void downloadPack();
    // 先关卡片交还键盘，再唤起面板
    case 'settings': return void cardClose(at).finally(() => openPanelSettings('translate'));
    case 'access': return void requestAccessibility();
  }
});

// 原文框里改字、换目标语言（M26 规则 20）：主按钮在“翻译”和“复制译文”之间换时才重画，否则只调高度
root.addEventListener('input', (e) => {
  const s = tr(), t = e.target as Element;
  if (!s || !t.matches('textarea.c-orig')) return;
  const was = dirty(s) || !s.source.trim();
  s.source = (t as HTMLTextAreaElement).value;
  if (input) draft = s.source;
  if (was !== (dirty(s) || !s.source.trim())) render(); else fit();
});
root.addEventListener('change', (e) => {
  const t = e.target as HTMLSelectElement;
  if (!t.matches('select[data-act="lang"]')) return;
  pick = t.value;
  render();
});

// ⌘↵（Windows Ctrl + 回车）：没翻译过或改过原文、语言时翻译，否则复制译文；回车复制（在原文框里是换行）；Esc 关闭。
// 卡片里选中文字后的 Mod + C 照常。焦点在按钮上时回车交给按钮（同截图卡片）
addEventListener('keydown', (e) => {
  // 输入法选字时 Esc 只收起候选、回车只上屏，都不归卡片
  if (composing(e)) return;
  if (e.key === 'Escape') { e.preventDefault(); void cardClose(seq); return; }
  if (e.key !== 'Enter') return;
  const s = tr();
  if (MAC ? e.metaKey : e.ctrlKey) {
    e.preventDefault();
    if (s && (s.kind === 'idle' || dirty(s))) void go(); else void copy();
    return;
  }
  if (e.target instanceof Element && e.target.closest('button, textarea, select')) return;
  e.preventDefault();
  void copy();
});
// 按住拖动区（标题行、原文提示；按钮与下拉框不算）：记下拖动过，系统随后接手拖动
root.addEventListener('mousedown', (e) => {
  const t = e.target as Element;
  if (t.closest('[data-tauri-drag-region]') && !t.closest('button, select')) moved = true;
});
// 窗口拿到键盘时 WebKit 会把焦点交给第一个可聚焦的控件（“对照原文”、下拉框），当作键盘聚焦画出焦点环，回车也会按到它。
// 卡片刚上屏、用户还没按键或点鼠标时撤掉这种聚焦；输入卡片改把光标放回原文框
addEventListener('focusin', (e) => {
  const t = e.target as HTMLElement;
  if (!fresh || t.matches('textarea.c-orig')) return;
  t.blur();
  if (st.kind === 'idle') src()?.focus();
}, true);
addEventListener('pointerdown', () => (fresh = false), true);
addEventListener('keydown', () => (fresh = false), true);
addEventListener('focus', () => {
  if (st.kind === 'idle' && document.activeElement === document.body) src()?.focus();
});
// 点了卡片以外的地方：关闭（不交还键盘），输入卡片也一样（失焦后收不到 Esc，留在屏幕上关不掉）
addEventListener('blur', () => void cardBlur(seq));
// 窗口被后端改了大小：翻译中仍跟到最新一段；展开或收起后按新宽度重新量高度
addEventListener('resize', () => { follow(); fit(); });
