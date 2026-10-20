import './styles.css';
import { attachConsole, info } from './api/log';
import {
  accessibilityGranted, bootstrap, clearHistory, copy, countUnpinned, deleteRecord, finishOnboarding, getRecord, hidePanel, installedApps, openUrl, prunePreview,
  requestAccessibility, revealFile, saveSettings, search, setDialogOpen, setPaused, setPinned, undoDelete,
} from './api/commands';
import {
  onConfirmClear,
  onHotkeyPressed,
  onOpenSettings,
  onPanelHidden,
  onPanelHideRequested,
  onPanelShown,
  type PanelShown,
  onPausedChanged,
  onRecordAdded,
  onRecordsRemoved,
  onRecordUpdated,
} from './api/events';
import type { AppError, Appearance, FileEntry, PlatformInfo, RecordDetail, RecordSummary, Settings } from './api/types';
import { copyToast, needsAccessibility } from './copy-status';
import {
  canInlinePlay, esc, FILTERS, FL, fmtBytes, fmtDateTime, fmtDur, fmtInt, hl, hlCode, host, isFav, isHttpUrl, LABEL, rel, truncate, videoChips,
  type Filter,
} from './format';
import { hotkeyFromEvent, hotkeyLabel } from './hotkey';
import { keyAction } from './keymap';
import { copyVerb, escHint, openLabel } from './labels';
import { nextStep, onboardSteps, type Step } from './onboarding';
import { applyAdded, applyRemoved, applyUpdated } from './records';
import { layoutBars, sameColumns, spectrumStat, staggerIndex, xPct, type Layout } from './spectrum';
import { addIgnored, formToSettings, optionFromRetention, parseFrequent, parseLimit, pickerOptions, removeIgnored } from './settings-form';
import {
  armUndo, canUndo, counts, cycleFilter, ensureSelection, groups, inFilter, isStale, nextSelection, UNDO_MS, visible, type UndoState,
} from './view';
import { layout, range, ROW_H, scrollTo, topOf, type Cell } from './virtual-list';

const I = {
  text: '<path d="M4 5.5h12M4 10h12M4 14.5h7"/>',
  link: '<path d="M8.5 11.5a3 3 0 0 0 4.2 0l2.6-2.6a3 3 0 0 0-4.2-4.2l-.9.9"/><path d="M11.5 8.5a3 3 0 0 0-4.2 0l-2.6 2.6a3 3 0 0 0 4.2 4.2l.9-.9"/>',
  code: '<path d="M7 6 3 10l4 4M13 6l4 4-4 4"/>',
  image: '<rect x="3" y="4" width="14" height="12" rx="2"/><circle cx="7.5" cy="8.5" r="1.5"/><path d="m4 15 4.5-4.5L12 14l2-2 3 3"/>',
  video: '<rect x="2.5" y="5" width="11" height="10" rx="2"/><path d="m13.5 9 4-2.5v7l-4-2.5"/>',
  file: '<path d="M5 3h6.5L15 6.5V17H5z"/><path d="M11 3v4h4"/>',
  star: '<path d="m10 3 2.1 4.4 4.8.6-3.5 3.3.9 4.7L10 13.8 5.7 16l.9-4.7L3.1 8l4.8-.6z"/>',
  play: '<path d="M7 5v10l8-5z" fill="currentColor"/>',
  search: '<circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3"/>',
  sliders: '<path d="M4 6h8M15.5 6h.5M4 14h.5M8 14h8"/><circle cx="13.5" cy="6" r="1.8"/><circle cx="6.5" cy="14" r="1.8"/>',
  pin: '<path d="M7.5 3h5l-.8 5 2.8 3H5.5l2.8-3z"/><path d="M10 11v6"/>',
  pinFill: '<path d="M7.5 3h5l-.8 5 2.8 3H5.5l2.8-3z" fill="currentColor"/><path d="M10 11v6"/>',
  copy: '<rect x="7" y="7" width="9.5" height="9.5" rx="2"/><path d="M13 7V5a1.5 1.5 0 0 0-1.5-1.5h-6A1.5 1.5 0 0 0 4 5v6.5A1.5 1.5 0 0 0 5.5 13H7"/>',
  check: '<path d="m4.5 10.5 3.5 3.5 7.5-8"/>',
  trash: '<path d="M4 6h12M8 6V4h4v2M6 6l.7 10h6.6L14 6"/>',
  x: '<path d="m5.5 5.5 9 9M14.5 5.5l-9 9"/>',
  open: '<path d="M11 4h5v5M16 4l-7 7M14 11.5V16H4V6h4.5"/>',
  plus: '<path d="M10 4v12M4 10h12"/>',
  spark: '<path d="M10 3v3M10 14v3M3 10h3M14 10h3M5.5 5.5l2 2M12.5 12.5l2 2M14.5 5.5l-2 2M7.5 12.5l-2 2"/>',
  inbox: '<path d="M3 11h4l1 2h4l1-2h4"/><path d="M5 5h10l2 6v4H3v-4z"/>',
  sun: '<circle cx="10" cy="10" r="3.2"/><path d="M10 2.5v1.8M10 15.7v1.8M2.5 10h1.8M15.7 10h1.8M4.7 4.7 6 6M14 14l1.3 1.3M15.3 4.7 14 6M6 14l-1.3 1.3"/>',
  moon: '<path d="M15.5 12.2A6 6 0 0 1 7.8 4.5a6 6 0 1 0 7.7 7.7z"/>',
  chevron: '<path d="m6 12 4-4 4 4"/>',
  monitor: '<rect x="3" y="4" width="14" height="9.5" rx="1.5"/><path d="M7.5 16.5h5M10 13.5v3"/>',
};
const svg = (n: keyof typeof I, cls = '') =>
  `<svg class="ic ${cls}" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${I[n]}</svg>`;
document.querySelectorAll<HTMLElement>('[data-icon]').forEach((el) => {
  el.innerHTML = svg(el.dataset.icon as keyof typeof I);
  el.style.display = 'inline-grid';
});

const $ = <T extends HTMLElement = HTMLElement>(s: string) => document.querySelector<T>(s)!;
const root = document.documentElement;
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const EASE_ENTER = 'cubic-bezier(.22,1,.36,1)', EASE_MOVE = 'cubic-bezier(.25,1,.5,1)', EASE_SPRING = 'cubic-bezier(.34,1.56,.64,1)';
const fx = (el: Element | null, kf: Keyframe[], opts: KeyframeAnimationOptions) => {
  if (el && !reduced.matches) el.animate(kf, opts);
};
// 先关过渡再改状态（键盘触发、重建后定位）
const instantly = (el: HTMLElement, on: boolean, fn: () => void) => {
  if (on) el.setAttribute('data-instant', '');
  fn();
  if (on) {
    void el.offsetWidth;
    el.removeAttribute('data-instant');
  }
};
const hf = new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false });

// —— 元素 ——
const win = $('#win'), list = $('#list'), rowsEl = $('#rows'), ind = $('#ind'), q = $<HTMLInputElement>('#q'), clearQ = $('#clearQ');
const tabs = $('#tabs'), tabsBase = $('#tabsBase'), tabsOver = $('#tabsOver');
const ptype = $('#ptype'), pbody = $('#pbody'), pmeta = $('#pmeta'), countEl = $('#count');
const pinBtn = $<HTMLButtonElement>('#pinBtn'), copyBtn = $<HTMLButtonElement>('#copyBtn');
const delBtn = $<HTMLButtonElement>('#delBtn');
const menu = $('#menu'), mOpen = $<HTMLButtonElement>('[data-m="open"]'), toastEl = $('#toast'), toastMsg = $('#toastMsg'), undoBtn = $<HTMLButtonElement>('#undoBtn'), toastAct = $<HTMLButtonElement>('#toastAct'), pauseBtn = $('#pauseBtn');
const spBars = $('#spBars'), spTrack = $('#spTrack'), spCursor = $('#spCursor'), spTip = $('#spTip');

// —— 状态 ——
const S = {
  records: new Map<number, RecordSummary>(),
  filter: 'all' as Filter,
  q: '',
  matchIds: null as Set<number> | null,
  sel: null as number | null,
  detail: null as RecordDetail | null,
  undo: null as UndoState,
  removing: new Set<number>(),
  menuFor: null as number | null,
  newId: null as number | null,
  shown: false,
  settings: {} as Settings,
  platform: {} as PlatformInfo,
  paused: false,
  /** macOS 已授权辅助功能 / Windows：Enter 直接粘贴 */
  canPaste: false,
  cells: [] as Cell[],
  total: 0,
  /** 当前可见条数，供 aria-setsize */
  count: 0,
};
const menuOpen = () => menu.dataset.open === 'true';

// —— 随状态变化的文案 ——
const undoHint = $('#undoHint'), escHintEl = $('#escHint');
const paintUndoHint = () => (undoHint.hidden = !canUndo(S.undo, Date.now()));
// macOS 看辅助功能权限（bootstrap 后取一次，每次唤起刷新），Windows 恒能粘贴
async function paintVerb() {
  S.canPaste = S.platform.os !== 'macos' || (await accessibilityGranted().catch(() => false));
  const v = copyVerb(S.canPaste);
  document.querySelectorAll<HTMLElement>('[data-verb]').forEach((e) => (e.textContent = (e.dataset.verb ?? '') + v));
  rowsEl.querySelectorAll<HTMLElement>('[data-qa="copy"]').forEach((b) => (b.title = v));
}

// —— 数据视图 ——
const favMin = () => S.settings.frequentMinUses;
const vis = () => visible(S.records, S.filter, S.matchIds, favMin());
const matches = (r: RecordSummary) => inFilter(r, S.filter, favMin()) && (S.matchIds === null || S.matchIds.has(r.id));
const rowEl = (id: number | null) => (id === null ? null : document.getElementById('opt-' + id));
const source = (r: RecordSummary) => r.app?.name || '未知来源';

const thumb = (r: RecordSummary) =>
  r.kind === 'image' && r.thumb ? `<span class="thumb"><img src="${esc(r.thumb)}" alt="" width="36" height="36"></span>`
  : r.kind === 'video' && r.thumb ? `<span class="thumb"><img src="${esc(r.thumb)}" alt="" width="36" height="36"><span class="play">${svg('play')}</span></span>`
  : `<span class="thumb">${svg(r.kind)}</span>`;

const metaLine = (r: RecordSummary) => {
  const parts = [esc(source(r)), rel(r.copiedAt), r.kind === 'video' && r.durationSec !== null ? fmtDur(r.durationSec) : fmtBytes(r.size)];
  if (isFav(r, favMin())) parts.push(`<span class="uses">用过 ${fmtInt(r.uses)} 次</span>`);
  return parts.join(' · ');
};

const rowHTML = (r: RecordSummary, i: number, stagger: boolean, isNew = r.id === S.newId) => `
    <div class="item${isNew ? ' is-new' : ''}${S.removing.has(r.id) ? ' is-removing' : ''}${stagger && i < 12 ? ' stg' : ''}" data-id="${r.id}" style="--i:${i}">
      <div class="row" role="option" id="opt-${r.id}" aria-selected="false" aria-posinset="${i + 1}" aria-label="${esc(r.title)}，${LABEL[r.kind]}" data-type="${r.kind}">
        <div class="row-in">
          ${thumb(r)}
          <div class="main">
            <div class="title${r.kind === 'code' ? ' mono' : ''}">${hl(r.title, S.q)}</div>
            <div class="meta">${metaLine(r)}</div>
          </div>
          <div class="tail">
            ${r.pinned ? `<span class="pin-mark">${svg('pinFill')}</span>` : ''}${i < 9 ? `<kbd class="idx">${i + 1}</kbd>` : ''}
            <span class="qa" aria-hidden="true">
              <button type="button" tabindex="-1" data-qa="pin" title="${r.pinned ? '取消置顶' : '置顶'}">${svg(r.pinned ? 'pinFill' : 'pin')}</button>
              <button type="button" tabindex="-1" data-qa="copy" title="${copyVerb(S.canPaste)}">${svg('copy')}</button>
            </span>
          </div>
        </div>
      </div>
    </div>`;

function setCount(n: number, total: number) {
  const html = `${fmtInt(n)}<small>/ ${fmtInt(total)}</small>`;
  if (countEl.innerHTML === html) return;
  countEl.innerHTML = html;
  fx(countEl, [{ transform: 'translateY(6px)', opacity: 0, filter: 'blur(3px)' }, { transform: 'none', opacity: 1, filter: 'blur(0)' }], { duration: 260, easing: EASE_ENTER });
}

