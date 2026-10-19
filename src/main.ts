import './styles.css';
import { attachConsole, info } from './api/log';
import {
  bootstrap, copy, copyText, deleteRecord, getRecord, hidePanel, openUrl, revealFile, search, setPaused, setPinned, undoDelete,
} from './api/commands';
import {
  onPanelHidden,
  onPanelShown,
  onPausedChanged,
  onRecordAdded,
  onRecordsRemoved,
  onRecordUpdated,
} from './api/events';
import type { AppError, PlatformInfo, RecordDetail, RecordSummary, Settings } from './api/types';
import { grade, hexInfo, isHex } from './color';
import { copyToast } from './copy-status';
import {
  esc, FAV_MIN, FILTERS, FL, fmtBytes, fmtDateTime, fmtDur, fmtInt, hl, hlCode, host, isFav, isHttpUrl, LABEL, rel, truncate,
  type Filter,
} from './format';
import { keyAction } from './keymap';
import { applyAdded, applyRemoved, applyUpdated } from './records';
import {
  armUndo, canUndo, counts, cycleFilter, ensureSelection, groups, inFilter, isStale, nextSelection, UNDO_MS, visible, type UndoState,
} from './view';
import { layout, range, ROW_H, scrollTo, topOf, type Cell } from './virtual-list';

