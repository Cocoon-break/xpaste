// 划词翻译卡片页面（card.html，设计 §3.2、§3.4）：状态 st、渲染、键盘与按钮。整张卡片由 view.ts 生成。
// 后端每次换卡片、重翻都换 seq：命令都带当前 seq，迟到的事件与调用按 seq 丢弃。
import './card.css';
import {
  cardAgain, cardBlur, cardClose, cardCopy, cardFocus, cardResize, onCardDelta, onCardDone, onCardShow, openPanelSettings,
  requestAccessibility, translateDownload, translateEngines,
} from '../api/card';
import { error as logError } from '../api/log';
import type { AppError, Engines } from '../api/types';
import { otherEngine, type Engine } from '../translate';
import { cardHTML, type CardState } from './view';

addEventListener('error', (e) => void logError(`划词卡片页错误：${e.message}`));
addEventListener('unhandledrejection', (e) => void logError(`划词卡片页未处理的拒绝：${String(e.reason)}`));

/** 无文字、排除应用的提示停留多久；复制后“已复制译文”停留多久（设计 §3.2） */
const HINT_MS = 2000, COPIED_MS = 900;

type Tr = Extract<CardState, { kind: 'busy' | 'done' | 'fail' }>;
const root = document.body;
let seq = 0;
let st: CardState = { kind: 'blocked' };
/** 两种翻译方式能不能用，每次出卡片取一次 */
let engines: Engines | null = null;
/** 缺语言包时那次识别出的语言对，下载用 */
let pack: { source: string; target: string } | null = null;
let timer = 0, lastH = 0;
/** 正在等系统的下载确认或下载：再点“下载语言包”不理，免得失败分支把键盘从确认框抢回来 */
let downloading = false;

const tr = (): Tr | null => (st.kind === 'empty' || st.kind === 'blocked' ? null : st);
const body = () => root.querySelector<HTMLElement>('.c-body');

function render(pop = false) {
  const s = tr();
  // 换用按钮只在收完或失败后、另一种能用时出现（设计 §3.4）
  const view: CardState = s ? { ...s, other: s.kind !== 'busy' && engines ? otherEngine(s.engine, engines) : null } : st;
  const focused = (document.activeElement as HTMLElement | null)?.dataset.act;
  const top = body()?.scrollTop ?? 0;
  root.innerHTML = cardHTML(view);
  if (pop) root.firstElementChild?.classList.add('pop-in');
  if (focused) root.querySelector<HTMLElement>(`[data-act="${focused}"]`)?.focus();
  const b = body();
  if (b) b.scrollTop = top;
  follow();
  fit();
}

/** 翻译中正文跟到最新一段 */
function follow() {
  const b = body();
  if (b && st.kind === 'busy') b.scrollTop = b.scrollHeight;
}

/** 按内容的完整高度改窗口高度（后端限到工作区的 60%，超出部分正文在卡片内滚动）。提示也调，免得窗口留着透明的空条 */
function fit() {
  const card = root.firstElementChild as HTMLElement | null, b = body();
  if (!card) return;
  const h = Math.ceil(card.offsetHeight + (b ? b.scrollHeight - b.clientHeight : 0));
  if (h !== lastH) {
    lastH = h;
    void cardResize(seq, h);
  }
}

void onCardShow((p) => {
  clearTimeout(timer);
  seq = p.seq;
  lastH = 0;
  pack = null;
  if (p.kind !== 'text') {
    st = p.kind === 'empty' ? { kind: 'empty', hotkey: p.hotkey } : { kind: 'blocked' };
    render(true);
    timer = window.setTimeout(() => void cardClose(p.seq), HINT_MS);
    return;
  }
  st = { kind: 'busy', source: p.source ?? '', engine: p.engine ?? '', text: '', noAccess: p.noAccess, other: null };
  render(true);
  void translateEngines().then((e) => { engines = e; render(); }, () => {});
});

// 增量不带 seq：只收翻译中的；重翻前已清空，旧请求的后端也不再发
void onCardDelta((d) => {
  const s = tr();
  if (s?.kind !== 'busy') return;
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
  if (p.ok) Object.assign(s, { kind: 'done', text: p.text ?? '', engine: p.engine });
  else {
    Object.assign(s, { kind: 'fail', engine: p.engine, error: p.error?.message ?? '翻译失败', errorKind: p.error?.kind });
    pack = p.error?.kind === 'translatePack' ? { source: p.error.source ?? '', target: p.error.target ?? '' } : null;
  }
  render();
});

/** 用另一种（或下载语言包后用系统翻译）重翻同一段原文。done 可能早于命令返回，所以先把 seq 换成新的 */
async function again(engine: Engine) {
  const s = tr();
  if (!s) return;
  const from = seq;
  seq = from + 1;
  pack = null;
  Object.assign(s, { kind: 'busy', text: '', error: undefined, errorKind: undefined, copied: false, engine: engine === 'system' ? 'system' : `openai:${engines?.model ?? ''}` });
  render();
  try {
    const r = await cardAgain(from, engine);
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
  s.copied = true;
  render();
  timer = window.setTimeout(() => void cardClose(at), COPIED_MS);
}

/** 下载语言包（设计 §3.1）：装好后用系统翻译重试；取消或失败时把键盘拿回来（下载期间失焦被忽略，不拿回来 Esc 和点别处都不再生效） */
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
  if (r === 'installed') void again('system');
  else void cardFocus(at);
}

root.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('button[data-act]');
  if (!b || b.getAttribute('aria-disabled') === 'true') return;
  const at = seq;
  switch (b.dataset.act) {
    case 'copy': return void copy();
    case 'close': return void cardClose(at);
    case 'switch': return void again(b.dataset.engine as Engine);
    case 'pack': return void downloadPack();
    // 先关卡片交还键盘，再唤起面板
    case 'settings': return void cardClose(at).finally(() => openPanelSettings('translate'));
    case 'access': return void requestAccessibility();
  }
});

// 回车复制、Esc 关闭；卡片里选中文字后的 Mod + C 照常。焦点在按钮上时回车交给按钮（同截图卡片）
addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); void cardClose(seq); return; }
  if (e.key === 'Enter' && !(e.target instanceof Element && e.target.closest('button'))) { e.preventDefault(); void copy(); }
});
// 点了卡片以外的地方：关闭（不交还键盘）
addEventListener('blur', () => void cardBlur(seq));
// 窗口高度被后端改了：翻译中仍跟到最新一段
addEventListener('resize', follow);