let bootError = '';
function emptyHTML() {
  if (bootError) return `<div class="empty"><span class="ring">${svg('inbox')}</span><p>无法读取历史记录：${esc(bootError)}</p></div>`;
  const qq = S.q.trim(), f = S.filter;
  const msg = qq ? `没有匹配“${esc(qq)}”的${f === 'all' ? '' : FL[f]}记录`
    : f === 'fav' ? `还没有常用记录。同一条内容复制 ${favMin()} 次以上就会出现在这里`
    : `还没有${f === 'all' ? '' : FL[f]}记录，复制一次就会出现在这里`;
  return `<div class="empty"><span class="ring">${svg(qq ? 'search' : f === 'fav' ? 'star' : 'inbox')}</span><p>${msg}</p>
      ${qq || f !== 'all' ? '<button class="btn sm" type="button" data-act="reset">清除搜索与筛选</button>' : ''}</div>`;
}

// —— 虚拟列表 ——
// 只渲染 range() 内的 cell；按 key 复用元素，HTML 不变时只改位置，悬停与动画类不闪。
// 一次性的入场类（stg、is-new）不计入比较，之后的重绘不会打断动画
type Painted = { el: HTMLElement; h: string; top: number };
let painted = new Map<string, Painted>();
// cell 的 top 相对 .list-inner，滚动容器还有上内边距
const padTop = () => parseFloat(getComputedStyle(list).paddingTop) || 0;
// 正在删除的行收起后，它下方的 cell 显示位置上移一行（设计 9.4）。原型靠 grid 收起把邻居推上去，
// 这里 .vrow 绝对定位，位置显式算：S.cells 不变，删除计时器到点重排时 layout 给出的 top 与这里一致。
// 滚到底时重排会把 scrollTop 夹紧到新的最大值（原型是收起过程中逐渐夹紧）：超出的 over 让所有 cell
// 先下移 over，重排后浏览器夹紧 scrollTop 正好落到同一画面
type Gone = { tops: number[]; over: number };
const goneOf = (): Gone => {
  const tops = [...S.removing].flatMap((id) => topOf(S.cells, id) ?? []);
  // 删除后的最大滚动量不小于 0：不满一屏（或溢出不足）时不夹紧
  const over = tops.length ? Math.max(0, list.scrollTop - Math.max(0, list.scrollHeight - tops.length * ROW_H - list.clientHeight)) : 0;
  return { tops, over };
};
const shiftUp = (top: number, g: Gone) => top + g.over - g.tops.filter((t) => t < top).length * ROW_H;
const shownTop = (id: number) => {
  const t = topOf(S.cells, id);
  return t === undefined ? t : shiftUp(t, goneOf());
};
// 复制光扫：记住开始时间，行被事件重绘整行替换时在新元素上接着扫（负的 animation-delay）
const BEAM_MS = 760;
let beam = { id: -1, t0: 0 };
function paintBeam(row: HTMLElement | null) {
  const left = beam.t0 + BEAM_MS - performance.now();
  if (!row || left <= 0 || reduced.matches) return;
  row.style.setProperty('--beam-at', `${Math.round(left - BEAM_MS)}ms`);
  row.classList.add('beam');
}
/** `glide`：已渲染的元素位置变了就用这条 transition 过去（插入 / 删除时下方的行补位）；transitionend 后清掉。
 *  `glideBusy`：元素正在补位途中（连续删除）时改用它，不再空停一段延迟 */
function paintRows({ stagger = false, glide = '', glideBusy = '' } = {}) {
  const [a, b] = range(S.cells, list.scrollTop - padTop(), list.clientHeight);
  const next = new Map<string, Painted>();
  const gone = goneOf();
  if (reduced.matches) glide = '';
  for (let i = a; i < b; i++) {
    const c = S.cells[i];
    const r = c.kind === 'row' ? S.records.get(c.id) : undefined;
    if (c.kind === 'row' && !r) continue;
    const key = c.kind === 'head' ? 'h' + c.name : 'r' + c.id;
    const h = r && c.kind === 'row' ? rowHTML(r, c.index, false, false)
      : c.kind === 'head' ? `<div class="ghead" role="presentation" id="gh${c.gi}">${esc(c.name)}<span>${c.count}</span></div>` : '';
    let p = painted.get(key);
    if (!p) {
      const el = document.createElement('div');
      el.className = 'vrow';
      p = { el, h: '', top: NaN };
    }
    const fresh = !!r && (stagger || r.id === S.newId);
    if (r && c.kind === 'row' && fresh) p.el.innerHTML = rowHTML(r, c.index, stagger);
    else if (p.h !== h) p.el.innerHTML = h;
    if (r && (fresh || p.h !== h)) {
      const row = p.el.querySelector<HTMLElement>('.row');
      // 新记录弹入：旧 WebKit 没有 @starting-style，用 WAAPI 兜底
      if (r.id === S.newId) {
        fx(row, [{ opacity: 0 }, { opacity: 1 }], { duration: 260, easing: EASE_ENTER, delay: 60, fill: 'backwards' });
        fx(row, [{ transform: 'translateY(-12px) scale(.97)' }, { transform: 'none' }], { duration: 520, easing: EASE_SPRING, delay: 60, fill: 'backwards' });
      }
      if (r.id === beam.id) paintBeam(row);
    }
    p.h = h;
    // 条数一变全部行都会变，放在 HTML 里会让每次增删整表替换
    if (r) p.el.querySelector('.row')?.setAttribute('aria-setsize', String(S.count));
    const y = shiftUp(c.top, gone);
    // 位置不变时不动 transition：不会有 transitionend 来清它；位置变了但这次不补位，清掉残留的过渡
    if (p.top !== y) p.el.style.transition = glide && p.el.isConnected ? (p.el.style.transition && glideBusy) || glide : '';
    p.top = y;
    p.el.style.transform = `translateY(${y}px)`;
    // ponytail: 新元素追加在末尾，DOM 顺序不严格等于视觉顺序；行不可聚焦，读屏走 aria-activedescendant
    if (!p.el.isConnected) rowsEl.append(p.el);
    next.set(key, p);
  }
  for (const [k, p] of painted) if (!next.has(k)) p.el.remove();
  painted = next;
  markSel();
}
rowsEl.addEventListener('transitionend', (e) => {
  const t = e.target as HTMLElement;
  if (t.classList.contains('vrow')) t.style.transition = '';
});

// aria-activedescendant 只指向已渲染的行
function markSel() {
  rowsEl.querySelectorAll('.row[aria-selected="true"]').forEach((r) => r.setAttribute('aria-selected', 'false'));
  const r = rowEl(S.sel);
  r?.setAttribute('aria-selected', 'true');
  q.setAttribute('aria-activedescendant', r ? r.id : '');
}

let paintRaf = 0;
const schedulePaint = () => {
  if (!paintRaf) paintRaf = requestAnimationFrame(() => {
    paintRaf = 0;
    paintRows();
  });
};

function renderList({ scroll = false, stagger = false, base = 0, glide = '' } = {}) {
  const v = vis();
  S.count = v.length;
  setCount(v.length, S.records.size);
  rowsEl.style.setProperty('--base', base + 'ms');
  if (!v.length) {
    S.cells = [];
    S.total = 0;
    painted.clear();
    rowsEl.style.height = '';
    rowsEl.innerHTML = emptyHTML();
    select(null);
    return;
  }
  ({ cells: S.cells, total: S.total } = layout(groups(v, S.filter)));
  rowsEl.querySelector('.empty')?.remove();
  rowsEl.style.height = S.total + 'px';
  paintRows({ stagger, glide });
  S.sel = ensureSelection(v, S.sel);
  // 补位时选中光标跟着选中行一起滑
  select(S.sel, { scroll, glide: !!glide, fade: false, indDur: 300, indEase: EASE_ENTER });
}
const ENTER_GLIDE = `transform 300ms ${EASE_ENTER}`;

// 选中光标：独立一层滑动，内容即时
let indShown = false;
function moveInd({ instant = false, dur = 130, ease = EASE_MOVE }: { instant?: boolean; dur?: number; ease?: string } = {}) {
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  const t = r ? shownTop(r.id) : undefined;
  if (!r || t === undefined) {
    ind.style.opacity = '0';
    indShown = false;
    return;
  }
  ind.dataset.type = r.kind;
  const d = instant || !indShown || reduced.matches ? 0 : dur;
  ind.style.transition = `transform ${d}ms ${ease}, opacity 160ms ease`;
  ind.style.transform = `translateY(${t}px)`;
  ind.style.opacity = '1';
  indShown = true;
}

type SelectOpts = { scroll?: boolean; glide?: boolean; instant?: boolean; fade?: boolean; indDur?: number; indEase?: string };
function select(id: number | null, { scroll = false, glide = true, instant = false, fade = true, indDur, indEase }: SelectOpts = {}) {
  const changed = S.sel !== id;
  S.sel = id;
  const top = id === null ? undefined : shownTop(id);
  if (scroll && top !== undefined) {
    const st = scrollTo(top + padTop(), ROW_H, list.scrollTop, list.clientHeight);
    // 先同步滚动并重绘，保证 aria-activedescendant 指向的行已在 DOM 中
    if (st !== list.scrollTop) {
      list.scrollTop = st;
      paintRows();
    }
  }
  markSel();
  moveInd({ instant: !glide || instant, dur: indDur, ease: indEase });
  renderPreview(changed && fade && !instant);
  paintSpectrumSel(!instant);
}

// 键盘上下移动
function move(d: 1 | -1, instant: boolean) {
  const v = vis().filter((r) => !S.removing.has(r.id));
  if (!v.length) return;
  const i = v.findIndex((r) => r.id === S.sel);
  const n = Math.max(0, Math.min(v.length - 1, i + d));
  select(v[n].id, { scroll: true, instant });
}

function setPin(on: boolean, instant: boolean) {
  const sw = pinBtn.querySelector<HTMLElement>('.swap')!, st = on ? 'b' : 'a';
  if (sw.dataset.state === st) return;
  instantly(sw, instant, () => {
    sw.dataset.state = st;
  });
  pinBtn.setAttribute('aria-pressed', String(on));
  pinBtn.setAttribute('aria-label', on ? '取消置顶' : '置顶');
}

// —— 预览 ——
const MISSING = '原文件已移动或删除';
const UNPLAYABLE = '无法在预览中播放这个格式';

/** `failed`：WebView 实际播放出错后，按不可播放重绘。 */
function videoHTML(d: RecordDetail, failed = false): string {
  const f = d.files[0], v = d.video, w = v?.w ?? 0, h = v?.h ?? 0, src = v?.src ?? null;
  const playable = !failed && src !== null && canInlinePlay(f.ext);
  const pic = d.thumb
    ? `<img src="${esc(d.thumb)}" alt=""${w && h ? ` width="${w}" height="${h}"` : ''}>`
    : `<span class="poster-blank" style="aspect-ratio:${w && h ? `${w}/${h}` : '16/9'}"></span>`;
  const inner = pic + (playable ? `<span class="play">${svg('play')}</span>` : '')
    + (d.durationSec !== null ? `<span class="dur">${fmtDur(d.durationSec)}</span>` : '');
  const poster = playable
    ? `<button type="button" class="poster" data-play aria-label="播放“${esc(d.title)}”">${inner}</button>`
    : `<div class="poster">${inner}</div>`;
  const chips = v ? videoChips(v).map((c) => `<span class="chip">${esc(c)}</span>`).join('') : '';
  const note = src === null ? MISSING : playable ? '' : UNPLAYABLE;
  return `<figure class="p-video">${poster}<figcaption>${esc(f.name)}</figcaption>${chips ? `<div class="chips">${chips}</div>` : ''}${note ? `<div class="fpath">${note}</div>` : ''}</figure>`;
}