const I = {
  text: '<path d="M4 5.5h12M4 10h12M4 14.5h7"/>',
  link: '<path d="M8.5 11.5a3 3 0 0 0 4.2 0l2.6-2.6a3 3 0 0 0-4.2-4.2l-.9.9"/><path d="M11.5 8.5a3 3 0 0 0-4.2 0l-2.6 2.6a3 3 0 0 0 4.2 4.2l.9-.9"/>',
  code: '<path d="M7 6 3 10l4 4M13 6l4 4-4 4"/>',
  color: '<path d="M10 3.5c3 3.4 5 6 5 8.3a5 5 0 0 1-10 0c0-2.3 2-4.9 5-8.3z"/>',
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
  more: '<circle cx="5" cy="10" r="1.3" fill="currentColor" stroke="none"/><circle cx="10" cy="10" r="1.3" fill="currentColor" stroke="none"/><circle cx="15" cy="10" r="1.3" fill="currentColor" stroke="none"/>',
  min: '<path d="M5 10h10"/>',
  x: '<path d="m5.5 5.5 9 9M14.5 5.5l-9 9"/>',
  open: '<path d="M11 4h5v5M16 4l-7 7M14 11.5V16H4V6h4.5"/>',
  plus: '<path d="M10 4v12M4 10h12"/>',
  spark: '<path d="M10 3v3M10 14v3M3 10h3M14 10h3M5.5 5.5l2 2M12.5 12.5l2 2M14.5 5.5l-2 2M7.5 12.5l-2 2"/>',
  inbox: '<path d="M3 11h4l1 2h4l1-2h4"/><path d="M5 5h10l2 6v4H3v-4z"/>',
  sun: '<circle cx="10" cy="10" r="3.2"/><path d="M10 2.5v1.8M10 15.7v1.8M2.5 10h1.8M15.7 10h1.8M4.7 4.7 6 6M14 14l1.3 1.3M15.3 4.7 14 6M6 14l-1.3 1.3"/>',
  moon: '<path d="M15.5 12.2A6 6 0 0 1 7.8 4.5a6 6 0 1 0 7.7 7.7z"/>',
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
const nf2 = new Intl.NumberFormat('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const hf = new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false });
const DAY = 864e5;

// —— 元素 ——
const win = $('#win'), list = $('#list'), rowsEl = $('#rows'), ind = $('#ind'), q = $<HTMLInputElement>('#q'), clearQ = $('#clearQ');
const tabs = $('#tabs'), tabsBase = $('#tabsBase'), tabsOver = $('#tabsOver');
const ptype = $('#ptype'), pbody = $('#pbody'), pmeta = $('#pmeta'), countEl = $('#count');
const pinBtn = $<HTMLButtonElement>('#pinBtn'), copyBtn = $<HTMLButtonElement>('#copyBtn');
const moreBtn = $<HTMLButtonElement>('#moreBtn'), delBtn = $<HTMLButtonElement>('#delBtn');
const menu = $('#menu'), toastEl = $('#toast'), toastMsg = $('#toastMsg'), undoBtn = $<HTMLButtonElement>('#undoBtn'), pauseBtn = $('#pauseBtn');
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
  cells: [] as Cell[],
  total: 0,
};
const menuOpen = () => menu.dataset.open === 'true';

// —— 数据视图 ——
const vis = () => visible(S.records, S.filter, S.matchIds);
const matches = (r: RecordSummary) => inFilter(r, S.filter) && (S.matchIds === null || S.matchIds.has(r.id));
const rowEl = (id: number | null) => (id === null ? null : document.getElementById('opt-' + id));
const source = (r: RecordSummary) => r.app?.name || '未知来源';

const thumb = (r: RecordSummary) =>
  r.kind === 'color' && isHex(r.title) ? `<span class="thumb swatch-thumb" style="background:${esc(r.title)}"></span>`
  : r.kind === 'image' && r.thumb ? `<span class="thumb"><img src="${esc(r.thumb)}" alt="" width="36" height="36"></span>`
  : r.kind === 'video' && r.thumb ? `<span class="thumb"><img src="${esc(r.thumb)}" alt="" width="36" height="36"><span class="play">${svg('play')}</span></span>`
  : `<span class="thumb">${svg(r.kind)}</span>`;

const metaLine = (r: RecordSummary) => {
  const parts = [esc(source(r)), rel(r.copiedAt), r.kind === 'video' && r.durationSec !== null ? fmtDur(r.durationSec) : fmtBytes(r.size)];
  if (isFav(r)) parts.push(`<span class="uses">用过 ${fmtInt(r.uses)} 次</span>`);
  return parts.join(' · ');
};

const rowHTML = (r: RecordSummary, i: number, stagger: boolean, isNew = r.id === S.newId) => `
    <div class="item${isNew ? ' is-new' : ''}${S.removing.has(r.id) ? ' is-removing' : ''}${stagger ? ' stg' : ''}" data-id="${r.id}" style="--i:${i}">
      <div class="row" role="option" id="opt-${r.id}" aria-selected="false" data-type="${r.kind}">
        <div class="row-in">
          ${thumb(r)}
          <div class="main">
            <div class="title${r.kind === 'code' || r.kind === 'color' ? ' mono' : ''}">${hl(r.title, S.q)}</div>
            <div class="meta">${metaLine(r)}</div>
          </div>
          <div class="tail">
            ${r.pinned ? `<span class="pin-mark">${svg('pinFill')}</span>` : ''}${i < 9 ? `<kbd class="idx">${i + 1}</kbd>` : ''}
            <span class="qa" aria-hidden="true">
              <button type="button" tabindex="-1" data-qa="pin" title="${r.pinned ? '取消置顶' : '置顶'}">${svg(r.pinned ? 'pinFill' : 'pin')}</button>
              <button type="button" tabindex="-1" data-qa="copy" title="复制">${svg('copy')}</button>
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

function emptyHTML() {
  const qq = S.q.trim(), f = S.filter;
  const msg = qq ? `没有匹配“${esc(qq)}”的${f === 'all' ? '' : FL[f]}记录`
    : f === 'fav' ? `还没有常用记录。同一条内容复制 ${FAV_MIN} 次以上就会出现在这里`
    : `还没有${f === 'all' ? '' : FL[f]}记录，复制一次就会出现在这里`;
  return `<div class="empty"><span class="ring">${svg(qq ? 'search' : f === 'fav' ? 'star' : 'inbox')}</span><p>${msg}</p>
      ${qq || f !== 'all' ? '<button class="btn sm" type="button" data-act="reset">清除搜索与筛选</button>' : ''}</div>`;
}

// —— 虚拟列表 ——
// 只渲染 range() 内的 cell；按 key 复用元素，HTML 不变时只改位置，悬停与动画类不闪。
// 一次性的入场类（stg、is-new）不计入比较，之后的重绘不会打断动画
let painted = new Map<string, { el: HTMLElement; h: string }>();
// cell 的 top 相对 .list-inner，滚动容器还有上内边距
const padTop = () => parseFloat(getComputedStyle(list).paddingTop) || 0;
function paintRows(stagger = false) {
  const [a, b] = range(S.cells, list.scrollTop - padTop(), list.clientHeight);
  const next = new Map<string, { el: HTMLElement; h: string }>();
  for (let i = a; i < b; i++) {
    const c = S.cells[i];
    const r = c.kind === 'row' ? S.records.get(c.id) : undefined;
    if (c.kind === 'row' && !r) continue;
    const key = c.kind === 'head' ? 'h' + c.name : 'r' + c.id;
    const h = r && c.kind === 'row' ? rowHTML(r, c.index, false, false)
      : c.kind === 'head' ? `<div class="ghead" id="gh${c.gi}">${esc(c.name)}<span>${c.count}</span></div>` : '';
    let p = painted.get(key);
    if (!p) {
      const el = document.createElement('div');
      el.className = 'vrow';
      p = { el, h: '' };
    }
    if (r && c.kind === 'row' && (stagger || r.id === S.newId)) p.el.innerHTML = rowHTML(r, c.index, stagger);
    else if (p.h !== h) p.el.innerHTML = h;
    p.h = h;
    p.el.style.transform = `translateY(${c.top}px)`;
    // ponytail: 新元素追加在末尾，DOM 顺序不严格等于视觉顺序；行不可聚焦，读屏走 aria-activedescendant
    if (!p.el.isConnected) rowsEl.append(p.el);
    next.set(key, p);
  }
  for (const [k, p] of painted) if (!next.has(k)) p.el.remove();
  painted = next;
  markSel();
}

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

function renderList({ scroll = false, stagger = false, base = 0 } = {}) {
  const v = vis();
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
  paintRows(stagger);
  S.sel = ensureSelection(v, S.sel);
  select(S.sel, { scroll, glide: false, fade: false });
}

// 选中光标：独立一层滑动，内容即时
let indShown = false;
function moveInd({ instant = false, dur = 130, top }: { instant?: boolean; dur?: number; top?: number } = {}) {
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  const t = top ?? (r ? topOf(S.cells, r.id) : undefined);
  if (!r || t === undefined) {
    ind.style.opacity = '0';
    indShown = false;
    return;
  }
  ind.dataset.type = r.kind;
  const d = instant || !indShown || reduced.matches ? 0 : dur;
  ind.style.transition = `transform ${d}ms ${EASE_MOVE}, opacity 160ms ease`;
  ind.style.transform = `translateY(${t}px)`;
  ind.style.opacity = '1';
  indShown = true;
}

type SelectOpts = { scroll?: boolean; glide?: boolean; instant?: boolean; fade?: boolean; indTop?: number; indDur?: number };
function select(id: number | null, { scroll = false, glide = true, instant = false, fade = true, indTop, indDur }: SelectOpts = {}) {
  const changed = S.sel !== id;
  S.sel = id;
  const top = id === null ? undefined : topOf(S.cells, id);
  if (scroll && top !== undefined) {
    const st = scrollTo(top + padTop(), ROW_H, list.scrollTop, list.clientHeight);
    // 先同步滚动并重绘，保证 aria-activedescendant 指向的行已在 DOM 中
    if (st !== list.scrollTop) {
      list.scrollTop = st;
      paintRows();
    }
  }
  markSel();
  moveInd({ instant: !glide || instant, top: indTop, dur: indDur });
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
const gradeHTML = (c: number) => {
  const g = grade(c);
  return `<em class="${g === '不达标' ? 'fail' : 'pass'}">${g}</em>`;
};
const MISSING = '原文件已移动或删除';

function bodyHTML(d: RecordDetail): string {
  const text = d.text ?? '';
  const trunc = d.truncated ? '<p class="trunc">仅显示前 200 KB，复制时为完整内容</p>' : '';
  switch (d.kind) {
    case 'text':
      return `<div class="p-text">${esc(text)}</div>${trunc}`;
    case 'link': {
      const h = host(text);
      const href = isHttpUrl(text) ? ` href="${esc(text)}" target="_blank" rel="noopener noreferrer"` : '';
      return `<div class="p-link"><div class="lk-card"><span class="lk-fav">${esc((h.replace(/^www\./, '')[0] || '?').toUpperCase())}</span><div><div class="lk-title">${esc(d.linkTitle || h)}</div><div class="lk-host">${esc(h)}</div></div></div>
          <a${href}>${esc(text)} ${svg('open')}</a>
          <div class="chips">${text.startsWith('https:') ? '<span class="chip ok">HTTPS</span>' : '<span class="chip">HTTP</span>'}<span class="chip">${esc(h)}</span></div></div>`;
    }
    case 'code':
      return `<pre class="p-code" translate="no"><code>${text.split('\n').map((l) => `<span class="ln">${hlCode(l) || ' '}</span>`).join('')}</code></pre>${trunc}`;
    case 'color': {
      if (!isHex(text)) return `<div class="p-text">${esc(text)}</div>`;
      const c = hexInfo(text);
      const ink = c.onBlack > c.onWhite ? '#06080F' : '#FFFFFF';
      return `<div class="p-color"><div class="swatch" style="background:${c.hex};--sw:${c.hex};color:${ink}"><b>${c.hex}</b></div>
          <ul class="cvals">${[['HEX', c.hex], ['RGB', c.rgb], ['HSL', c.hsl]].map(([k, v]) =>
            `<li><span class="k">${k}</span><code>${esc(v)}</code><button class="btn sm" type="button" data-copy="${esc(v)}">复制</button></li>`).join('')}</ul>
          <div class="contrast"><div>放在白色背景上<b>${nf2.format(c.onWhite)} : 1${gradeHTML(c.onWhite)}</b></div><div>放在黑色背景上<b>${nf2.format(c.onBlack)} : 1${gradeHTML(c.onBlack)}</b></div></div></div>`;
    }
    case 'image': {
      const im = d.image;
      if (!im) return `<div class="p-empty">${MISSING}</div>`;
      const wh = im.w && im.h ? ` width="${im.w}" height="${im.h}"` : '';
      return `<figure class="p-img"><img src="${esc(im.src)}"${wh} alt="${esc(d.title)}"></figure>`;
    }
    case 'video':
    case 'file': {
      const reveal = S.platform.os === 'macos' ? '在访达中显示' : '在资源管理器中显示';
      // 视频本阶段不内联播放：类型图标作封面占位，列出文件名与路径
      return d.files.map((f, i) => `<div class="p-file"><span class="big">${svg(d.kind)}</span><div><div class="fname">${esc(f.name)}</div><code class="fpath">${esc(f.path)}</code>
          <div class="chips">${f.ext ? `<span class="chip">${esc(f.ext.toUpperCase())}</span>` : ''}${d.kind === 'video' && d.durationSec !== null ? `<span class="chip">${fmtDur(d.durationSec)}</span>` : ''}<span class="chip">${fmtBytes(f.size)}</span></div>
          ${d.kind === 'file' ? `<div><button class="btn sm" type="button" data-reveal="${i}"${f.exists ? '' : ' disabled'}>${reveal}</button></div>` : ''}
          ${f.exists ? '' : `<div class="fpath">${MISSING}</div>`}</div></div>`).join('');
    }
  }
}

function metaHTML(r: RecordSummary, d: RecordDetail | null) {
  const meta: [string, string][] = [['来源', esc(source(r))], ['复制于', fmtDateTime(r.copiedAt)], ['大小', fmtBytes(r.size)], ['使用', `${fmtInt(r.uses)} 次`]];
  if (r.kind === 'video' && r.durationSec !== null) meta.push(['时长', fmtDur(r.durationSec)]);
  if (d) {
    const t = d.text ?? '', more = d.truncated ? '+' : '';
    if (r.kind === 'text') meta.push(['字符', fmtInt([...t].length) + more]);
    if (r.kind === 'code') meta.push(['行数', fmtInt(t.split('\n').length) + more]);
    if (r.kind === 'image' && d.image?.w) meta.push(['尺寸', `${d.image.w} × ${d.image.h}`]);
    if (r.kind === 'link') meta.push(['域名', esc(host(t))]);
  }
  return meta.map(([k, v]) => `<div><dt>${k}</dt><dd>${v}</dd></div>`).join('');
}

// 摘要立即渲染；正文异步取详情，序号过期的结果丢弃。同一条只取一次（摘要更新只刷新头部与信息栏）
let detailSeq = 0, bodyFor: number | null = null;
function renderPreview(fade = false) {
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  [pinBtn, copyBtn, moreBtn, delBtn].forEach((b) => (b.disabled = !r));
  $('#selHint').textContent = r ? `已选中：${truncate(r.title, 18)}` : '没有选中的记录';
  if (!r) {
    ++detailSeq;
    bodyFor = null;
    S.detail = null;
    ptype.innerHTML = '';
    pmeta.innerHTML = '';
    pbody.innerHTML = '<div class="p-empty">在左侧选择一条记录查看完整内容</div>';
    return;
  }
  ptype.innerHTML = `<span class="tchip" data-type="${r.kind}">${svg(r.kind)}${LABEL[r.kind]}</span>`
    + (isFav(r) ? `<span class="tchip" data-type="fav">${svg('star')}常用</span>` : '')
    + (r.lang ? `<span class="chip">${esc(r.lang)}</span>` : '');
  setPin(r.pinned, true);
  pmeta.innerHTML = metaHTML(r, S.detail?.id === r.id ? S.detail : null);
  if (bodyFor === r.id) return;
  bodyFor = r.id;
  S.detail = null;
  const seq = ++detailSeq;
  getRecord(r.id).then(
    (d) => {
      if (isStale(seq, detailSeq)) return;
      S.detail = d;
      pbody.innerHTML = d ? bodyHTML(d) : '';
      pbody.scrollTop = 0;
      const cur = d && S.records.get(d.id);
      if (cur) pmeta.innerHTML = metaHTML(cur, d);
      if (fade) fx(pbody, [{ opacity: 0.35, transform: 'translateY(4px)', filter: 'blur(2px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }], { duration: 180, easing: EASE_ENTER });
    },
    (e: AppError) => {
      if (isStale(seq, detailSeq)) return;
      bodyFor = null;
      pbody.innerHTML = '<div class="p-empty">在左侧选择一条记录查看完整内容</div>';
      void info(`get_record 失败 id=${r.id} kind=${e.kind}`);
    },
  );
}

// —— 复制频谱 ——
// 左侧 6% 是“更早”区，其余 94% 映射最近 24 小时
const xOf = (at: number) => {
  const age = Date.now() - at;
  if (age > DAY) return Math.max(1, 5 - ((Math.min(age, 7 * DAY) - DAY) / (6 * DAY)) * 4);
  return Math.min(99.4, 6 + 94 * (1 - age / DAY));
};
const hOf = (r: RecordSummary) => 16 + Math.min(1, Math.log10(Math.max(10, r.size)) / 8) * 84;
function renderSpectrum(freshId?: number) {
  const nowT = Date.now();
  const sorted = [...S.records.values()].filter((r) => nowT - r.copiedAt <= 7 * DAY).sort((a, b) => a.copiedAt - b.copiedAt);
  spBars.innerHTML = sorted.map((r, i) =>
    `<span class="bar${r.id === freshId ? ' fresh' : ''}" data-id="${r.id}" data-type="${r.kind}" style="left:${xOf(r.copiedAt)}%;height:${hOf(r)}%;--i:${i}"></span>`).join('');
  const ticks: [number, string][] = [[3, '更早'], ...[18, 12, 6].map((h): [number, string] => [6 + 94 * (1 - h / 24), hf.format(nowT - h * 3600e3)]), [100, '现在']];
  $('#spAxis').innerHTML = ticks.map(([x, l]) => `<span style="left:${x}%">${l}</span>`).join('');
  const day = sorted.filter((r) => nowT - r.copiedAt <= DAY);
  const hours: Record<number, number> = {};
  day.forEach((r) => {
    const h = new Date(r.copiedAt).getHours();
    hours[h] = (hours[h] || 0) + 1;
  });
  const peak = Object.entries(hours).sort((a, b) => b[1] - a[1])[0];
  $('#spStat').textContent = `24 小时内复制 ${fmtInt(day.length)} 次${peak && peak[1] > 1 ? ` · 最忙 ${peak[0]} 点（${peak[1]} 次）` : ''}`;
  paintSpectrumSel(false);
}
function paintSpectrumSel(animate = true) {
  spBars.querySelectorAll<HTMLElement>('.bar').forEach((b) => {
    const r = S.records.get(Number(b.dataset.id));
    b.classList.toggle('on', r?.id === S.sel);
    b.classList.toggle('dim', !!r && !matches(r));
  });
  const r = S.sel === null ? undefined : S.records.get(S.sel);
  if (!r) {
    spCursor.style.opacity = '0';
    return;
  }
  spCursor.style.transitionDuration = animate && !reduced.matches ? '' : '0ms';
  spCursor.style.transform = `translateX(${(spTrack.clientWidth * xOf(r.copiedAt)) / 100}px)`;
  spCursor.style.opacity = '1';
}
const barOf = (e: Event) => {
  const b = (e.target as Element).closest<HTMLElement>('.bar');
  const r = b ? S.records.get(Number(b.dataset.id)) : undefined;
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
    q.value = '';
    renderTabs(false);
    runSearch('', { stagger: true });
  }
  select(hit.r.id, { scroll: true, indDur: 260 });
});

// —— 分类标签 ——
const tabColor = (k: string) => (k === 'all' ? 'var(--beam)' : `var(--c-${k})`);
function renderTabs(instant: boolean) {
  const c = counts(S.records);
  const html = (real: boolean) => FILTERS.map(([k, l]) => {
    const mark = k === 'fav' ? svg('star', 'star') : '<i class="dot"></i>';
    const tag = real ? `button type="button" class="tab" data-f="${k}" aria-pressed="${S.filter === k}"` : 'span class="tab"';
    return `<${tag} data-type="${k}">${mark}${l}<span class="n">${c[k]}</span></${real ? 'button' : 'span'}>${k === 'fav' ? '<span class="tabs-sep"></span>' : ''}`;
  }).join('');
  tabsBase.innerHTML = html(true);
  tabsOver.innerHTML = html(false);
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
// lastFilter 的持久化在阶段 5（设置保存）
function setFilter(k: Filter, instant: boolean) {
  if (S.filter === k) return;
  S.filter = k;
  renderTabs(instant);
  // M06 规则 6：切换分类后回到顶部并选中第一条
  list.scrollTop = 0;
  S.sel = null;
  renderList({ stagger: !instant });
  paintSpectrumSel(!instant);
}

// —— 搜索 ——
// 每次输入都查后端，不防抖；序号丢弃先发后到的结果
let searchSeq = 0;
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
function runSearch(v: string, opts: { stagger?: boolean } = {}) {
  S.q = v;
  clearQ.hidden = !v;
  fetchMatches(v, () => {
    list.scrollTop = 0;
    S.sel = null;
    renderList(opts);
  });
}

// —— 提示条 ——
// 连续触发时只换文字、不重播弹入；非撤销提示会顶掉未完成的撤销
let toastTimer = 0;
function toast(msg: string, { undo = false, icon = 'check' as keyof typeof I } = {}) {
  if (!undo) S.undo = null;
  $('#toastIc').innerHTML = svg(icon);
  if (toastEl.dataset.open === 'true') fx(toastMsg, [{ opacity: 0, transform: 'translateY(4px)', filter: 'blur(2px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }], { duration: 180, easing: EASE_ENTER });
  toastMsg.textContent = msg;
  undoBtn.hidden = !undo;
  toastEl.dataset.open = 'true';
  if (undo) {
    const ring = $('#ring');
    ring.getAnimations().forEach((a) => a.cancel());
    ring.animate([{ strokeDashoffset: 0 }, { strokeDashoffset: 44 }], { duration: UNDO_MS, fill: 'forwards' });
  }
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl.dataset.open = 'false';
    if (undo) S.undo = null;
  }, undo ? UNDO_MS : 2400);
}

// —— 操作 ——
// 结果以后端事件为准：copy 之后 record-updated 带来新的 uses / copiedAt
let copyTimer = 0;
function copyRecord(id: number) {
  const before = S.records.get(id);
  if (!before || S.removing.has(id)) return;
  copy(id, true).then(
    (res) => {
      const cur = S.records.get(id);
      const uses = cur && cur !== before ? cur.uses : before.uses + 1;
      const fav = !isFav(before) && uses >= FAV_MIN;
      const msg = copyToast(res, S.platform.os, fav);
      if (msg) toast(msg, { icon: fav && res.result === 'copied' ? 'star' : 'check' });
      if (!S.shown) return;
      const sw = copyBtn.querySelector<HTMLElement>('.swap')!;
      sw.dataset.state = 'b';
      clearTimeout(copyTimer);
      copyTimer = window.setTimeout(() => (sw.dataset.state = 'a'), 1300);
      // “移到最上方”会改变位置，保持选中行可见
      renderList({ scroll: true });
      const r = rowEl(id);
      if (r && !reduced.matches) {
        r.classList.remove('beam');
        void r.offsetWidth;
        r.classList.add('beam');
        setTimeout(() => r.classList.remove('beam'), 800);
      }
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
    el.classList.add('lift');
    el.animate([{ transform: `translateY(${dy}px) scale(1.02)` }, { transform: 'translateY(0) scale(1)' }], { duration: 300, easing: EASE_MOVE })
      .finished.then(() => el.classList.remove('lift'), () => {});
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
  if (p && c?.kind === 'row') {
    // 直接加类让过渡播放；同步 p.h，避免下一次重绘整行替换
    p.el.querySelector('.item')?.classList.add('is-removing');
    p.h = rowHTML(r, c.index, false, false);
  }
  const bar = spBars.querySelector(`.bar[data-id="${id}"]`);
  fx(bar, [{ transform: 'scaleY(1)', opacity: 1 }, { transform: 'scaleY(0)', opacity: 0 }], { duration: 260, easing: EASE_MOVE, fill: 'forwards' });
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
    },
    (err: AppError) => {
      S.removing.delete(id);
      bar?.getAnimations().forEach((a) => a.cancel());
      paintRows();
      toast(err.message);
    },
  );
}

// 恢复后后端发 record-added，由 handlers.added 插入并选中
let restoring: number | null = null;
function undo() {
  const u = S.undo;
  if (!u || !canUndo(u, Date.now())) return;
  S.undo = null;
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
function openMenu(id: number, x: number, y: number, viaKb: boolean) {
  const r = S.records.get(id);
  if (!r) return;
  S.menuFor = id;
  $('#mPin').textContent = r.pinned ? '取消置顶' : '置顶';
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
  if (restore && menuReturn && document.contains(menuReturn)) menuReturn.focus({ preventScroll: true });
}
function moveMenuFocus(d: 1 | -1) {
  const bs = [...menu.querySelectorAll('button')];
  const i = bs.indexOf(document.activeElement as HTMLButtonElement);
  bs[(i + d + bs.length) % bs.length].focus();
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
      renderList({ scroll: restored });
      renderSpectrum(s.id);
      S.newId = null;
    };
    // 搜索中：新记录可能命中，重新取命中集合但不重置选中
    if (S.q.trim()) fetchMatches(S.q, paint);
    else paint();
  },
  updated: (s: RecordSummary) => {
    const prev = S.records.get(s.id);
    applyUpdated(S.records, s);
    renderTabs(true);
    // 去重命中会改 copiedAt，需要重新排序；选中项的头部与信息栏随 select 刷新，不重取详情
    renderList();
    renderSpectrum(prev && prev.copiedAt !== s.copiedAt ? s.id : undefined);
  },
  removed: (ids: number[]) => {
    // 自己发起的删除由 remove() 的计时器移除，这里跳过，也不清掉它的撤销
    const gone = ids.filter((id) => S.records.has(id) && !S.removing.has(id));
    if (!gone.length) return;
    const before = vis();
    applyRemoved(S.records, gone);
    if (S.sel !== null && gone.includes(S.sel)) S.sel = nextSelection(before, S.sel);
    if (S.undo && gone.includes(S.undo.id)) S.undo = null;
    renderTabs(true);
    renderList();
    renderSpectrum();
  },
  paused: (p: boolean) => {
    S.paused = p;
    paintPause();
  },
};

// —— 面板显隐 ——
let booted = false;
const shown = (t0: number | null) => {
  S.shown = true;
  win.dataset.shown = 'true';
  if (!booted) {
    // 首次唤起才播放 720ms 编排；之后的快捷键唤起只做 90ms 淡入
    booted = true;
    win.classList.add('boot');
    setTimeout(() => win.classList.remove('boot'), 1400);
  }
  q.value = '';
  q.focus();
  // 隐藏期间不刷新相对时间与分组，唤起时补一次
  runSearch('');
  renderSpectrum();
  if (import.meta.env.DEV && t0 !== null) {
    requestAnimationFrame(() => void info(`唤起耗时 ${Date.now() - t0} ms`));
  }
};
// 一次隐藏可能收到两次 panel-hidden，这里只写状态，重复调用无副作用
const hidden = () => {
  S.shown = false;
  win.dataset.shown = 'false';
  closeMenu(false);
  (document.activeElement as HTMLElement | null)?.blur();
};

await attachConsole();
// 先订阅再 bootstrap，避免两者之间的事件丢失
await onPanelShown(({ t0 }) => shown(t0));
await onPanelHidden(hidden);
await onRecordAdded((s) => handlers.added(s));
await onRecordUpdated((s) => handlers.updated(s));
await onRecordsRemoved((ids) => handlers.removed(ids));
await onPausedChanged((p) => handlers.paused(p));

const b = await bootstrap();
S.settings = b.settings;
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
const MOD = b.platform.os === 'macos' ? '⌘' : 'Ctrl';
document.querySelectorAll('.mod').forEach((k) => (k.textContent = MOD));
$('.modp').textContent = `${MOD} P`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);

// —— 事件 ——
q.addEventListener('input', (e) => {
  if ((e as InputEvent).isComposing) return;
  runSearch(q.value);
});
q.addEventListener('compositionend', () => runSearch(q.value));
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
delBtn.addEventListener('click', () => S.sel !== null && remove(S.sel));
moreBtn.addEventListener('click', (e) => {
  if (menuOpen()) return closeMenu(true);
  const r = moreBtn.getBoundingClientRect();
  if (S.sel !== null) openMenu(S.sel, r.right, r.bottom + 6, e.detail === 0);
});
menu.addEventListener('click', (e) => {
  const m = (e.target as Element).closest<HTMLElement>('[data-m]')?.dataset.m;
  const id = S.menuFor, mouse = e.detail !== 0;
  if (!m || id === null) return;
  closeMenu(true);
  if (m === 'copy') copyRecord(id);
  if (m === 'pin') togglePin(id, mouse);
  if (m === 'del') remove(id);
});
document.addEventListener('pointerdown', (e) => {
  const t = e.target as Node;
  if (menuOpen() && !menu.contains(t) && !moreBtn.contains(t)) closeMenu(false);
}, true);
undoBtn.addEventListener('click', undo);
document.querySelectorAll('[data-hide]').forEach((b) => b.addEventListener('click', () => void hidePanel()));
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
  const btn = t.closest<HTMLElement>('[data-copy]');
  if (!btn) return;
  const v = btn.dataset.copy!;
  copyText(v).then(
    () => {
      btn.textContent = '已复制';
      fx(btn, [{ transform: 'scale(.9)' }, { transform: 'scale(1)' }], { duration: 320, easing: EASE_SPRING });
      setTimeout(() => (btn.textContent = '复制'), 1200);
      toast(`已复制 ${v}`);
    },
    (err: AppError) => toast(err.message),
  );
});

addEventListener('resize', () => {
  renderTabs(true);
  paintRows();
  paintSpectrumSel(false);
  moveInd({ instant: true });
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
renderList({ stagger: true, base: 260 });
void document.fonts?.ready.then(() => {
  renderTabs(true);
  moveInd({ instant: true });
  paintSpectrumSel(false);
});

// —— 键盘 ——
document.addEventListener('keydown', (e) => {
  if (e.isComposing) return;
  const t = e.target as Element, inSearch = t === q;
  const a = keyAction(
    { key: e.key, code: e.code, mod: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey, repeat: e.repeat },
    {
      inSearch,
      onControl: !inSearch && !!t.closest?.('button, a, video'),
      searchHasText: !!q.value,
      menuOpen: menuOpen(),
      hasSel: S.sel !== null,
      canUndo: canUndo(S.undo, Date.now()),
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
      if (r) {
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
    case 'hide': return void hidePanel();
    case 'menu': {
      // ponytail: 选中行滚出视口未渲染时不弹菜单；键盘移动总会把选中行滚回视口
      const r = rowEl(sel)?.getBoundingClientRect();
      if (r && sel !== null) openMenu(sel, r.left + 48, r.bottom, true);
      return;
    }
  }
});