function bodyHTML(d: RecordDetail): string {
  const text = d.text ?? '';
  const trunc = d.truncated ? '<p class="trunc">仅显示前 200 KB，复制时为完整内容</p>' : '';
  switch (d.kind) {
    case 'text':
      return `<div class="p-text">${esc(text)}</div>${trunc}`;
    case 'link': {
      const h = host(text);
      // 链接判定按去掉首尾空白的文字，库里存的是原文
      const u = text.trim();
      const href = isHttpUrl(u) ? ` href="${esc(u)}" target="_blank" rel="noopener noreferrer"` : '';
      return `<div class="p-link"><div class="lk-card"><span class="lk-fav">${esc((h.replace(/^www\./, '')[0] || '?').toUpperCase())}</span><div><div class="lk-title">${esc(d.linkTitle || h)}</div><div class="lk-host">${esc(h)}</div></div></div>
          <a${href}>${esc(text)} ${svg('open')}</a>
          <div class="chips">${u.startsWith('https:') ? '<span class="chip ok">HTTPS</span>' : '<span class="chip">HTTP</span>'}<span class="chip">${esc(h)}</span></div></div>`;
    }
    case 'code':
      return `<pre class="p-code" translate="no"><code>${text.split('\n').map((l) => `<span class="ln">${hlCode(l) || ' '}</span>`).join('')}</code></pre>${trunc}`;
    case 'image': {
      const im = d.image;
      if (!im) return `<div class="p-empty">${MISSING}</div>`;
      const wh = im.w && im.h ? ` width="${im.w}" height="${im.h}"` : '';
      return `<figure class="p-img"><img src="${esc(im.src)}"${wh} alt="${esc(d.title)}"></figure>`;
    }
    case 'video':
    case 'file': {
      const reveal = openLabel('file', true, S.platform.os)!;
      const row = (f: FileEntry, i: number) => `<div class="p-file"><span class="big">${svg(d.kind)}</span><div><div class="fname">${esc(f.name)}</div><code class="fpath">${esc(f.path)}</code>
          <div class="chips">${f.ext ? `<span class="chip">${esc(f.ext.toUpperCase())}</span>` : ''}${d.kind === 'video' && d.durationSec !== null ? `<span class="chip">${fmtDur(d.durationSec)}</span>` : ''}<span class="chip">${fmtBytes(f.size)}</span></div>
          ${d.kind === 'file' ? `<div><button class="btn sm" type="button" data-reveal="${i}"${f.exists ? '' : ' disabled'}>${reveal}</button></div>` : ''}
          ${f.exists ? '' : `<div class="fpath">${MISSING}</div>`}</div></div>`;
      // 视频：第一个文件用封面卡片（点击才建 <video>，掠过时不加载），其余沿用文件行
      if (d.kind === 'video' && d.files[0]) return videoHTML(d) + d.files.slice(1).map((f, i) => row(f, i + 1)).join('');
      return d.files.map(row).join('');
    }
  }
}

function metaHTML(r: RecordSummary, d: RecordDetail | null) {
  const meta: [string, string][] = [['来源', source(r)], ['复制于', fmtDateTime(r.copiedAt)], ['大小', fmtBytes(r.size)], ['使用', `${fmtInt(r.uses)} 次`]];
  if (r.kind === 'video' && r.durationSec !== null) meta.push(['时长', fmtDur(r.durationSec)]);
  if (d) {
    const t = d.text ?? '', more = d.truncated ? '+' : '';
    if (r.kind === 'text') meta.push(['字符', fmtInt([...t].length) + more]);
    if (r.kind === 'code') meta.push(['行数', fmtInt(t.split('\n').length) + more]);
    if (r.kind === 'image' && d.image?.w) meta.push(['尺寸', `${d.image.w} × ${d.image.h}`]);
    if (r.kind === 'link') meta.push(['域名', host(t)]);
  }
  return meta.map(([k, v]) => `<div><dt>${k}</dt><dd title="${esc(v)}">${esc(v)}</dd></div>`).join('');
}

// 摘要立即渲染；正文异步取详情，序号过期的结果丢弃。同一条只取一次（摘要更新只刷新头部与信息栏）
// 最近 20 条详情缓存，命中时同步渲染；记录更新、删除、面板隐藏时作废
let detailSeq = 0, bodyFor: number | null = null;
const details = new Map<number, RecordDetail>();
function showBody(d: RecordDetail | null, fade: boolean) {
  S.detail = d;
  pbody.style.opacity = pbody.style.filter = '';
  pbody.innerHTML = d ? bodyHTML(d) : '';
  pbody.scrollTop = 0;
  const cur = d && S.records.get(d.id);
  if (cur) pmeta.innerHTML = metaHTML(cur, d);
  if (fade) fx(pbody, [{ opacity: 0.35, transform: 'translateY(4px)', filter: 'blur(2px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }], { duration: 180, easing: EASE_ENTER });
}
function renderPreview(fade = false) {
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  [pinBtn, copyBtn, delBtn].forEach((b) => (b.disabled = !r));
  $('#selHint').textContent = r ? `已选中：${truncate(r.title, 18)}` : '没有选中的记录';
  if (!r) {
    ++detailSeq;
    bodyFor = null;
    S.detail = null;
    ptype.innerHTML = '';
    pmeta.innerHTML = '';
    pbody.style.opacity = pbody.style.filter = '';
    pbody.innerHTML = '<div class="p-empty">在左侧选择一条记录查看完整内容</div>';
    return;
  }
  ptype.innerHTML = `<span class="tchip" data-type="${r.kind}">${svg(r.kind)}${LABEL[r.kind]}</span>`
    + (isFav(r, favMin()) ? `<span class="tchip" data-type="fav">${svg('star')}常用</span>` : '')
    + (r.lang ? `<span class="chip">${esc(r.lang)}</span>` : '');
  setPin(r.pinned, true);
  pmeta.innerHTML = metaHTML(r, S.detail?.id === r.id ? S.detail : null);
  if (bodyFor === r.id) return;
  bodyFor = r.id;
  const seq = ++detailSeq;
  const hit = details.get(r.id);
  if (hit) return showBody(hit, fade);
  S.detail = null;
  // 新头部不配旧正文：旧正文先压暗，详情到达后替换（需要动画时浮入）
  pbody.style.opacity = '.35';
  pbody.style.filter = 'blur(2px)';
  getRecord(r.id).then(
    (d) => {
      if (isStale(seq, detailSeq)) return;
      // 请求途中这条记录更新过，到达的详情可能是旧的：照常显示但不进缓存
      if (d && S.records.get(d.id) === r) {
        details.delete(d.id);
        details.set(d.id, d);
        if (details.size > 20) details.delete(details.keys().next().value!);
      }
      showBody(d, fade);
    },
    (e: AppError) => {
      if (isStale(seq, detailSeq)) return;
      bodyFor = null;
      pbody.style.opacity = pbody.style.filter = '';
      pbody.innerHTML = '<div class="p-empty">在左侧选择一条记录查看完整内容</div>';
      void info(`get_record 失败 id=${r.id} kind=${e.kind}`);
    },
  );
}

// —— 复制频谱 ——
// 每 2px 一列、一列一根光柱（spectrum.ts）；sp 是当前布局，spNow 是布局用的时刻，游标按同一时刻定位
let sp: Layout = { bars: [], colOf: new Map() };
let spNow = Date.now();
let spOn: Element | null = null;
// 收起状态以 data-collapsed 为准：S.settings 要等保存成功才更新，读它会落后 300ms 防抖
const spectrum = $('#spectrum'), spToggle = $('#spToggle');
const spCollapsed = () => spectrum.dataset.collapsed === 'true';
function paintSpCollapsed(c: boolean) {
  spectrum.dataset.collapsed = String(c);
  spToggle.setAttribute('aria-expanded', String(!c));
  spToggle.setAttribute('aria-label', c ? '展开复制频谱' : '收起复制频谱');
}
function renderSpectrum(freshId?: number) {
  spNow = Date.now();
  // 统计不依赖轨道宽度，收起时照常更新
  const { count, peak } = spectrumStat(S.records.values(), spNow);
  $('#spStat').textContent = `24 小时内复制 ${fmtInt(count)} 次${peak ? ` · 最忙 ${peak.hour} 点（${peak.n} 次）` : ''}`;
  if (spCollapsed()) return;
  // 删除中的记录不参与布局，否则重绘会把正在收起的光柱拉回来
  const recs = [...S.records.values()].filter((r) => !S.removing.has(r.id));
  const next = layoutBars(recs, spNow, spTrack.clientWidth, matches, freshId);
  const cls = (b: Layout['bars'][number]) => `bar${b.dim ? ' dim' : ''}${b.fresh ? ' fresh' : ''}`;
  const n = next.bars.length;
  if (sameColumns(sp, next) && spBars.children.length === n) {
    // 列没变（切分类、搜索、时间小幅流逝）：原地改，.dim/.on 的 200ms 过渡照常播放（原型同样是原地切类）
    next.bars.forEach((b, i) => {
      const el = spBars.children[i] as HTMLElement;
      el.className = cls(b);
      el.dataset.type = b.kind;
      el.style.left = b.x + '%';
      el.style.height = b.h + '%';
      // 撤销删除时光柱还在原列，但 remove() 的收起动画 fill: forwards 仍压着它：取消脚本动画，CSS 过渡照常
      el.getAnimations().forEach((a) => {
        if (!(a instanceof CSSAnimation) && !(a instanceof CSSTransition)) a.cancel();
      });
    });
  } else {
    spBars.innerHTML = next.bars.map((b, i) =>
      `<span class="${cls(b)}" data-col="${i}" data-type="${b.kind}" style="left:${b.x}%;height:${b.h}%;--i:${staggerIndex(i, n)}"></span>`).join('');
    // 悬停的光柱已被替换，提示条不再对应
    spTip.classList.remove('show');
  }
  sp = next;
  // className 重写会去掉 .on，两条路径都由 paintSpectrumSel 重新标记
  spOn = null;
  const ticks: [number, string][] = [[3, '更早'], ...[18, 12, 6].map((h): [number, string] => [6 + 94 * (1 - h / 24), hf.format(spNow - h * 3600e3)]), [100, '现在']];
  $('#spAxis').innerHTML = ticks.map(([x, l]) => `<span style="left:${x}%">${l}</span>`).join('');
  paintSpectrumSel(false);
}
// 只换上一根和这一根的 .on；变暗由布局决定，筛选变化时重绘
function paintSpectrumSel(animate = true) {
  if (spCollapsed()) return;
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  const col = r && sp.colOf.get(r.id);
  const b = col === undefined ? null : spBars.children[col] ?? null;
  if (b !== spOn) {
    spOn?.classList.remove('on');
    b?.classList.add('on');
    spOn = b;
  }
  // 超过 7 天的记录没有光柱，游标不指向空处
  if (!r || !b) {
    spCursor.style.opacity = '0';
    return;
  }
  spCursor.style.transitionDuration = animate && !reduced.matches ? '' : '0ms';
  // 用选中记录自己的位置，不用所在列代表记录的
  spCursor.style.transform = `translateX(${(spTrack.clientWidth * (xPct(spNow - r.copiedAt) ?? 1)) / 100}px)`;
  spCursor.style.opacity = '1';
}
const barOf = (e: Event) => {
  const b = (e.target as Element).closest<HTMLElement>('.bar');
  const bar = b ? sp.bars[Number(b.dataset.col)] : undefined;
  const r = bar && S.records.get(bar.id);
  return b && r ? { b, r } : null;
};
spTrack.addEventListener('pointerover', (e) => {
  const hit = barOf(e);
  if (!hit) return;
  spTip.dataset.type = hit.r.kind;
  spTip.innerHTML = `<i></i>${esc(hit.r.title)}<span>${rel(hit.r.copiedAt)}</span>`;
  spTip.style.left = Math.max(0, Math.min(spTrack.clientWidth - spTip.offsetWidth, hit.b.offsetLeft - spTip.offsetWidth / 2)) + 'px';
  spTip.classList.add('show');
});
spTrack.addEventListener('pointerout', (e) => {
  if (!(e.relatedTarget as Element | null)?.closest?.('.bar')) spTip.classList.remove('show');
});
spTrack.addEventListener('click', (e) => {
  const hit = barOf(e);
  if (!hit || S.removing.has(hit.r.id)) return;
  if (!matches(hit.r)) {
    S.filter = 'all';
    persistFilter();
    q.value = '';
    renderTabs(false);
    runSearch('', { stagger: true });
  }
  select(hit.r.id, { scroll: true, indDur: 260 });
});

// —— 分类标签 ——
const tabColor = (k: string) => (k === 'all' ? 'var(--beam)' : `var(--c-${k})`);
function renderTabs(instant: boolean) {
  const c = counts(S.records, favMin());
  const html = (real: boolean) => FILTERS.map(([k, l]) => {
    const mark = k === 'fav' ? svg('star', 'star') : '<i class="dot"></i>';
    const tag = real ? `button type="button" class="tab" data-f="${k}" aria-pressed="${S.filter === k}"` : 'span class="tab"';
    return `<${tag} data-type="${k}">${mark}${l}<span class="n">${c[k]}</span></${real ? 'button' : 'span'}>${k === 'fav' ? '<span class="tabs-sep"></span>' : ''}`;
  }).join('');
  const ae = document.activeElement as HTMLElement | null;
  const refocus = ae?.classList.contains('tab') ? ae.dataset.f : undefined;
  tabsBase.innerHTML = html(true);
  tabsOver.innerHTML = html(false);
  if (refocus) tabsBase.querySelector<HTMLElement>(`[data-f="${refocus}"]`)?.focus({ preventScroll: true });
  // 颜色带的色标对齐到每个标签中心，指示条停在哪就是哪个类型的颜色
  const stops = [...tabsBase.querySelectorAll<HTMLElement>('.tab')].map((b) => `${tabColor(b.dataset.f!)} ${b.offsetLeft + b.offsetWidth / 2}px`);
  tabsOver.style.background = `linear-gradient(90deg, ${stops.join(', ')})`;
  const b = tabsBase.querySelector<HTMLElement>(`[data-f="${S.filter}"]`)!;
  const l = b.offsetLeft, r = tabs.offsetWidth - l - b.offsetWidth;
  instantly(tabsOver, instant, () => {
    tabsOver.style.clipPath = `inset(0 ${r}px 0 ${l}px round 999px)`;
  });
  if (instant) b.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}
// 界面记忆（分类 M06 等）：合并写进 settings，失败不提示。
// 防抖 300ms：按住 Mod+←/→ 连续切换时只写最后落定的值；成功后只合并本次 patch，不回滚同时进行的设置保存
let uiTimer = 0;
let uiPending: Partial<Settings> = {};
function persistUi(patch: Partial<Settings>) {
  uiPending = { ...uiPending, ...patch };
  clearTimeout(uiTimer);
  uiTimer = window.setTimeout(() => {
    const p = uiPending;
    uiPending = {};
    if ((Object.keys(p) as (keyof Settings)[]).every((k) => S.settings[k] === p[k])) return;
    saveSettings({ ...S.settings, ...p }).then(
      () => void (S.settings = { ...S.settings, ...p }),
      (e: AppError) => void info(`保存界面状态失败 kind=${e.kind}`),
    );
  }, 300);
}
function persistFilter() { persistUi({ lastFilter: S.filter }); }
function setFilter(k: Filter, instant: boolean) {
  if (S.filter === k) return;
  S.filter = k;
  persistFilter();
  renderTabs(instant);
  // M06 规则 6：切换分类后回到顶部并选中第一条
  list.scrollTop = 0;
  S.sel = null;
  // 先重绘频谱（变暗随分类变），再由 renderList 的选中移动游标
  renderSpectrum();
  renderList({ stagger: !instant });
  paintSpectrumSel(!instant);
}

// —— 搜索 ——
// 每次输入都查后端，不防抖；序号丢弃先发后到的结果
let searchSeq = 0;
// 唤起时后端带了 t0 说明 XPASTE_TIMING 已开启；搜索计时从第一次快捷键唤起之后才开始
let timing = false;
function fetchMatches(v: string, done: () => void) {
  const seq = ++searchSeq;
  if (!v.trim()) {
    S.matchIds = null;
    done();
    return;
  }
  search(v).then(
    (ids) => {
      if (isStale(seq, searchSeq)) return;
      S.matchIds = new Set(ids);
      done();
    },
    (e: AppError) => void info(`search 失败 kind=${e.kind}`),
  );
}
// 用户搜索：结果回到顶部并选中第一条（唤起时也走这里）
// timed：只有 input / compositionend 路径计时，唤起、清空等程序调用不算搜索样本
function runSearch(v: string, opts: { stagger?: boolean; base?: number; timed?: boolean } = {}) {
  const t = performance.now();
  S.q = v;
  clearQ.hidden = !v;
  escHintEl.textContent = escHint(!!v);
  fetchMatches(v, () => {
    list.scrollTop = 0;
    S.sel = null;
    renderSpectrum();
    renderList(opts);
    if (timing && opts.timed) {
      const n = S.matchIds?.size ?? S.records.size;
      requestAnimationFrame(
        () => void info(`搜索耗时 ${Math.round(performance.now() - t)} ms，命中 ${n} 条`),
      );
    }
  });
}

// —— 提示条 ——
// 连续触发时只换文字、不重播弹入；非撤销提示只顶掉撤销按钮，Mod + Z 在撤销期限内仍有效（canUndo 按时间判断）。undo 与 action 不同时出现
type ToastAction = { label: string; run: () => void };
let toastTimer = 0;
let toastRun: (() => void) | null = null;
function toast(msg: string, { undo = false, icon = 'check' as keyof typeof I, action = undefined as ToastAction | undefined } = {}) {
  paintUndoHint();
  $('#toastIc').innerHTML = svg(icon);
  if (toastEl.dataset.open === 'true') fx(toastMsg, [{ opacity: 0, transform: 'translateY(4px)', filter: 'blur(2px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }], { duration: 180, easing: EASE_ENTER });
  toastMsg.textContent = msg;
  undoBtn.hidden = !undo;
  toastAct.hidden = !action;
  toastRun = action?.run ?? null;
  if (action) toastAct.textContent = action.label;
  toastEl.dataset.open = 'true';
  if (undo) {
    const ring = $('#ring');
    ring.getAnimations().forEach((a) => a.cancel());
    ring.animate([{ strokeDashoffset: 0 }, { strokeDashoffset: 44 }], { duration: UNDO_MS, fill: 'forwards' });
  }
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl.dataset.open = 'false';
  }, undo || action ? UNDO_MS : 2400);
}

// —— 操作 ——
// 结果以后端事件为准：copy 之后 record-updated 带来新的 uses / copiedAt
let copyTimer = 0;
function copyRecord(id: number) {
  const before = S.records.get(id);
  if (!before || S.removing.has(id)) return;
  // 粘贴成功由后端直接隐藏面板（无淡出）；只复制或失败时面板不动
  copy(id, true).then(
    (res) => {
      const cur = S.records.get(id);
      // 事件可能还没到；并发的置顶、补全更新也不会让次数变少
      const uses = Math.max(cur?.uses ?? 0, before.uses + 1);
      const fav = !isFav(before, favMin()) && isFav({ uses }, favMin());
      const msg = copyToast(res, S.platform.os, fav ? uses : null);
      if (msg) {
        toast(msg, {
          icon: fav ? 'star' : 'check',
          action: needsAccessibility(res)
            ? { label: '前往授权', run: () => void requestAccessibility().catch((e: AppError) => toast(e.message, { icon: 'copy' })) }
            : undefined,
        });
      }
      if (!S.shown || res.result === 'pasted') return;
      const sw = copyBtn.querySelector<HTMLElement>('.swap')!;
      sw.dataset.state = 'b';
      clearTimeout(copyTimer);
      copyTimer = window.setTimeout(() => (sw.dataset.state = 'a'), 1300);
      beam = { id, t0: performance.now() };
      // “移到最上方”会改变位置，保持选中行可见
      renderList({ scroll: true });
      const r = rowEl(id);
      r?.classList.remove('beam');
      void r?.offsetWidth;
      paintBeam(r);
    },
    (err: AppError) => toast(err.message, { icon: 'copy' }),
  );
}

// 乐观更新，失败回滚；鼠标操作时 FLIP 平移到新分组，起点或终点没渲染就直接到位
function togglePin(id: number, animate: boolean) {
  const r = S.records.get(id);
  if (!r || S.removing.has(id)) return;
  const on = !r.pinned;
  const setLocal = (pinned: boolean) => {
    const cur = S.records.get(id);
    if (cur) S.records.set(id, { ...cur, pinned });
    if (id === S.sel) setPin(pinned, !animate);
  };
  const r0 = animate && !reduced.matches ? rowEl(id)?.getBoundingClientRect() : undefined;
  const indFrom = ind.style.transform;
  setLocal(on);
  renderList({ scroll: id === S.sel });
  const el = rowEl(id), top = topOf(S.cells, id);
  const dy = r0 && el ? r0.top - el.getBoundingClientRect().top : 0;
  if (el && dy) {
    // 抬起的行要压在它滑过的 .vrow 之上，层级给外层定位元素
    const v = el.closest<HTMLElement>('.vrow')!;
    el.classList.add('lift');
    v.style.zIndex = '2';
    el.animate([{ transform: `translateY(${dy}px) scale(1.02)` }, { transform: 'translateY(0) scale(1)' }], { duration: 300, easing: EASE_MOVE })
      .finished.finally(() => {
        el.classList.remove('lift');
        v.style.zIndex = '';
      }).catch(() => {});
    if (id === S.sel && top !== undefined) {
      // 选中光标用同样的时长和曲线跟随
      ind.style.transition = 'none';
      ind.style.transform = indFrom;
      void ind.offsetWidth;
      ind.style.transition = `transform 300ms ${EASE_MOVE}, opacity 160ms ease`;
      ind.style.transform = `translateY(${top}px)`;
    }
  }
  setPinned(id, on).then(
    () => toast(on ? '已置顶，会一直显示在列表最上方' : '已取消置顶', { icon: on ? 'pinFill' : 'pin' }),
    (err: AppError) => {
      setLocal(!on);
      renderList();
      toast(err.message);
    },
  );
}

// 先播放滑出，后端确认后再从列表移除；records-removed 对 S.removing 中的 id 不做处理
function remove(id: number) {
  const r = S.records.get(id);
  if (!r || S.removing.has(id)) return;
  const next = nextSelection(vis().filter((x) => !S.removing.has(x.id)), id);
  const p = painted.get('r' + id), c = S.cells.find((c) => c.kind === 'row' && c.id === id);
  S.removing.add(id);
  ownEcho.add(id);
  if (p && c?.kind === 'row') {
    // 直接加类让过渡播放；同步 p.h，避免下一次重绘整行替换
    p.el.querySelector('.item')?.classList.add('is-removing');
    p.h = rowHTML(r, c.index, false, false);
  }
  // 下方的行与 .item.is-removing 的收起同步上移一行（滚到底时整体按夹紧量修正，见 goneOf）
  paintRows({ glide: `transform 240ms ${EASE_MOVE} 100ms`, glideBusy: `transform 240ms ${EASE_MOVE}` });
  // 光柱只代表这一条时才收起；同列还有别的记录就等删除后的重绘
  const col = sp.colOf.get(id);
  const bar = col !== undefined && sp.bars[col].ids.length === 1 ? spBars.children[col] ?? null : null;
  fx(bar, [{ transform: 'scaleY(1)', opacity: 1 }, { transform: 'scaleY(0)', opacity: 0 }], { duration: 260, easing: EASE_MOVE, fill: 'forwards' });
  // 光标目标是下一条补位后的位置：同组时就是被删行的位置，光标停住、下一条滑进来（原型 1384–1393）
  select(next, { scroll: true, indDur: 340 });
  deleteRecord(id).then(
    () => {
      setTimeout(() => {
        if (!S.removing.delete(id)) return;
        S.records.delete(id);
        renderTabs(true);
        renderList();
        renderSpectrum();
      }, reduced.matches ? 0 : 360);
      toast(`已删除“${truncate(r.title, 14)}”`, { undo: true, icon: 'trash' });
      S.undo = armUndo(S.undo, id, r.title, Date.now());
      paintUndoHint();
      setTimeout(paintUndoHint, UNDO_MS);
    },
    (err: AppError) => {
      S.removing.delete(id);
      ownEcho.delete(id);
      // 重绘把光柱补回来：没重建过时原地路径取消收起动画，删除途中重建过时按新布局补上
      renderSpectrum();
      paintRows({ glide: ENTER_GLIDE });
      select(id, { scroll: true });
      toast(err.message);
    },
  );
}

// 自己删除后后端回发的 records-removed，收到一次就消掉
const ownEcho = new Set<number>();

// 恢复后后端发 record-added，由 handlers.added 插入并选中
let restoring: number | null = null;
function undo() {
  const u = S.undo;
  if (!u || !canUndo(u, Date.now())) return;
  S.undo = null;
  paintUndoHint();
  restoring = u.id;
  undoDelete(u.id).then(
    (ok) => {
      if (!ok) restoring = null;
      toast(ok ? '已恢复' : '已无法恢复');
    },
    (err: AppError) => {
      restoring = null;
      toast(err.message);
    },
  );
}

// —— 右键菜单 ——
let menuReturn: HTMLElement | null = null;
function openMenu(id: number, x: number, y: number, viaKb: boolean, detail = details.get(id)) {
  const r = S.records.get(id);
  if (!r) return;
  // 图片是否来自文件只有详情知道：没缓存时先取（本地调用，几毫秒），回来时已隐藏、换了选中或菜单已开就作罢
  if (r.kind === 'image' && !detail) {
    getRecord(id).then((d) => d && S.shown && S.sel === id && !menuOpen() && openMenu(id, x, y, viaKb, d), (err: AppError) => toast(err.message));
    return;
  }
  S.menuFor = id;
  $('#mPin').textContent = r.pinned ? '取消置顶' : '置顶';
  const open = openLabel(r.kind, !!detail?.files.length, S.platform.os);
  mOpen.hidden = !open;
  $('#mOpen').textContent = open ?? '';
  // stagger 序号按可见项重排，隐藏的项不留空拍
  menu.querySelectorAll<HTMLElement>('button:not([hidden])').forEach((b, i) => b.style.setProperty('--i', String(i)));
  menuReturn = document.activeElement as HTMLElement | null;
  const wr = win.getBoundingClientRect(), mw = menu.offsetWidth, mh = menu.offsetHeight;
  let left = x - wr.left, top = y - wr.top, ox = 'left', oy = 'top';
  if (left + mw > wr.width - 8) {
    left = Math.max(8, left - mw);
    ox = 'right';
  }
  if (top + mh > wr.height - 8) {
    top = Math.max(8, top - mh);
    oy = 'bottom';
  }
  menu.style.left = left + 'px';
  menu.style.top = top + 'px';
  menu.style.transformOrigin = `${ox} ${oy}`;
  menu.toggleAttribute('data-instant', viaKb);
  menu.dataset.open = 'true';
  menu.querySelector('button')!.focus({ preventScroll: true });
}
function closeMenu(restore: boolean) {
  if (!menuOpen()) return;
  menu.dataset.open = 'false';
  S.menuFor = null;
  const back = restore && menuReturn && document.contains(menuReturn) ? menuReturn : null;
  // 焦点不能留在隐藏的菜单里，否则 Enter 还会触发旧动作
  if (back) back.focus({ preventScroll: true });
  else if (menu.contains(document.activeElement)) q.focus({ preventScroll: true });
}
function moveMenuFocus(d: 1 | -1) {
  const bs = [...menu.querySelectorAll<HTMLElement>('button:not([hidden])')];
  const i = bs.indexOf(document.activeElement as HTMLElement);
  bs[(i + d + bs.length) % bs.length].focus();
}
// 菜单的打开动作：链接交给浏览器；文件、视频、文件来源的图片在访达 / 资源管理器中显示第一个文件
function openFromMenu(id: number) {
  const hit = details.get(id);
  (hit ? Promise.resolve(hit) : getRecord(id))
    .then((d) => {
      if (d?.kind === 'link') return openUrl((d.text ?? '').trim());
      if (d?.files[0]) return revealFile(d.files[0].path);
    })
    .catch((err: AppError) => toast(err.kind === 'fileMissing' ? MISSING : err.message));
}

function paintPause() {
  pauseBtn.querySelector('span')!.textContent = S.paused ? '已暂停' : '正在记录';
  pauseBtn.setAttribute('aria-pressed', String(S.paused));
  pauseBtn.dataset.paused = String(S.paused);
}

// —— 后端事件 ——
// 外部新记录（设计 §2 回写项 2）：不改搜索、分类、选中项，不弹提示；只有撤销恢复的那条会被选中
const handlers = {
  added: (s: RecordSummary) => {
    applyAdded(S.records, s);
    S.removing.delete(s.id);
    const restored = s.id === restoring;
    if (restored) restoring = null;
    const paint = () => {
      // newId 只在插入这一次渲染时有效，之后的重绘不重播动画
      if (S.shown) S.newId = s.id;
      if (restored) S.sel = s.id;
      renderTabs(true);
      // 面板可见时下方的行滑到新位置，新行自己展开弹入
      renderList({ scroll: restored, glide: S.shown ? ENTER_GLIDE : '' });
      renderSpectrum(s.id);
      S.newId = null;
    };
    // 搜索中：新记录可能命中，重新取命中集合但不重置选中
    if (S.q.trim()) fetchMatches(S.q, paint);
    else paint();
  },
  updated: (s: RecordSummary) => {
    const prev = S.records.get(s.id);
    details.delete(s.id);
    applyUpdated(S.records, s);
    renderTabs(true);
    // 选中视频的补全到了（时长或封面变了）：正文重取以显示封面与标签；正在播放（已有 <video>）时不打断。
    // 置顶、复制计数、图标等其他更新不重取，免得预览滚动位置与焦点丢失
    if (s.id === bodyFor && s.kind === 'video' && !pbody.querySelector('video')
      && (prev?.durationSec !== s.durationSec || prev?.thumb !== s.thumb)) bodyFor = null;
    // 去重命中会改 copiedAt，需要重新排序；选中项的头部与信息栏随 select 刷新，不重取详情
    renderList();
    renderSpectrum(prev && prev.copiedAt !== s.copiedAt ? s.id : undefined);
  },
  removed: (ids: number[]) => {
    // 别处的删除（保留时长清理、清空历史）清掉了待撤销的记录：撤销已无意义
    const foreign = ids.filter((id) => !ownEcho.delete(id));
    ids.forEach((id) => details.delete(id));
    if (S.undo && foreign.includes(S.undo.id)) {
      S.undo = null;
      paintUndoHint();
      // 提示条已换成别的提示时不收起它
      if (!undoBtn.hidden) {
        clearTimeout(toastTimer);
        undoBtn.hidden = true;
        toastEl.dataset.open = 'false';
        $('#ring').getAnimations().forEach((a) => a.cancel());
      }
    }
    if (S.menuFor !== null && foreign.includes(S.menuFor)) closeMenu(false);
    // 自己发起的删除由 remove() 的计时器移除，这里跳过
    const gone = ids.filter((id) => S.records.has(id) && !S.removing.has(id));
    if (!gone.length) return;
    // 与 remove() 一致：正在删除的行不作为下一个选中
    const before = vis().filter((r) => !S.removing.has(r.id));
    applyRemoved(S.records, gone);
    if (S.sel !== null && gone.includes(S.sel)) S.sel = nextSelection(before, S.sel);
    renderTabs(true);
    renderList();
    renderSpectrum();
  },
  paused: (p: boolean) => {
    S.paused = p;
    paintPause();
  },
};

// —— 确认框 ——
// 与设置对话框同一套弹入 / 150ms 收起。Esc / 取消 / 遮罩 → false，#cfOk → true；
// 结果在点击时就交出去（调用方不必等收起动画），面板隐藏时直接 close 也按 false 结算。
// 同一时间只有一个确认框：已有一个在等待时，新的请求直接得到 false
const cfDlg = $<HTMLDialogElement>('#confirm'), cfOk = $<HTMLButtonElement>('#cfOk');
// 设置或确认框打开时后端不因失焦或快捷键隐藏面板：每次 showModal 之后与每个 close 事件里按 DOM 实际状态同步。
// 引导不算：其中“授权”要切去系统设置，面板浮在最上层会盖住它，照旧失焦隐藏
const keepDlgs = [...document.querySelectorAll<HTMLDialogElement>('#settings, #confirm')];
const syncDialogOpen = () =>
  void setDialogOpen(keepDlgs.some((d) => d.open)).catch((e: AppError) => void info(`同步对话框状态失败 kind=${e.kind}`));
keepDlgs.forEach((d) => d.addEventListener('close', syncDialogOpen));
let cfResolve: ((ok: boolean) => void) | null = null, cfReturn: HTMLElement | null = null, cfTimer = 0;
function confirm({ title, body, ok, danger = false }: { title: string; body: string; ok: string; danger?: boolean }) {
  if (cfResolve) return Promise.resolve(false);
  clearTimeout(cfTimer);
  // 收起途中再次打开：沿用最初的返回焦点
  if (!cfDlg.open) cfReturn = document.activeElement as HTMLElement | null;
  $('#cf-title').textContent = title;
  $('#cfBody').textContent = body;
  cfOk.textContent = ok;
  cfOk.className = `btn ${danger ? 'danger' : 'primary'}`;
  if (!cfDlg.open) cfDlg.showModal();
  syncDialogOpen();
  void cfDlg.offsetWidth;
  cfDlg.classList.add('is-shown');
  $('#cfCancel').focus();
  return new Promise<boolean>((r) => (cfResolve = r));
}
function settleConfirm(ok: boolean) {
  cfResolve?.(ok);
  cfResolve = null;
}
function closeConfirm(ok: boolean) {
  settleConfirm(ok);
  cfDlg.classList.remove('is-shown');
  clearTimeout(cfTimer);
  cfTimer = window.setTimeout(() => cfDlg.close(), reduced.matches ? 0 : 150);
}
cfDlg.addEventListener('close', () => {
  // 上一次 close 排队的事件到达时已重新打开：不能结算新的确认框
  if (cfDlg.open) return;
  cfDlg.classList.remove('is-shown');
  settleConfirm(false);
  if (S.shown && cfReturn?.isConnected) cfReturn.focus({ preventScroll: true });
  cfReturn = null;
});
cfDlg.addEventListener('cancel', (e) => {
  e.preventDefault();
  closeConfirm(false);
});
let cfDownOnBackdrop = false;
cfDlg.addEventListener('pointerdown', (e) => (cfDownOnBackdrop = e.target === cfDlg));
cfDlg.addEventListener('click', (e) => {
  const t = e.target as Element;
  if (t.closest('#cfOk')) closeConfirm(true);
  else if (t.closest('#cfCancel') || (t === cfDlg && cfDownOnBackdrop)) closeConfirm(false);
});
// 托盘“清空历史…”：后端已先唤起面板。置顶记录保留；列表由 records-removed 更新，待撤销状态由 removed 处理清掉
// 返回提示文案（取消时为 null），设置对话框里的入口把它写进行内（提示条在对话框下面看不见）
async function confirmClear(): Promise<string | null> {
  let msg: string, icon: keyof typeof I = 'check';
  try {
    const n = await countUnpinned();
    if (n === 0) msg = '没有可删除的记录';
    else if (!(await confirm({ title: '清空历史', body: `将删除 ${n} 条未置顶的记录，置顶记录会保留`, ok: `删除 ${n} 条记录`, danger: true }))) return null;
    else [msg, icon] = [`已删除 ${await clearHistory()} 条记录`, 'trash'];
  } catch (e) {
    msg = (e as AppError).message;
  }
  toast(msg, { icon });
  return msg;
}

// —— 设置 ——
const clearNote = $('#clearNote');
const dlg = $<HTMLDialogElement>('#settings'), form = $<HTMLFormElement>('#settingsForm'), hotkeyIn = $<HTMLInputElement>('#hotkey');
const hotkeyHelp = $('#hotkeyHelp'), retainIn = $<HTMLSelectElement>('#retain'), limitIn = $<HTMLInputElement>('#limit'), limitErr = $('#limitErr');
const frequentIn = $<HTMLInputElement>('#frequentIn'), frequentErr = $('#frequentErr');
const ignoreIn = $<HTMLInputElement>('#ignore'), sensitiveIn = $<HTMLInputElement>('#sensitive');
const moveTopIn = $<HTMLInputElement>('#moveTop'), autostartIn = $<HTMLInputElement>('#autostart');
const permRow = $('#permRow'), permState = $('#permState'), permBtn = $<HTMLButtonElement>('#permBtn');
const themeSeg = $('#themeSeg'), themePill = themeSeg.querySelector<HTMLElement>('.pill')!;
const themeOpts = [...themeSeg.querySelectorAll<HTMLElement>('[data-theme-opt]')];
const HOTKEY_IDLE = '点击输入框，然后按下新的组合键';
let pendingHotkey = '', dlgReturn: HTMLElement | null = null, closeTimer = 0, saving = false;

// 不记录这些应用：芯片 + 组合框（M15 P1）。草稿只在保存时写回，取消 / Esc / 遮罩丢弃
const ignoreBox = $('#ignoreBox'), ignoreChips = $('#ignoreChips'), ignoreList = $('#ignoreList');
let draftIgnored: string[] = [], allApps: string[] = [], appOpts: string[] = [], appActive = -1, escClosedList = false;
// from 起的芯片是新加的：弹入
function paintChips(from = Infinity) {
  ignoreChips.innerHTML = draftIgnored
    .map((n, i) => `<span class="chip app-chip">${esc(n)}<button type="button" data-rm="${i}" aria-label="移除 ${esc(n)}">${svg('x')}</button></span>`)
    .join('');
  [...ignoreChips.children].slice(from).forEach((c) => fx(c, [{ transform: 'scale(.9)' }, { transform: 'scale(1)' }], { duration: 220, easing: EASE_SPRING }));
}
// 只有键盘移动才滚动：悬停时滚动会让列表在鼠标下面爬
function setAppActive(i: number, scroll = true) {
  appActive = i;
  [...ignoreList.children].forEach((li, j) => li.setAttribute('aria-selected', String(j === i)));
  if (i < 0) return ignoreIn.removeAttribute('aria-activedescendant');
  ignoreIn.setAttribute('aria-activedescendant', `app-opt-${i}`);
  if (scroll) ignoreList.children[i].scrollIntoView({ block: 'nearest' });
}
// 有候选才展开
function paintAppList(open: boolean) {
  const wasHidden = ignoreList.hidden;
  appOpts = open ? pickerOptions(allApps, draftIgnored, ignoreIn.value) : [];
  ignoreList.innerHTML = appOpts.map((n, i) => `<li role="option" id="app-opt-${i}" aria-selected="false">${esc(n)}</li>`).join('');
  ignoreList.hidden = appOpts.length === 0;
  ignoreIn.setAttribute('aria-expanded', String(!ignoreList.hidden));
  setAppActive(-1);
  // 刚展开时整张列表滚入对话框中部滚动区，免得下半截压在页脚下面
  if (wasHidden && !ignoreList.hidden) ignoreList.scrollIntoView({ block: 'nearest' });
}
function addApps(text: string) {
  const from = draftIgnored.length;
  draftIgnored = addIgnored(draftIgnored, text);
  ignoreIn.value = '';
  paintChips(from);
  paintAppList(true);
}
ignoreIn.addEventListener('focus', () => paintAppList(true));
ignoreIn.addEventListener('input', () => paintAppList(true));
ignoreIn.addEventListener('blur', () => paintAppList(false));
ignoreIn.addEventListener('keydown', (e) => {
  // 输入法组字中的回车 / 方向键归输入法
  if (e.isComposing || e.keyCode === 229) return;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (ignoreList.hidden) paintAppList(true);
    const n = appOpts.length, d = e.key === 'ArrowDown' ? 1 : -1;
    if (n) setAppActive(appActive < 0 ? (d > 0 ? 0 : n - 1) : (appActive + d + n) % n);
  } else if (e.key === 'Enter') {
    // 有高亮加高亮项，否则加输入的文字；输入为空且无高亮时照常提交
    if (appActive >= 0) addApps(appOpts[appActive]);
    else if (ignoreIn.value.trim()) addApps(ignoreIn.value);
    else return;
    e.preventDefault();
  } else if (e.key === 'Escape' && !ignoreList.hidden) {
    // 列表展开时 Esc 只收起列表；旧 WebKit 仍会触发对话框的 cancel，在那里按此标记拦下
    e.preventDefault();
    e.stopPropagation();
    paintAppList(false);
    escClosedList = true;
    setTimeout(() => (escClosedList = false));
  } else if (e.key === 'Backspace' && !ignoreIn.value && draftIgnored.length) {
    e.preventDefault();
    draftIgnored = draftIgnored.slice(0, -1);
    paintChips();
    paintAppList(!ignoreList.hidden);
  }
});
// 点候选不抢焦点：输入框不失焦、列表不先收起。Chromium / WebView2 只拦 pointerdown 不够，mousedown 也要拦
ignoreList.addEventListener('pointerdown', (e) => e.preventDefault());
ignoreList.addEventListener('mousedown', (e) => e.preventDefault());
// 悬停即高亮：鼠标与键盘共用一个高亮项，Enter 加的就是看到的那项
ignoreList.addEventListener('pointermove', (e) => {
  const li = (e.target as Element).closest('[role="option"]');
  const i = li ? [...ignoreList.children].indexOf(li) : -1;
  if (i >= 0 && i !== appActive) setAppActive(i, false);
});
ignoreList.addEventListener('click', (e) => {
  const li = (e.target as Element).closest('[role="option"]');
  if (li) addApps(appOpts[[...ignoreList.children].indexOf(li)]);
});
ignoreBox.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('[data-rm]');
  if (b) {
    draftIgnored = removeIgnored(draftIgnored, draftIgnored[Number(b.dataset.rm)]);
    paintChips();
  }
  if (b || e.target === ignoreBox) ignoreIn.focus();
});

// 外观：点选即生效并立即保存（不防抖、不预览，关闭对话框不还原）。窗口主题由 save_settings 切换；
// 切到“跟随系统”时 prefers-color-scheme 随窗口主题翻转，由下方 matchMedia 监听整体切换
const themeMode = () => (root.dataset.theme || 'system') as Appearance;
function paintTheme(instant: boolean) {
  const mode = themeMode();
  themeOpts.forEach((b, i) => {
    const on = b.dataset.themeOpt === mode;
    b.setAttribute('aria-checked', String(on));
    b.tabIndex = on ? 0 : -1;
    if (on) instantly(themePill, instant, () => (themePill.style.transform = `translateX(${i * 32}px)`));
  });
}
// 整体一次切换：两帧内关掉所有过渡，不让各元素按各自的过渡渐变（M16 规则 5）
function setTheme(mode: Appearance) {
  root.setAttribute('data-theme-switching', '');
  if (mode === 'system') delete root.dataset.theme;
  else root.dataset.theme = mode;
  paintTheme(true);
  requestAnimationFrame(() => requestAnimationFrame(() => root.removeAttribute('data-theme-switching')));
}
// save_settings 是异步命令，连点时并发保存可能乱序落盘：串行，上一次结束后再发当时最新的设置。
// savedAppearance 是最近一次确认落盘的外观，保存失败时回到它
let themeSave: Promise<unknown> = Promise.resolve(), savedAppearance: Appearance = 'dark';
async function applyAppearance(mode: Appearance) {
  if (mode === themeMode()) return;
  // 先改本地设置：保存途中 persistUi 落盘或点“保存设置”带上的都是新外观，不会把主题改回去
  S.settings = { ...S.settings, appearance: mode };
  setTheme(mode);
  const save = (themeSave = themeSave.catch(() => {}).then(() => {
    const s = S.settings;
    return saveSettings(s).then(() => void (savedAppearance = s.appearance));
  }));
  try {
    await save;
  } catch (e) {
    void info(`保存外观失败 kind=${(e as AppError).kind}`);
    // 等待期间又点了别的：交给后一次
    if (S.settings.appearance !== mode) return;
    S.settings = { ...S.settings, appearance: savedAppearance };
    setTheme(S.settings.appearance);
    toast('外观未能保存');
  }
}
themeSeg.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('[data-theme-opt]');
  if (b) void applyAppearance(b.dataset.themeOpt as Appearance);
});
themeSeg.addEventListener('keydown', (e) => {
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  e.preventDefault();
  e.stopPropagation();
  // 按住不放：只切一次，不每次重复都保存
  if (e.repeat) return;
  const i = themeOpts.findIndex((b) => b.dataset.themeOpt === themeMode());
  const n = themeOpts[(i + (e.key === 'ArrowRight' ? 1 : -1) + themeOpts.length) % themeOpts.length];
  void applyAppearance(n.dataset.themeOpt as Appearance);
  n.focus();
});

const setHotkeyHelp = (msg: string, err = false) => {
  hotkeyHelp.textContent = msg;
  hotkeyHelp.className = err ? 'err' : 'help';
  hotkeyIn.setAttribute('aria-invalid', String(err));
};
async function paintPerm() {
  permRow.hidden = S.platform.os !== 'macos';
  if (permRow.hidden) return;
  const ok = await accessibilityGranted().catch(() => false);
  permState.textContent = ok ? '已授予' : '未授予';
  permBtn.hidden = ok;
}
function openSettings() {
  // bootstrap 之前没有设置可填；已打开时（托盘再点“设置…”）不重填，保留正在改的内容
  if (!S.settings.hotkey || dlg.classList.contains('is-shown')) return;
  clearTimeout(closeTimer);
  if (!dlg.open) dlgReturn = document.activeElement as HTMLElement | null;
  clearNote.textContent = '置顶记录会保留';
  const st = S.settings;
  pendingHotkey = st.hotkey;
  hotkeyIn.value = hotkeyLabel(st.hotkey, S.platform.os);
  setHotkeyHelp(HOTKEY_IDLE);
  retainIn.value = optionFromRetention(st.retentionDays);
  limitIn.value = String(st.maxRecords);
  limitErr.hidden = true;
  limitIn.removeAttribute('aria-invalid');
  frequentIn.value = String(st.frequentMinUses);
  frequentErr.hidden = true;
  frequentIn.removeAttribute('aria-invalid');
  draftIgnored = [...st.ignoredApps];
  ignoreIn.value = '';
  paintChips();
  paintAppList(false);
  // 每次打开都重取（期间可能装了新应用）；取不到只记日志，照样能手输
  void installedApps().then(
    (a) => {
      allApps = a;
      if (document.activeElement === ignoreIn) paintAppList(true);
    },
    (e: AppError) => void info(`读取应用列表失败 kind=${e.kind}`),
  );
  sensitiveIn.checked = st.skipSensitive;
  moveTopIn.checked = st.moveToTop;
  autostartIn.checked = st.launchAtLogin;
  paintTheme(true);
  void paintPerm();
  if (!dlg.open) dlg.showModal();
  syncDialogOpen();
  // 关闭时 display:none 不会清掉滚动位置，打开后再回到顶部
  dlg.querySelector('.dlg-body')!.scrollTop = 0;
  void dlg.offsetWidth;
  dlg.classList.add('is-shown');
  hotkeyIn.focus();
}
function closeSettings() {
  dlg.classList.remove('is-shown');
  clearTimeout(closeTimer);
  closeTimer = window.setTimeout(() => dlg.close(), reduced.matches ? 0 : 150);
}
// 所有关闭路径（含面板隐藏时直接 close）都到这里：清掉弹入状态、焦点回到打开前的元素
dlg.addEventListener('close', () => {
  dlg.classList.remove('is-shown');
  if (S.shown && dlgReturn?.isConnected) dlgReturn.focus({ preventScroll: true });
  dlgReturn = null;
});
dlg.addEventListener('cancel', (e) => {
  e.preventDefault();
  if (!escClosedList) closeSettings();
});
// 遮罩点击才关：在输入框里拖选文字、松手落到遮罩上不算（那次 click 的目标也是 dialog）
let downOnBackdrop = false;
dlg.addEventListener('pointerdown', (e) => (downOnBackdrop = e.target === dlg));
dlg.addEventListener('click', (e) => {
  if ((e.target as Element).closest('[data-close]') || (e.target === dlg && downOnBackdrop)) closeSettings();
});
hotkeyIn.addEventListener('focus', () => setHotkeyHelp('请按下组合键，至少包含一个修饰键…'));
hotkeyIn.addEventListener('blur', () => setHotkeyHelp(HOTKEY_IDLE));
hotkeyIn.addEventListener('keydown', (e) => {
  if (e.key === 'Tab' || e.key === 'Escape') return;
  e.preventDefault();
  // 按住不放的重复事件不重播弹跳与提示
  if (e.repeat || ['Control', 'Meta', 'Shift', 'Alt'].includes(e.key)) return;
  const h = hotkeyFromEvent({ key: e.key, code: e.code, mod: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey, repeat: e.repeat });
  if (!h) return setHotkeyHelp(`至少需要包含 ${MOD} 或 Alt`);
  recordHotkey(h);
});
function recordHotkey(h: string) {
  pendingHotkey = h;
  hotkeyIn.value = hotkeyLabel(h, S.platform.os);
  setHotkeyHelp('已记录，保存后生效');
  fx(hotkeyIn, [{ transform: 'scale(.98)' }, { transform: 'scale(1)' }], { duration: 300, easing: EASE_SPRING });
}
// 授权要切去系统设置：设置框开着时失焦不隐藏，这里主动收起，免得面板盖住系统设置
permBtn.addEventListener('click', () => void requestAccessibility().then(hide, (err: AppError) => toast(err.message)));
form.addEventListener('submit', (e) => {
  e.preventDefault();
  void submitSettings();
});
async function submitSettings() {
  if (saving) return;
  const limit = parseLimit(limitIn.value), frequent = parseFrequent(frequentIn.value);
  limitErr.hidden = limit !== null;
  limitIn.setAttribute('aria-invalid', String(limit === null));
  frequentErr.hidden = frequent !== null;
  frequentIn.setAttribute('aria-invalid', String(frequent === null));
  if (limit === null) return limitIn.focus();
  if (frequent === null) return frequentIn.focus();
  const s = formToSettings(S.settings, {
    hotkey: pendingHotkey, retain: retainIn.value, limit, frequent, ignored: addIgnored(draftIgnored, ignoreIn.value), skipSensitive: sensitiveIn.checked,
    moveToTop: moveTopIn.checked, launchAtLogin: autostartIn.checked,
  });
  saving = true;
  try {
    // 新的保留时长 / 条数上限会立即清理记录：先告诉用户删几条（设计 §5.5）
    if (s.retentionDays !== S.settings.retentionDays || s.maxRecords !== S.settings.maxRecords) {
      const n = await prunePreview(s.retentionDays, s.maxRecords);
      if (n > 0 && !(await confirm({ title: '保存设置', body: `将删除 ${n} 条记录`, ok: '删除并保存' }))) return;
    }
    await saveSettings(s);
    const favChanged = s.frequentMinUses !== S.settings.frequentMinUses;
    S.settings = s;
    // 常用阈值只在前端用：立即按新阈值重算分类计数、列表与预览标签
    if (favChanged) {
      renderTabs(true);
      // 先重绘频谱（常用分类下的变暗随阈值变），再 renderList，同 setFilter
      renderSpectrum();
      renderList();
    }
    closeSettings();
    toast('设置已保存');
  } catch (e) {
    const err = e as AppError;
    if (err.kind === 'hotkeyTaken') {
      // 先聚焦再写错误：focus 处理器会改帮助文字
      hotkeyIn.focus();
      setHotkeyHelp(err.message, true);
    } else toast(err.message);
  } finally {
    saving = false;
  }
}
$('#settingsBtn').addEventListener('click', openSettings);

// —— 首次启动引导（M18） ——
// 与设置对话框同一套弹入 / 150ms 收起。每步一个 .dlg-body，切步时新显示的那个重放 rowIn。
// 关闭按钮、Esc、“完成”都算结束（不再显示）；点遮罩不关，免得首次使用误点丢掉引导；
// 面板隐藏时直接 close 不算结束，下次唤起从当前步继续
const obDlg = $<HTMLDialogElement>('#onboard'), obNext = $<HTMLButtonElement>('#obNext'), obSkip = $<HTMLButtonElement>('#obSkip');
const obDots = $('#obDots'), obAuto = $<HTMLInputElement>('#obAutostart');
let obSteps: Step[] = [], obStep: Step | null = null, obOk = false, obPoll = 0, obTimer = 0, obReturn: HTMLElement | null = null;
const obSection = () => obDlg.querySelector<HTMLElement>(`[data-step="${obStep}"]`)!;
// 开机启动步没有成功态
function paintObOk(pop: boolean) {
  const sec = obSection(), ok = sec.querySelector<HTMLElement>('[data-ok]');
  if (!ok) return;
  sec.querySelector<HTMLElement>('[data-try]')!.hidden = obOk;
  ok.hidden = !obOk;
  if (!pop || !obOk) return;
  fx(ok, [{ opacity: 0 }, { opacity: 1 }], { duration: 200, easing: 'ease-out' });
  fx(ok.querySelector('.ic'), [{ transform: 'scale(.3)' }, { transform: 'scale(1)' }], { duration: 420, easing: EASE_SPRING });
}
function obSucceed() {
  if (obOk) return;
  obOk = true;
  clearInterval(obPoll);
  void paintVerb();
  paintObOk(true);
  obNext.disabled = false;
  obNext.focus();
}
async function pollAx() {
  if (await accessibilityGranted().catch(() => false)) {
    if (obStep === 'accessibility' && obDlg.open) obSucceed();
  }
}
function paintOb() {
  const step = obStep!, last = nextStep(obSteps, step) === null;
  obDlg.querySelectorAll<HTMLElement>('[data-step]').forEach((el) => (el.hidden = el.dataset.step !== step));
  obDots.querySelectorAll('i').forEach((d, i) => d.classList.toggle('on', obSteps[i] === step));
  obDots.setAttribute('aria-label', `第 ${obSteps.indexOf(step) + 1} 步，共 ${obSteps.length} 步`);
  obNext.textContent = last ? '完成' : '下一步';
  obNext.disabled = !last && !obOk;
  obSkip.hidden = last;
  obSkip.textContent = step === 'hotkey' ? '跳过这一步' : '跳过';
  paintObOk(false);
  clearInterval(obPoll);
  if (step === 'accessibility' && !obOk) {
    void pollAx();
    obPoll = window.setInterval(() => void pollAx(), 1000);
  }
  // 热键步没有可用的按钮：聚焦对话框本身，免得 Enter 落到关闭按钮上结束引导
  (step === 'accessibility' && !obOk ? $('#obGrant') : obNext.disabled ? obDlg : obNext).focus();
}
function startOnboarding() {
  obSteps = onboardSteps(S.platform.os);
  obStep = obSteps[0];
  obOk = false;
  obDots.innerHTML = obSteps.map(() => '<i></i>').join('');
  $('#obKey').textContent = hotkeyLabel(S.settings.hotkey, S.platform.os);
  obAuto.checked = S.settings.launchAtLogin;
}
function openOnboard() {
  if (!obStep || obDlg.classList.contains('is-shown')) return;
  clearTimeout(obTimer);
  if (!obDlg.open) {
    obReturn = document.activeElement as HTMLElement | null;
    obDlg.showModal();
  }
  void obDlg.offsetWidth;
  obDlg.classList.add('is-shown');
  paintOb();
}
function obAdvance(skip: boolean) {
  // 结束后的 150ms 收起途中还能点到按钮
  if (!obStep) return;
  const n = nextStep(obSteps, obStep);
  if (!n) return void finishOnboard();
  if (!skip && !obOk) return;
  obStep = n;
  obOk = false;
  paintOb();
}
async function finishOnboard() {
  if (!obStep) return;
  obStep = null;
  clearInterval(obPoll);
  obDlg.classList.remove('is-shown');
  clearTimeout(obTimer);
  obTimer = window.setTimeout(() => obDlg.close(), reduced.matches ? 0 : 150);
  const s = { ...S.settings, launchAtLogin: obAuto.checked };
  try {
    await saveSettings(s);
    S.settings = s;
  } catch (e) {
    void info(`引导：保存开机启动失败 kind=${(e as AppError).kind}`);
  }
  await finishOnboarding().catch((e: AppError) => void info(`引导：结束引导失败 kind=${e.kind}`));
}
obDlg.addEventListener('close', () => {
  if (obDlg.open) return;
  obDlg.classList.remove('is-shown');
  clearInterval(obPoll);
  if (S.shown && obReturn?.isConnected) obReturn.focus({ preventScroll: true });
  obReturn = null;
});
obDlg.addEventListener('cancel', (e) => {
  e.preventDefault();
  void finishOnboard();
});
obDlg.addEventListener('click', (e) => {
  const t = e.target as Element;
  if (t.closest('[data-close]')) void finishOnboard();
  else if (t.closest('#obNext')) obAdvance(false);
  else if (t.closest('#obSkip')) obAdvance(true);
  else if (t.closest('#obGrant')) void requestAccessibility().catch((err: AppError) => toast(err.message));
});

// —— 面板显隐 ——
let booted = false;
const shown = (t0: number | null, trigger?: PanelShown['trigger']) => {
  S.shown = true;
  // 授权可能在隐藏期间变化；bootstrap 之前平台未知，由 bootstrap 后那一次负责
  if (S.platform.os) void paintVerb();
  // 淡出途中又被唤起：取消待发的隐藏
  clearTimeout(hideTimer);
  // 键盘唤起几乎即时（90 / 120ms）；托盘单击是鼠标唤起，按原型 240 / 320ms 弹性回位
  win.toggleAttribute('data-instant', trigger !== 'tray');
  win.dataset.shown = 'true';
  const first = !booted;
  if (!booted) {
    // 首次唤起才播放 720ms 编排；之后的快捷键唤起只做 90ms 淡入
    booted = true;
    win.classList.add('boot');
    setTimeout(() => win.classList.remove('boot'), 1400);
  }
  q.value = '';
  q.focus();
  // 隐藏期间不刷新相对时间与分组，唤起时补一次；首次唤起逐行浮现（启动时窗口还不可见，不在那时播）
  // 空查询同步完成，runSearch 里已按当前宽度与时刻重绘频谱；这里只让游标直接落位、不滑
  runSearch('', first ? { stagger: true, base: 260 } : {});
  paintSpectrumSel(false);
  // 引导没结束就被隐藏过：从当前步继续
  openOnboard();
  if (t0 !== null) {
    timing = true;
    requestAnimationFrame(() => void info(`唤起耗时 ${Date.now() - t0} ms`));
  }
};
// 一次隐藏可能收到两次 panel-hidden，这里只写状态，重复调用无副作用
const hidden = () => {
  S.shown = false;
  win.dataset.shown = 'false';
  // 跨隐藏不复用详情（例如文件是否还在）
  bodyFor = null;
  details.clear();
  pbody.querySelectorAll('video').forEach((v) => v.pause());
  pbody.innerHTML = '';
  closeMenu(false);
  // 对话框不播收起动画，直接关（各自的 close 事件里交还焦点、结算确认框并同步后端的对话框标志）
  document.querySelectorAll<HTMLDialogElement>('dialog[open]').forEach((d) => d.close());
  (document.activeElement as HTMLElement | null)?.blur();
};

// 键盘在 bootstrap 之前接线：读取失败的错误页也能按 Esc 收起
// 前端发起的隐藏先播 160ms 淡出；失焦隐藏由后端直接做，没有淡出。
// macOS 系统材质的底板不归网页画：内容只位移（data-native-fade），立即调 hide_panel，后端在 160ms 内把整个窗口 alpha 淡到 0；
// 唤起时后端同样把 alpha 从 0 淡入（90ms，托盘 240ms）
let hideTimer = 0;
const nativeFade = () => S.platform.os === 'macos' && S.platform.material === 'glass';
const hide = () => {
  // 首次唤起编排的动画会压住淡出（原型 setPanel 同样先去掉 boot）
  win.classList.remove('boot');
  win.dataset.shown = 'false';
  clearTimeout(hideTimer);
  const animate = !reduced.matches;
  hideTimer = window.setTimeout(() => void hidePanel(animate).catch((err: AppError) => {
    win.dataset.shown = 'true';
    toast(err.message);
  }), animate && !nativeFade() ? 160 : 0);
};

// —— 键盘 ——
document.addEventListener('keydown', (e) => {
  if (e.isComposing) return;
  const t = e.target as Element, inSearch = t === q;
  // 其他输入控件（设置里的快捷键、保留时长）自己处理按键
  if (!inSearch && t.closest?.('input, textarea, select')) return;
  const a = keyAction(
    { key: e.key, code: e.code, mod: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey, repeat: e.repeat },
    {
      inSearch,
      onControl: !inSearch && !!t.closest?.('button, a, video'),
      searchHasText: !!q.value,
      menuOpen: menuOpen(),
      hasSel: S.sel !== null,
      canUndo: canUndo(S.undo, Date.now()),
      dialogOpen: !!document.querySelector('dialog[open]'),
    },
  );
  if (!a) return;
  // 让这个字符照常输入到搜索框
  if (a.t === 'focusSearch') return q.focus();
  e.preventDefault();
  const sel = S.sel;
  switch (a.t) {
    case 'menuClose': return closeMenu(true);
    case 'menuMove': return moveMenuFocus(a.d);
    case 'move': return move(a.d, a.instant);
    case 'copy': return void (sel !== null && copyRecord(sel));
    case 'copyIndex': {
      const r = vis()[a.i];
      if (r && !S.removing.has(r.id)) {
        select(r.id);
        copyRecord(r.id);
      }
      return;
    }
    case 'pin': return void (sel !== null && togglePin(sel, false));
    case 'filter': return setFilter(cycleFilter(S.filter, a.d), true);
    case 'undo': return undo();
    case 'delete': return void (sel !== null && remove(sel));
    case 'clearSearch':
      q.value = '';
      return runSearch('');
    case 'hide': return hide();
    case 'settings': return openSettings();
    case 'menu': {
      // ponytail: 选中行滚出视口未渲染时不弹菜单；键盘移动总会把选中行滚回视口
      const r = rowEl(sel)?.getBoundingClientRect();
      if (r && sel !== null) openMenu(sel, r.left + 48, r.bottom, true);
      return;
    }
  }
});

await attachConsole();
// 先订阅再 bootstrap，避免两者之间的事件丢失
await onPanelShown(({ t0, trigger }) => shown(t0, trigger));
await onPanelHidden(hidden);
await onPanelHideRequested(hide);
await onRecordAdded((s) => handlers.added(s));
await onRecordUpdated((s) => handlers.updated(s));
await onRecordsRemoved((ids) => handlers.removed(ids));
await onPausedChanged((p) => handlers.paused(p));
// 托盘“设置…”：后端已先唤起面板
await onOpenSettings(openSettings);
// 设置对话框开着时提示条被它盖住，结果同样写进“历史记录”行内
await onConfirmClear(() => void confirmClear().then((m) => m && dlg.open && (clearNote.textContent = m)));
// 引导中按唤起快捷键：可见时面板不隐藏，隐藏时先 panel-shown 再到这里；只在“试按快捷键”这一步算成功
// 设置对话框打开时按唤起快捷键：面板同样不隐藏；录制框有焦点就当作按下了当前快捷键（网页收不到被全局注册拦下的按键）
await onHotkeyPressed(() => {
  // 只认面板有焦点时的按键：失焦时后端只把面板拉回焦点，这里再防一道
  if (document.activeElement === hotkeyIn && document.hasFocus()) return recordHotkey(S.settings.hotkey);
  if (obStep === 'hotkey' && obDlg.open) obSucceed();
});

// 页面重载时对话框都已关闭：先复位后端标志（重载前若有对话框开着，它会停在 true；读取失败也照样复位）
syncDialogOpen();
const b = await bootstrap().catch((e: AppError) => {
  // 之后的每次 renderList 都显示这条，不被空列表文案覆盖
  bootError = e?.message || String(e);
  renderList();
  throw e;
});
S.settings = b.settings;
savedAppearance = b.settings.appearance;
paintSpCollapsed(b.settings.spectrumCollapsed);
S.platform = b.platform;
S.paused = b.paused;
paintPause();
// 订阅后到达的事件比快照新，快照不覆盖已有条目
// ponytail: 快照之后、返回之前被删除的记录会被快照带回，下次 records-removed 或重启时消失
for (const r of b.records) if (!S.records.has(r.id)) S.records.set(r.id, r);
S.filter = FILTERS.some(([k]) => k === b.settings.lastFilter) ? (b.settings.lastFilter as Filter) : 'all';
if (b.settings.appearance === 'system') delete root.dataset.theme;
else root.dataset.theme = b.settings.appearance;
root.dataset.material = b.platform.material;
// 原生淡出时网页内容不再自己淡出（否则内容先消失、只剩底板），整体由窗口 alpha 承担
root.toggleAttribute('data-native-fade', nativeFade());
const MOD = b.platform.os === 'macos' ? '⌘' : 'Ctrl';
document.querySelectorAll('.mod').forEach((k) => (k.textContent = MOD));
$('.modp').textContent = `${MOD} P`;
// macOS 没有 ⌦ 键（MacBook），删除提示写 ⌘ ⌫：状态栏与邻项一样拆成两枚键帽，菜单里是一枚
const mac = b.platform.os === 'macos';
$('.statusbar .del').textContent = mac ? '⌫' : 'Del';
$('.statusbar .delmod').hidden = !mac;
$('#menu .del').textContent = mac ? '⌘ ⌫' : 'Del';
void paintVerb();
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);
// 首次启动后端在 setup 里就显示了面板，那时页面还没订阅，panel-shown 已丢失，这里补上
if (b.firstRun) {
  startOnboarding();
  // shown() 末尾会打开引导；panel-shown 已先到时直接打开
  if (S.shown) openOnboard();
  else shown(null);
}

// —— 事件 ——
// 按钮在标题行里，点击冒泡到这一个监听，不会切两次
$('.sp-head').addEventListener('click', () => {
  const c = !spCollapsed();
  paintSpCollapsed(c);
  persistUi({ spectrumCollapsed: c });
  // 收起期间没画光柱：展开后立即按当前宽度画（读 clientWidth 会同步排版，轨道已有真实宽度，不闪一帧旧光柱）；
  // renderSpectrum 末尾会重新标记选中光柱与游标
  // 先清空光柱走整组重建：原地改 .dim / .on 会在显示时播 200ms 过渡，违背即时切换
  if (!c) {
    spBars.replaceChildren();
    renderSpectrum();
  }
});
q.addEventListener('input', (e) => {
  if ((e as InputEvent).isComposing) return;
  runSearch(q.value, { timed: true });
});
q.addEventListener('compositionend', () => runSearch(q.value, { timed: true }));
clearQ.addEventListener('click', () => {
  q.value = '';
  runSearch('');
  q.focus();
});
tabsBase.addEventListener('click', (e) => {
  const t = (e.target as Element).closest<HTMLElement>('[data-f]');
  if (t) setFilter(t.dataset.f as Filter, e.detail === 0);
});
list.addEventListener('scroll', () => {
  schedulePaint();
  closeMenu(false);
}, { passive: true });
// 行不抢焦点，键盘始终留在搜索框
list.addEventListener('mousedown', (e) => {
  if ((e.target as Element).closest('.row')) e.preventDefault();
});
const itemOf = (e: Event) => {
  const it = (e.target as Element).closest<HTMLElement>('.item');
  const id = Number(it?.dataset.id);
  return it && !S.removing.has(id) ? id : null;
};
list.addEventListener('click', (e) => {
  const t = e.target as Element;
  if (t.closest('[data-act="reset"]')) {
    q.value = '';
    S.filter = 'all';
    persistFilter();
    renderTabs(false);
    runSearch('', { stagger: true });
    q.focus();
    return;
  }
  const id = itemOf(e);
  if (id === null) return;
  const qa = t.closest<HTMLElement>('[data-qa]')?.dataset.qa;
  select(id, { indDur: 180 });
  if (qa === 'copy') copyRecord(id);
  if (qa === 'pin') togglePin(id, true);
});
list.addEventListener('dblclick', (e) => {
  const id = itemOf(e);
  if (id !== null && !(e.target as Element).closest('[data-qa]')) copyRecord(id);
});
list.addEventListener('contextmenu', (e) => {
  const id = itemOf(e);
  if (id === null) return;
  e.preventDefault();
  select(id, { indDur: 180 });
  openMenu(id, e.clientX, e.clientY, false);
});

pinBtn.addEventListener('click', (e) => S.sel !== null && togglePin(S.sel, e.detail !== 0));
copyBtn.addEventListener('click', () => S.sel !== null && copyRecord(S.sel));
// 设置对话框留在确认框下面；列表与频谱由 records-removed 刷新
$('#clearBtn').addEventListener('click', () => void confirmClear().then((m) => m && (clearNote.textContent = m)));
delBtn.addEventListener('click', () => S.sel !== null && remove(S.sel));
menu.addEventListener('click', (e) => {
  if (!menuOpen()) return;
  const m = (e.target as Element).closest<HTMLElement>('[data-m]')?.dataset.m;
  const id = S.menuFor, mouse = e.detail !== 0;
  if (!m || id === null) return;
  closeMenu(true);
  if (m === 'copy') copyRecord(id);
  if (m === 'pin') togglePin(id, mouse);
  if (m === 'open') openFromMenu(id);
  if (m === 'del') remove(id);
});
document.addEventListener('pointerdown', (e) => {
  const t = e.target as Node;
  if (menuOpen() && !menu.contains(t)) closeMenu(false);
}, true);
undoBtn.addEventListener('click', undo);
toastAct.addEventListener('click', () => {
  toastRun?.();
  toastRun = null;
  toastEl.dataset.open = 'false';
  clearTimeout(toastTimer);
});
pauseBtn.addEventListener('click', () => setPaused(!S.paused).catch((err: AppError) => toast(err.message)));

pbody.addEventListener('click', (e) => {
  const t = e.target as Element;
  const a = t.closest('a');
  if (a) {
    // 链接只交给系统浏览器打开，WebView 不导航
    e.preventDefault();
    const u = a.getAttribute('href');
    if (u && isHttpUrl(u)) openUrl(u).catch((err: AppError) => toast(err.message));
    return;
  }
  const rv = t.closest<HTMLElement>('[data-reveal]');
  if (rv) {
    const f = S.detail?.files[Number(rv.dataset.reveal)];
    // 后端的 fileMissing 文案是“无法粘贴”，这里按 kind 给自己的文案
    if (f) revealFile(f.path).catch((err: AppError) => toast(err.kind === 'fileMissing' ? MISSING : err.message));
    return;
  }
  const play = t.closest<HTMLElement>('[data-play]');
  if (play) {
    const d = S.detail, v = d?.video;
    if (!d || !v?.src) return;
    const wh = v.w && v.h ? ` width="${v.w}" height="${v.h}"` : '';
    play.insertAdjacentHTML('afterend', `<video src="${esc(v.src)}" controls playsinline preload="auto" aria-label="${esc(d.title)}"${wh}></video>`);
    const vid = play.nextElementSibling as HTMLVideoElement;
    play.remove();
    // 容器认得但解码不了（例如 mov 里的 ProRes）：换回封面并说明；已切走的旧正文不管
    vid.addEventListener('error', () => {
      const fig = vid.closest('figure');
      if (!vid.isConnected || !fig) return;
      const had = document.activeElement === vid;
      fig.outerHTML = videoHTML(d, true);
      // 焦点随 <video> 一起消失会落到 body：交还搜索框
      if (had) q.focus({ preventScroll: true });
    }, { once: true });
    // WebKit 在这里不会用空格切换播放；Enter 由 keymap 让给 video，不复制
    vid.addEventListener('keydown', (e) => {
      if (e.key !== ' ') return;
      e.preventDefault();
      // 按住空格不反复切换
      if (e.repeat) return;
      if (vid.paused) vid.play().catch(() => {});
      else vid.pause();
    });
    // 同步调用，仍在用户手势内；被拒绝（自动播放策略等）时留给用户点控件
    vid.play().catch(() => {});
    vid.focus();
    return;
  }
});

// 非行区域不弹 WebView 原生菜单（含“重新加载”）；输入框保留系统的编辑菜单
win.addEventListener('contextmenu', (e) => {
  if (!(e.target as Element).closest('input, textarea')) e.preventDefault();
});
// 描边跟随指针发光：只改 .glow 自己的变量，rAF 合并
if (matchMedia('(hover: hover) and (pointer: fine)').matches) {
  const glow = $('#glow');
  let glowRaf = 0, gx = 0, gy = 0;
  win.addEventListener('pointermove', (e) => {
    gx = e.clientX;
    gy = e.clientY;
    if (glowRaf) return;
    glowRaf = requestAnimationFrame(() => {
      glowRaf = 0;
      const r = win.getBoundingClientRect();
      glow.style.setProperty('--mx', gx - r.left + 'px');
      glow.style.setProperty('--my', gy - r.top + 'px');
    });
  });
}
// 跟随系统切深浅色时整体一次切换，不让各元素按各自的过渡渐变（M16 规则 5）
matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
  if (root.dataset.theme) return;
  root.setAttribute('data-theme-switching', '');
  // 先让“关过渡”生效，再由新的配色 token 重算样式
  void root.offsetWidth;
  requestAnimationFrame(() => root.removeAttribute('data-theme-switching'));
});
// 列数随轨道宽度变，一帧内的多次 resize 只重排一次频谱
let spRaf = 0;
addEventListener('resize', () => {
  renderTabs(true);
  paintRows();
  paintSpectrumSel(false);
  moveInd({ instant: true });
  if (!spRaf) spRaf = requestAnimationFrame(() => {
    spRaf = 0;
    renderSpectrum();
  });
});
setInterval(() => {
  if (S.shown && !menuOpen() && !S.removing.size) {
    // 跨午夜时分组标题也要更新；选中项不变时 renderPreview 不重取详情（bodyFor）
    renderList();
    renderSpectrum();
  }
}, 60e3);

renderTabs(true);
renderSpectrum();
renderList();
void document.fonts?.ready.then(() => {
  renderTabs(true);
  moveInd({ instant: true });
  paintSpectrumSel(false);
});
