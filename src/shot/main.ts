// 截图遮罩页面（shot.html），每块显示器一个窗口。结构对照原型 docs/specs/2026-10-28-截图与录屏-原型.html：
// 状态 S、update / place、指针与键盘处理。坐标为本块屏的逻辑像素，冻结帧与标注 canvas 为物理像素。
import './shot.css';
import {
  onShotBegin, onShotClaim, onShotEnd, onShotTranslateDelta, recStart, shotAlive, shotCancel, shotClaim, shotCopyText, shotEngines,
  shotFinish, shotFrame, shotLongStart, shotReady, shotTranslateAgain, type BeginPayload, type ShotAction, type ShotOutcome,
} from '../api/shot';
import { error as logError, info } from '../api/log';
import type { AppError, Engines } from '../api/types';
import { cardInner, type CardState } from '../card/view';
import { otherEngine, staleDelta, type Engine } from '../translate';
import { badgePos, hit, loupePos, norm, outputSize, placeBar, placeCard, resize, type Handle, type Rect } from './geom';
import { COLORS, drawObj, FONT, nextStep, type Obj, type Tool } from './ink';

// 遮罩页出错时整个截图会静默失败：报错写进日志（只记消息，不含截图内容）
addEventListener('error', (e) => void logError(`遮罩页错误：${e.message}`));
addEventListener('unhandledrejection', (e) => void logError(`遮罩页未处理的拒绝：${String(e.reason)}`));

const ICON = {
  rect: '<rect x="4" y="5.5" width="16" height="13" rx="2"/>',
  ellipse: '<ellipse cx="12" cy="12" rx="8.5" ry="6.5"/>',
  arrow: '<path d="M5.5 18.5 18 6"/><path d="M10 5.5h8.5V14"/>',
  pen: '<path d="M15.5 4.5l4 4L9 19l-5 1 1-5z"/><path d="M13 7l4 4"/>',
  mosaic: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 9.3h16M4 14.6h16M9.3 4v16M14.6 4v16"/><path d="M4.5 4.5h4.8v4.8H4.5zM14.6 9.3h4.9v5.3h-4.9zM9.3 14.6h5.3v4.9H9.3z" fill="currentColor" stroke="none"/>',
  text: '<path d="M5 7V5h14v2"/><path d="M12 5v14"/><path d="M9 19h6"/>',
  step: '<circle cx="12" cy="12" r="8.5"/><path d="M10.6 9.4 12.6 8v8"/>',
  undo: '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>',
  redo: '<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>',
  translate: '<path d="M3.6 6h8.4M7.8 4.2V6M10.4 6c-.8 3.6-3 6.2-6.2 7.8M6.2 9.1c1.1 2 2.8 3.5 4.8 4.3"/><path d="m12.6 19.8 3.6-9 3.6 9M13.9 16.8h4.6"/>',
  long: '<rect x="6" y="3.5" width="12" height="17" rx="2"/><path d="M12 7.5v9M9.6 9.9 12 7.5l2.4 2.4M9.6 14.1 12 16.5l2.4-2.4"/>',
  ocr: '<path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16"/><path d="M8 9h8M8 12h8M8 15h5"/>',
  save: '<path d="M12 4v11"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>',
  close: '<path d="M6 6l12 12M18 6 6 18"/>',
  pin: '<path d="M9 3.5h6l-.8 6.2 3.3 3.3h-11l3.3-3.3z"/><path d="M12 13v7.5"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  speaker: '<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6M18 6.5a7.8 7.8 0 0 1 0 11"/>',
  speakerOff: '<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z"/><path d="m16 9.5 5 5M21 9.5l-5 5"/>',
  mic: '<rect x="9" y="3.5" width="6" height="10.5" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>',
  micOff: '<rect x="9" y="3.5" width="6" height="10.5" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/><path d="M4 4l16 16"/>',
};
type Icon = keyof typeof ICON;
const svg = (n: Icon) =>
  `<svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICON[n]}</svg>`;

const $ = <T extends HTMLElement = HTMLElement>(s: string) => document.querySelector<T>(s)!;
const clamp = (v: number, a: number, b: number) => Math.min(Math.max(v, a), b);
const isMac = /Mac/.test(navigator.userAgent);
const MODK = isMac ? '⌘' : 'Ctrl';
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const mk = (w: number, h: number) => Object.assign(document.createElement('canvas'), { width: w, height: h });
const inside = (x: number, y: number, r: Rect | null) => !!r && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
const restart = (el: HTMLElement, cls: string) => { el.classList.remove(cls); void el.offsetWidth; el.classList.add(cls); };

const ov = $('#ov'), frozen = $<HTMLCanvasElement>('#frozen'), ink = $<HTMLCanvasElement>('#ink'), ic = ink.getContext('2d')!;
const cut = $('#cut'), badge = $('#badge'), loupe = $('#loupe'), lc = $<HTMLCanvasElement>('#lc').getContext('2d')!, linfo = $('#linfo');
const tb = $('#tb'), sub = $('#sub'), busy = $('#busy'), hint = $('#hint'), gx = $('#gx'), gy = $('#gy');
const toastEl = $('#toast'), toastLead = $('#toastLead'), toastMsg = $('#toastMsg'), count = $('#count');

const TOOLS: [Tool, string][] = [['rect', '矩形'], ['ellipse', '椭圆'], ['arrow', '箭头'], ['pen', '画笔'], ['mosaic', '马赛克'], ['text', '文字'], ['step', '序号']];
const COLOR_NAMES = ['红', '橙', '绿', '蓝', '紫', '白'];
/** 动作完成后提示条停留多久再退出截图 */
const TOAST_MS = 1600;

/** `countdown`：录屏倒计时（M24）；倒计时结束到后端结束会话之间为 `done`；`card`：译文卡片（M26 规则 5） */
type Phase = 'off' | 'idle' | 'selected' | 'countdown' | 'busy' | 'card' | 'done';
type Drag =
  | { k: 'new'; x0: number; y0: number; moved: boolean }
  | { k: 'move' | 'resize'; h?: Handle; s: Rect; x0: number; y0: number }
  | { k: 'draw' };
type Editing = { el: HTMLElement; x: number; y: number; color: string; size: 0 | 1 | 2 };

const S = {
  gen: 0, index: 0, scale: 1, rects: [] as Rect[],
  phase: 'off' as Phase,
  /** 光标在这块屏上（其他屏只显示蒙层） */
  here: false,
  snap: null as Rect | null, sel: null as Rect | null, drag: null as Drag | null,
  tool: null as Tool | null, color: 0, size: 1 as 0 | 1 | 2,
  objs: [] as Obj[], redo: [] as Obj[], cur: null as Obj | null, editing: null as Editing | null,
  mx: 0, my: 0, barY: 0,
  /** 设置“保存截图时同时复制”（只影响工具栏提示） */
  saveCopy: true,
  /** 另一块屏正在识别文字：这块屏不能再框选 */
  locked: false,
  /** 截图（M23）或选录屏区域（M24） */
  mode: 'shot' as 'shot' | 'rec',
  /** 本次录屏的系统声音、麦克风开关；麦克风为 null 表示本机不支持（macOS 15 以下） */
  sys: false, mic: null as boolean | null,
  /** 录屏输出缩放与帧率（设置“录屏清晰度”“录屏帧率”），尺寸标签显示输出像素（M24 规则 3） */
  recScale: 1, fps: 30,
};
let W = innerWidth, H = innerHeight, tc = '#FFB84D';
let pix: HTMLCanvasElement | null = null; // 马赛克底图，第一次用马赛克时生成
let thumbUrl = '', toastT = 0, endT = 0, cdT = 0;
/** 本屏已调用 shot_ready 的代数：只有已上屏的屏发心跳 */
let readyGen = 0;
/** 进行中的截图翻译：kind `done` 收完（可复制）；engine 为翻译方式标签，出第一段前先按设置猜；
 *  seq 每次换用加一，迟到的结果按它丢弃；回到选区或开始新的一次时为 null */
type ShotTr = Exclude<CardState, { kind: 'blocked' }> & { gen: number; seq: number; want?: Engine };
let tr: ShotTr | null = null;
let trSeq = 0;
/** 两种翻译方式能不能用，每次按 T 取一次（M26 规则 16） */
let engines: Engines | null = null;

/* 译文卡片（M26 规则 5、16）：第一段译文到达（或识别后翻译失败）时出现，定一次位置后只变高；内容由 cardInner 生成 */
const card = document.createElement('div');
card.className = 'card';
card.hidden = true;
card.setAttribute('role', 'dialog');
card.setAttribute('aria-label', '译文');
ov.appendChild(card);
const cardBody = () => card.querySelector<HTMLElement>('.c-body');

/* —— 开始：画冻结帧，画完才让后端上屏 —— */
async function begin(p: BeginPayload) {
  // 推送与主动拉取都会到；代数单调递增，迟到的旧载荷也不能拿旧帧重开
  if (p.generation <= S.gen) return;
  const gen = (S.gen = p.generation);
  const f = await shotFrame();
  if (gen !== S.gen) return;
  for (const c of [frozen, ink]) { c.width = f.w; c.height = f.h; }
  frozen.getContext('2d')!.putImageData(new ImageData(f.rgba, f.w, f.h), 0, 0);
  pix = null;
  W = innerWidth; H = innerHeight;
  // 缩放系数以帧与窗口的实际尺寸为准（窗口摆位与显示器缩放不一致时，坐标仍能对上冻结帧）
  let scale = p.scale;
  if (W > 0 && Math.abs(f.w / W - p.scale) > 0.01) {
    scale = f.w / W;
    void info(`shot 屏 ${p.index} 缩放不一致：载荷 ${p.scale}，帧 ${f.w}×${f.h} / 窗口 ${W}×${H}`);
  }
  Object.assign(S, {
    index: p.index, scale, saveCopy: p.saveCopy, mode: p.mode, sys: p.systemAudio, mic: p.microphone, recScale: p.recScale, fps: p.recFps, locked: false, rects: p.rects, phase: 'idle', here: !!p.cursor, sel: null, drag: null,
    tool: null, color: 0, size: 1, objs: [], redo: [], cur: null, editing: null,
    mx: p.cursor?.[0] ?? 0, my: p.cursor?.[1] ?? 0,
  });
  S.snap = S.here ? hit(S.rects, S.mx, S.my, W, H) : null;
  clearTimeout(toastT); clearTimeout(endT); clearTimeout(cdT);
  count.hidden = true;
  if (thumbUrl) URL.revokeObjectURL(thumbUrl);
  thumbUrl = '';
  toastEl.setAttribute('data-instant', ''); toastEl.dataset.open = 'false'; void toastEl.offsetWidth; toastEl.removeAttribute('data-instant');
  document.querySelectorAll('.txt').forEach((el) => el.remove());
  tr = null;
  card.hidden = true;
  ic.clearRect(0, 0, ink.width, ink.height);
  ov.dataset.mode = p.mode;
  ov.hidden = false;
  ov.classList.remove('out');
  tc = getComputedStyle(ov).getPropertyValue('--tc').trim() || tc;
  buildBar();
  hint.innerHTML = (p.mode === 'rec' ? '<span><b>选择录制区域</b></span>' : '')
    + '<span><b>单击</b>选中窗口</span><span><b>拖动</b>框选</span><span><kbd>F</kbd>全屏</span><span><kbd>Esc</kbd>退出</span>';
  restart(hint, 'hintbar');
  setCursor();
  update();
  restart(ov, 'in');
  setTimeout(() => ov.classList.remove('in'), 120);
  readyGen = gen;
  await shotReady(gen);
}

/** 退出截图：遮罩淡出后后端隐藏全部遮罩。 */
function closeOverlay() {
  if (S.phase === 'off') return;
  S.phase = 'off';
  S.editing?.el.remove();
  S.editing = null;
  ov.classList.add('out');
  // 后端 end 只隐藏窗口：告诉其他屏一起淡出并放掉像素
  const gen = S.gen;
  void shotClaim({ generation: gen, index: S.index, state: 'done' });
  endT = window.setTimeout(() => { release(); void shotCancel(gen); }, reduced.matches ? 0 : 160);
}

/** 遮罩隐藏后留着复用：放掉冻结帧、标注与马赛克底图的像素（5K 屏每张一百多 MB），用户的屏幕画面不留在内存里。 */
function release() {
  S.phase = 'off';
  clearTimeout(cdT);
  count.hidden = true;
  for (const c of [frozen, ink]) { c.width = 0; c.height = 0; }
  pix = null;
  lc.clearRect(0, 0, 122, 122);
  if (thumbUrl) URL.revokeObjectURL(thumbUrl);
  thumbUrl = '';
}

/* 当前显示的框：选区前是吸附的窗口，拖动中和选区后是选区 */
function shownRect() {
  if (S.phase === 'idle') return S.drag?.k === 'new' && S.drag.moved ? S.sel : S.snap;
  return S.sel;
}

function update() {
  if (S.phase === 'off' || (S.phase === 'done' && !S.sel)) return;
  const r = S.scale, rect = shownRect(), d = S.drag;
  ov.dataset.phase = S.phase;
  ov.dataset.tool = S.tool ?? '';
  const c = rect ?? { x: 0, y: 0, w: 0, h: 0 };
  Object.assign(cut.style, { left: c.x + 'px', top: c.y + 'px', width: c.w + 'px', height: c.h + 'px' });
  cut.classList.toggle('none', !rect);
  cut.classList.toggle('snap', S.phase === 'idle' && !(d?.k === 'new' && d.moved));

  // 十字参考线与放大镜：选区前一直跟随光标，选区后只在调整大小时出现
  const dragging = d?.k === 'new' || d?.k === 'resize';
  // 另一块屏正在识别文字（locked）：这块屏只有蒙层
  const live = S.here && !S.locked;
  const showLoupe = live && (S.phase === 'idle' || dragging);
  gx.hidden = gy.hidden = !(live && S.phase === 'idle' && !dragging);
  gx.style.transform = `translateY(${Math.round(S.my)}px)`;
  gy.style.transform = `translateX(${Math.round(S.mx)}px)`;
  loupe.hidden = !showLoupe;
  if (showLoupe) drawLoupe(dragging ? rect : null);

  hint.hidden = S.phase !== 'idle' || !live;
  const above = place();

  // 尺寸标签：截图为物理像素，录屏为按清晰度换算的输出像素
  badge.hidden = !rect || (S.phase !== 'idle' && S.phase !== 'selected');
  if (rect && !badge.hidden) {
    const [bw, bh] = S.mode === 'rec' ? outputSize(rect.w, rect.h, S.recScale) : [Math.round(rect.w * r), Math.round(rect.h * r)];
    badge.innerHTML = `<i></i>${bw}<span class="x">×</span>${bh}${S.mode === 'rec' ? `<span class="fps">· ${S.fps} fps</span>` : ''}`;
    const b = badgePos(rect, badge.offsetWidth, above, W);
    Object.assign(badge.style, { left: b.x + 'px', top: b.y + 'px' });
  }
}

function drawLoupe(rect: Rect | null) {
  const r = S.scale, N = 15, Z = 122 / N;
  lc.imageSmoothingEnabled = false;
  lc.fillStyle = '#000'; lc.fillRect(0, 0, 122, 122);
  lc.drawImage(frozen, (S.mx - N / 2) * r, (S.my - N / 2) * r, N * r, N * r, 0, 0, 122, 122);
  lc.strokeStyle = 'rgba(255,255,255,.08)'; lc.lineWidth = 1;
  for (let i = 1; i < N; i++) { lc.beginPath(); lc.moveTo(i * Z, 0); lc.lineTo(i * Z, 122); lc.moveTo(0, i * Z); lc.lineTo(122, i * Z); lc.stroke(); }
  lc.strokeStyle = tc; lc.lineWidth = 1.5;
  lc.beginPath(); lc.moveTo(61, 0); lc.lineTo(61, 122); lc.moveTo(0, 61); lc.lineTo(122, 61); lc.globalAlpha = 0.45; lc.stroke(); lc.globalAlpha = 1;
  lc.strokeRect(7 * Z, 7 * Z, Z, Z);
  linfo.innerHTML = rect
    ? `<span>${Math.round(rect.w * r)} × ${Math.round(rect.h * r)}</span>`
    : `<span>X ${Math.round(S.mx * r)}</span><span>Y ${Math.round(S.my * r)}</span>`;
  const p = loupePos(S.mx, S.my, W, H);
  Object.assign(loupe.style, { left: p.x + 'px', top: p.y + 'px' });
}

/* —— 工具栏 —— */
function buildBar() {
  tb.setAttribute('aria-label', S.mode === 'rec' ? '录屏选项' : '截图工具');
  if (S.mode === 'rec') {
    // 录屏：两个声音开关只改本次（rec_start 时写回设置），麦克风在 macOS 15 以下不可用（M24）
    const micOk = S.mic !== null;
    tb.innerHTML = `<button type="button" data-act="sys" aria-pressed="${S.sys}" aria-label="录制系统声音" data-tip="录制系统声音：${S.sys ? '开' : '关'}">${svg(S.sys ? 'speaker' : 'speakerOff')}</button>`
      + `<button type="button" data-act="mic" aria-pressed="${!!S.mic}" ${micOk ? '' : 'aria-disabled="true"'} aria-label="录制麦克风" data-tip="${micOk ? `录制麦克风：${S.mic ? '开' : '关'}` : '麦克风需要 macOS 15'}">${svg(S.mic ? 'mic' : 'micOff')}</button>`
      + '<span class="sep"></span>'
      + `<button type="button" data-act="close" aria-label="取消" data-tip="取消  Esc">${svg('close')}</button>`
      + '<button type="button" class="go" data-act="start"><span class="rd"></span>开始录制<kbd>↩</kbd></button>';
    return;
  }
  tb.innerHTML = TOOLS.map(([t, n], i) => `<button type="button" data-tool="${t}" aria-pressed="false" aria-label="${n}" data-tip="${n}  ${i + 1}">${svg(t)}</button>`).join('')
    + '<span class="sep"></span>'
    + `<button type="button" data-act="undo" aria-label="撤销" data-tip="撤销  ${MODK} Z">${svg('undo')}</button>`
    + `<button type="button" data-act="redo" aria-label="重做" data-tip="重做  ${isMac ? '⇧ ⌘ Z' : 'Ctrl Shift Z'}">${svg('redo')}</button>`
    + '<span class="sep"></span>'
    + `<button type="button" data-act="long" aria-label="长截图" data-tip="长截图  L">${svg('long')}</button>`
    + `<button type="button" data-act="ocr" aria-label="识别文字并复制" data-tip="识别文字并复制  O">${svg('ocr')}</button>`
    + `<button type="button" data-act="translate" aria-label="识别文字并翻译" data-tip="识别文字并翻译  T">${svg('translate')}</button>`
    + `<button type="button" data-act="pin" aria-label="钉在屏幕上" data-tip="钉在屏幕上  P">${svg('pin')}</button>`
    + `<button type="button" data-act="save" aria-label="保存" data-tip="保存${S.saveCopy ? '并复制' : ''}  ${MODK} S">${svg('save')}</button>`
    + `<button type="button" data-act="close" aria-label="退出" data-tip="退出  Esc">${svg('close')}</button>`
    + '<button type="button" class="go" data-act="copy">复制<kbd>↩</kbd></button>';
  syncBar();
}

function syncBar() {
  tb.querySelectorAll<HTMLElement>('[data-tool]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.tool === S.tool)));
  tb.querySelector('[data-act="undo"]')?.setAttribute('aria-disabled', String(!S.objs.length));
  tb.querySelector('[data-act="redo"]')?.setAttribute('aria-disabled', String(!S.redo.length));
  // 长截图截的是滚动中的实时画面，带不上标注（M23 规则 13）
  const long = tb.querySelector<HTMLElement>('[data-act="long"]');
  if (long) {
    long.setAttribute('aria-disabled', String(S.objs.length > 0));
    long.dataset.tip = S.objs.length ? '长截图不带标注，先撤销标注' : '长截图  L';
  }
  if (!S.tool) { sub.innerHTML = ''; return; }
  const colors = S.tool !== 'mosaic'
    ? COLORS.map((c, i) => `<button type="button" class="sw" data-color="${i}" aria-pressed="${i === S.color}" aria-label="${COLOR_NAMES[i]}色" style="--c:${c}"><i></i></button>`).join('') + '<span class="sep"></span>'
    : '';
  const sizes = [0, 1, 2].map((i) => {
    const glyph = S.tool === 'text' ? `<b style="--fs:${[11, 14, 18][i]}px">A</b>` : `<i style="width:${[4, 7, 11][i]}px;height:${[4, 7, 11][i]}px"></i>`;
    const name = S.tool === 'text' ? ['小', '中', '大'][i] + '号字' : ['细', '中', '粗'][i];
    return `<button type="button" class="sz" data-size="${i}" aria-pressed="${i === S.size}" aria-label="${name}">${glyph}</button>`;
  }).join('');
  sub.innerHTML = colors + sizes;
}

/** 摆放工具栏、颜色粗细条与识别中提示；返回工具栏是否在选区上方。 */
function place(): boolean {
  const sel = S.sel, show = S.phase === 'selected' && !!sel;
  tb.hidden = !show;
  sub.hidden = !(show && S.tool);
  busy.hidden = S.phase !== 'busy';
  if (S.phase === 'busy' && sel) {
    const bw = busy.offsetWidth;
    Object.assign(busy.style, { left: clamp(sel.x + sel.w - bw, 8, W - bw - 8) + 'px', top: S.barY + 'px' });
    return false;
  }
  if (!show) return false;
  const th = tb.offsetHeight, sw = sub.hidden ? 0 : sub.offsetWidth, sh = sub.hidden ? 0 : sub.offsetHeight + 6;
  const p = placeBar(sel, { w: tb.offsetWidth, h: th }, sh, W, H);
  S.barY = p.y;
  Object.assign(tb.style, { left: p.x + 'px', top: p.y + 'px' });
  tb.classList.toggle('below-tips', p.y < 44);
  if (!sub.hidden) {
    const btn = tb.querySelector<HTMLElement>(`[data-tool="${S.tool}"]`);
    const bx = btn ? p.x + btn.offsetLeft + btn.offsetWidth / 2 - sw / 2 : p.x;
    Object.assign(sub.style, { left: clamp(bx, 8, W - sw - 8) + 'px', top: (p.above ? p.y - sh : p.y + th + 6) + 'px' });
  }
  return p.above;
}

/** 进入选区后。`animate`：鼠标触发时工具栏浮入，键盘触发（F）即时出现（M20 原则 2）。 */
function enterSelected(animate: boolean) {
  S.phase = 'selected';
  update();
  tb.classList.remove('pop-in');
  if (animate) restart(tb, 'pop-in');
  // 选区只能落在一块屏内：其他屏放弃各自的选区
  void shotClaim({ generation: S.gen, index: S.index, state: 'selected' });
}

tb.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('button');
  if (!b || b.getAttribute('aria-disabled') === 'true') return;
  if (b.dataset.tool) return toggleTool(b.dataset.tool as Tool, true);
  const a = b.dataset.act;
  if (a === 'undo') undo();
  else if (a === 'redo') redo();
  else if (a === 'close') closeOverlay();
  else if (a === 'sys') { S.sys = !S.sys; buildBar(); }
  else if (a === 'mic') { S.mic = !S.mic; buildBar(); }
  else if (a === 'start') startCountdown();
  else if (a === 'long') void startLong();
  else if (a) void act(a as ShotAction);
});
sub.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('button');
  if (!b) return;
  if (b.dataset.color) S.color = +b.dataset.color;
  if (b.dataset.size) S.size = +b.dataset.size as 0 | 1 | 2;
  syncBar();
});
function toggleTool(t: Tool, animate: boolean) {
  commitText();
  S.tool = S.tool === t ? null : t;
  syncBar(); update();
  sub.classList.remove('pop-in');
  if (animate && !sub.hidden) restart(sub, 'pop-in');
}

/* —— 指针 —— */
ov.addEventListener('pointerdown', (e) => {
  if (e.button !== 0) return;
  if ((e.target as Element).closest('.bar, .txt, .card')) return;
  const x = e.clientX, y = e.clientY;
  if (S.editing) { commitText(); if (S.tool === 'text') return; }
  if (S.phase === 'idle') {
    if (S.locked) return;
    Object.assign(S, { here: true, mx: x, my: y, snap: hit(S.rects, x, y, W, H) });
    S.drag = { k: 'new', x0: x, y0: y, moved: false };
  } else if (S.phase === 'selected') {
    const h = (e.target as HTMLElement).dataset?.h as Handle | undefined;
    if (h) S.drag = { k: 'resize', h, s: { ...S.sel! }, x0: x, y0: y };
    else if (inside(x, y, S.sel) && S.tool) { if (S.tool === 'text') e.preventDefault(); startDraw(x, y, e.shiftKey); if (!S.drag) return; }
    else if (inside(x, y, S.sel)) S.drag = { k: 'move', s: { ...S.sel! }, x0: x, y0: y };
    else return;
  } else return;
  ov.setPointerCapture(e.pointerId);
  update();
});

ov.addEventListener('pointermove', (e) => {
  if (S.phase === 'off') return;
  const x = e.clientX, y = e.clientY, d = S.drag;
  Object.assign(S, { mx: x, my: y, here: true });
  if (!d) {
    if (S.phase === 'idle' && !S.locked) S.snap = hit(S.rects, x, y, W, H);
    setCursor();
    if (S.phase === 'idle') update();
    return;
  }
  if (d.k === 'draw') return extendDraw(x, y, e.shiftKey);
  const dx = Math.round(x - d.x0), dy = Math.round(y - d.y0);
  if (d.k === 'new') {
    if (!d.moved && Math.hypot(dx, dy) > 3) d.moved = true;
    if (d.moved) S.sel = norm(d.x0, d.y0, x, y, W, H);
  } else if (d.k === 'move') {
    S.sel = { ...d.s, x: clamp(d.s.x + dx, 0, W - d.s.w), y: clamp(d.s.y + dy, 0, H - d.s.h) };
    renderInk();
  } else {
    S.sel = resize(d.s, d.h!, dx, dy, W, H);
    renderInk();
  }
  update();
});

ov.addEventListener('pointerup', () => {
  const d = S.drag;
  if (!d) return;
  S.drag = null;
  if (d.k === 'new') {
    if (!d.moved || !S.sel || S.sel.w < 4 || S.sel.h < 4) S.sel = { ...S.snap! };
    renderInk();
    enterSelected(true);
    return;
  }
  if (d.k === 'draw') finishDraw();
  update();
});

// 光标离开这块屏：选区前只留蒙层
document.documentElement.addEventListener('pointerleave', () => {
  if (S.phase !== 'idle' || S.drag) return;
  S.here = false;
  S.snap = null;
  update();
});

ov.addEventListener('dblclick', (e) => {
  if (S.phase !== 'selected' || S.tool || !inside(e.clientX, e.clientY, S.sel) || (e.target as Element).closest('.bar')) return;
  if (S.mode === 'rec') startCountdown();
  else void act('copy');
});
ov.addEventListener('contextmenu', (e) => { e.preventDefault(); escape(); });

function setCursor() {
  let c = 'crosshair';
  if (S.phase === 'card') c = 'default';
  else if (S.phase === 'selected') {
    const on = inside(S.mx, S.my, S.sel);
    if (S.tool) c = on ? (S.tool === 'text' ? 'text' : 'crosshair') : 'default';
    else c = on ? 'move' : 'default';
  }
  ov.style.cursor = c;
}

function selectAll() {
  if (S.locked) return;
  S.here = true;
  S.sel = { x: 0, y: 0, w: W, h: H };
  renderInk();
  if (S.phase === 'idle') enterSelected(false);
  else update();
}

/* —— 标注 —— */
function startDraw(x: number, y: number, shift: boolean) {
  const color = COLORS[S.color], size = S.size, t = S.tool!;
  if (t === 'text') return openText(x, y);
  if (t === 'step') {
    S.objs.push({ t, color, size, x, y, n: nextStep(S.objs) });
    S.redo = []; renderInk(); syncBar();
    return;
  }
  S.cur = t === 'pen' || t === 'mosaic'
    ? { t, color, size, pts: [[x, y]] }
    : { t, color, size, x0: x, y0: y, x1: x, y1: y, shift };
  if (t === 'mosaic' && !pix) pix = mosaicBase();
  S.drag = { k: 'draw' };
  renderInk();
}
function extendDraw(x: number, y: number, shift: boolean) {
  const o = S.cur;
  if (!o) return;
  if (o.t === 'pen' || o.t === 'mosaic') {
    const last = o.pts[o.pts.length - 1];
    if (Math.hypot(x - last[0], y - last[1]) > 1.5) o.pts.push([x, y]);
  } else if (o.t === 'rect' || o.t === 'ellipse' || o.t === 'arrow') Object.assign(o, { x1: x, y1: y, shift });
  renderInk();
}
function finishDraw() {
  const o = S.cur;
  S.cur = null;
  if (!o) return;
  const keep = o.t === 'pen' || o.t === 'mosaic'
    || ((o.t === 'rect' || o.t === 'ellipse' || o.t === 'arrow') && (Math.abs(o.x1 - o.x0) > 2 || Math.abs(o.y1 - o.y0) > 2));
  if (keep) { S.objs.push(o); S.redo = []; }
  renderInk(); syncBar();
}
/** 马赛克底图：冻结帧缩小 10 × 缩放系数倍再不平滑地放大，块为 10 逻辑像素。 */
function mosaicBase() {
  const B = 10 * S.scale, w = frozen.width, h = frozen.height;
  const small = mk(Math.ceil(w / B), Math.ceil(h / B));
  small.getContext('2d')!.drawImage(frozen, 0, 0, small.width, small.height);
  const out = mk(w, h), c = out.getContext('2d')!;
  c.imageSmoothingEnabled = false;
  c.drawImage(small, 0, 0, w, h);
  return out;
}
function renderInk() {
  const r = S.scale;
  ic.setTransform(1, 0, 0, 1, 0, 0);
  ic.clearRect(0, 0, ink.width, ink.height);
  if (!S.sel) return;
  ic.setTransform(r, 0, 0, r, 0, 0);
  ic.save(); ic.beginPath(); ic.rect(S.sel.x, S.sel.y, S.sel.w, S.sel.h); ic.clip();
  for (const o of S.objs) drawObj(ic, o, pix ?? frozen);
  if (S.cur) drawObj(ic, S.cur, pix ?? frozen);
  ic.restore();
}
function undo() { commitText(); const o = S.objs.pop(); if (!o) return; S.redo.push(o); renderInk(); syncBar(); }
function redo() { const o = S.redo.pop(); if (!o) return; S.objs.push(o); renderInk(); syncBar(); }

/* 文字标注：单击放置输入框，点别处或 Esc 完成；输入期间全局键位不生效 */
function openText(x: number, y: number) {
  const el = document.createElement('div');
  el.className = 'txt';
  el.contentEditable = 'plaintext-only';
  if (el.contentEditable !== 'plaintext-only') el.contentEditable = 'true';
  const color = COLORS[S.color];
  Object.assign(el.style, { left: x - 5 + 'px', top: y - 3 + 'px', fontSize: FONT[S.size] + 'px', color });
  el.setAttribute('aria-label', '标注文字');
  ov.appendChild(el);
  S.editing = { el, x, y, color, size: S.size };
  el.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Escape' || (e.key === 'Enter' && (isMac ? e.metaKey : e.ctrlKey))) { e.preventDefault(); commitText(); }
  });
  el.addEventListener('pointerdown', (e) => e.stopPropagation());
  el.focus();
}
function commitText() {
  const ed = S.editing;
  if (!ed) return;
  S.editing = null;
  const text = ed.el.innerText.replace(/\n$/, '');
  ed.el.remove();
  if (text.trim()) { S.objs.push({ t: 'text', x: ed.x, y: ed.y, text, color: ed.color, size: ed.size }); S.redo = []; }
  renderInk(); syncBar();
}

/* —— 输出：选区 × 缩放系数，从冻结帧与标注合成 PNG —— */
async function exportPng(sel: Rect): Promise<Blob> {
  const r = S.scale;
  const sx = Math.round(sel.x * r), sy = Math.round(sel.y * r);
  const sw = clamp(Math.round(sel.w * r), 1, frozen.width - sx), sh = clamp(Math.round(sel.h * r), 1, frozen.height - sy);
  const out = mk(sw, sh), oc = out.getContext('2d')!;
  oc.drawImage(frozen, sx, sy, sw, sh, 0, 0, sw, sh);
  oc.drawImage(ink, sx, sy, sw, sh, 0, 0, sw, sh);
  const blob = await new Promise<Blob | null>((res) => out.toBlob(res, 'image/png'));
  if (!blob) throw { message: '截图编码失败' };
  return blob;
}

/** 等待期间没有退出、也没有开始新的一次截图 */
const live = (gen: number) => gen === S.gen && S.phase !== 'off';

/** 复制 / 保存 / 识别文字 / 贴图 / 翻译。一次截图只做一个动作；识别文字、翻译期间显示进行中提示。 */
async function act(a: ShotAction) {
  if (S.phase !== 'selected' || !S.sel) return;
  commitText();
  const gen = S.gen, sel = { ...S.sel }, slow = a === 'ocr' || a === 'translate';
  busy.lastChild!.textContent = a === 'translate' ? '正在翻译…' : '正在识别文字…';
  S.phase = slow ? 'busy' : 'done';
  if (a === 'translate') {
    const t: ShotTr = tr = { gen, seq: ++trSeq, kind: 'busy', source: '', engine: '', text: '', noAccess: false, other: null };
    // 出第一段前就失败时，标题行按设置标出方式；增量、返回值带的是实际用的方式
    void shotEngines().then((e) => { engines = e; if (tr === t) { t.engine ||= e.current; renderCard(); } }, () => {});
  }
  update();
  // 复制、保存、贴图：确认的一刻遮罩就开始淡出，不等编码与写剪切板（整屏 5K 编码要几百毫秒），
  // 也不再播光扫：光扫、停顿、再淡出叠在一起，真机反馈像“抖一下”（M23 规则 7）。失败时恢复
  // 其他屏的蒙层一起淡出（succeed 时再广播一次也无妨）；万一失败，其他屏留着透明遮罩直到这次截图结束
  if (!slow) { ov.classList.add('out'); void shotClaim({ generation: gen, index: S.index, state: 'done' }); }
  // 识别、翻译要几秒：期间其他屏不能框选、出第二个动作
  if (slow) void shotClaim({ generation: gen, index: S.index, state: 'busy' });
  const seq0 = tr?.seq;
  let out: ShotOutcome, blob: Blob;
  try {
    blob = await exportPng(sel);
    out = await shotFinish(new Uint8Array(await blob.arrayBuffer()), a, S.index, sel);
  } catch (e) {
    const msg = (e as AppError)?.message || '截图处理失败';
    // 期间已按 Esc 退出；截图结束时后端中止翻译，结果丢弃。翻译中换用了：结果归换用那次
    if (!live(gen) || msg === '翻译已取消' || (a === 'translate' && tr && tr.seq !== seq0)) return;
    // 译文卡片已出现，或识别出文字后翻译失败：卡片上显示原因（保留已收到的部分），不能复制，
    // 可以换用另一种或关闭；不给下载语言包（M26 规则 5、16）
    if (a === 'translate' && tr && ((S.phase as Phase) === 'card' || TR_FAIL.has((e as AppError)?.kind))) return failTr(tr, e as AppError);
    // 失败：回到选区，可以重试或换个动作
    tr = null;
    ov.classList.remove('out');
    S.phase = 'selected';
    update();
    if (slow) void shotClaim({ generation: gen, index: S.index, state: 'selected' });
    toast(msg, { icon: 'close', danger: true });
    toastT = window.setTimeout(() => (toastEl.dataset.open = 'false'), 2800);
    return;
  }
  if (!live(gen)) return;
  if (a === 'translate') {
    // 收完：以完整译文为准（最后一段增量可能晚于返回值到达）；后端不进提示条模式，复制时才进
    if (tr && tr.seq === seq0) doneTr(tr, out);
    return;
  }
  if (a === 'copy' || a === 'save') thumbUrl = URL.createObjectURL(blob);
  const file = out.file ?? '';
  const chars = out.chars ?? 0;
  const [msg, lead]: [string, Lead] =
    a === 'copy' ? ['截图已复制，已加入历史', { thumb: thumbUrl }]
    : a === 'save' ? [out.copied ? `已保存并复制 · ${file}` : `已保存 · ${file}`, { thumb: thumbUrl }]
    : a === 'ocr' ? (chars ? [`已复制图中文字 · ${chars} 字，已加入历史`, { icon: 'check' }] : ['选区里没有识别到文字', { icon: 'ocr' }])
    : ['已钉在屏幕上 · 拖动移动，滚轮缩放，双击关闭', { icon: 'pin' }];
  succeed(gen, msg, lead);
}

/** 动作完成：后端已让遮罩鼠标穿透并交还键盘，其余部分淡出，只留提示条，随后退出。 */
function succeed(gen: number, msg: string, lead: Lead) {
  S.phase = 'done';
  ov.classList.add('out');
  void shotClaim({ generation: gen, index: S.index, state: 'done' });
  toast(msg, lead);
  const exit = reduced.matches ? 0 : 160;
  toastT = window.setTimeout(() => (toastEl.dataset.open = 'false'), TOAST_MS - exit);
  endT = window.setTimeout(() => { release(); void shotCancel(gen); }, TOAST_MS);
}

/* —— 译文卡片 —— */
/** 识别出文字后翻译失败时出卡片的错误类别（没配置在识别前就报，仍回到选区） */
const TR_FAIL = new Set(['translate', 'translatePack', 'translateOs']);

/** 按 tr 重画卡片内容；另一种能用时就有换用按钮，翻译中也有（M26 规则 16）。焦点与滚动位置保留。 */
function renderCard() {
  if (!tr || card.hidden) return;
  const other = engines ? otherEngine(tr.engine, engines) : null;
  // 截图里不给“下载语言包”“去设置”（M26 规则 13）：系统的下载确认挂不到截图遮罩上，遮罩一退出它就既不弹出也不返回；
  // 缺语言包的原因写在卡片上，AI 能用时还有“换用 AI 翻译”
  const focused = (document.activeElement as HTMLElement | null)?.dataset.act;
  const old = cardBody();
  // 原来在底部（翻译中跟着最新一段）的，换成完整译文后仍到底；否则保留滚动位置
  const atEnd = !old || old.scrollTop + old.clientHeight >= old.scrollHeight - 2, top = old?.scrollTop ?? 0;
  card.innerHTML = cardInner({ ...tr, other, errorKind: undefined });
  if (focused) card.querySelector<HTMLElement>(`[data-act="${focused}"]`)?.focus();
  const b = cardBody();
  if (b) b.scrollTop = tr.kind === 'busy' || atEnd ? b.scrollHeight : top;
}

/** 第一段译文到达（或识别后翻译失败）：工具栏与进行中提示隐藏，卡片在选区旁出现，位置只定这一次。宽 240–360。 */
function showCard() {
  const sel = S.sel!;
  S.phase = 'card';
  update(); setCursor();
  Object.assign(card.style, { width: clamp(sel.w, 240, 360) + 'px', maxHeight: '' });
  card.hidden = false;
  renderCard();
  const p = placeCard(sel, { w: card.offsetWidth, h: card.offsetHeight }, W, H);
  // 之后随内容变高，最高到屏底，超出时卡片内滚动
  Object.assign(card.style, { left: p.x + 'px', top: p.y + 'px', maxHeight: H - p.y - 8 + 'px' });
  restart(card, 'pop-in');
}

function doneTr(t: ShotTr, out: ShotOutcome) {
  Object.assign(t, { kind: 'done', text: out.text ?? '', engine: out.engine ?? t.engine });
  if (S.phase === 'busy') showCard(); else renderCard();
}

function failTr(t: ShotTr, e: AppError) {
  Object.assign(t, { kind: 'fail', error: e?.message || '翻译失败', errorKind: e?.kind });
  if (S.phase === 'busy') showCard(); else renderCard();
}

void onShotTranslateDelta((d, engine) => {
  if (tr?.kind !== 'busy' || !live(tr.gen) || staleDelta(engine, tr.want)) return;
  const placeholder = !tr.text;
  tr.text += d;
  tr.engine = engine;
  if (S.phase === 'busy') return showCard();
  if (S.phase !== 'card') return;
  if (placeholder) return renderCard();
  const b = cardBody();
  if (b) { b.append(d); b.scrollTop = b.scrollHeight; }
});

/** 用另一种重翻同一段原文，不再识别，翻译中也行（后端中止进行中的请求）；输出仍被这次翻译占着（M26 规则 16） */
async function again(engine: Engine) {
  const t = tr;
  if (!t || S.phase !== 'card') return;
  const gen = S.gen, seq = (t.seq = ++trSeq);
  Object.assign(t, { kind: 'busy', text: '', error: undefined, errorKind: undefined, want: engine, engine: engine === 'system' ? 'system' : `openai:${engines?.model ?? ''}` });
  renderCard();
  const stale = () => tr !== t || t.seq !== seq || !live(gen);
  try {
    const out = await shotTranslateAgain(gen, engine);
    if (!stale()) doneTr(t, out);
  } catch (e) {
    if (!stale()) failTr(t, e as AppError);
  }
}

/** 复制译文（回车或按钮）：写剪切板进历史，然后走成功收尾。 */
async function copyTranslation() {
  if (S.phase !== 'card' || tr?.kind !== 'done') return;
  const gen = S.gen, text = tr.text;
  S.phase = 'done';
  try {
    await shotCopyText(gen, text);
  } catch (e) {
    if (!live(gen)) return;
    S.phase = 'card';
    toast((e as AppError)?.message || '复制失败', { icon: 'close', danger: true });
    toastT = window.setTimeout(() => (toastEl.dataset.open = 'false'), 2800);
    return;
  }
  if (live(gen)) succeed(gen, `已复制译文 · ${[...text].length} 字，已加入历史`, { icon: 'check' });
}

card.addEventListener('click', (e) => {
  const b = (e.target as Element).closest<HTMLElement>('button[data-act]');
  if (!b || b.getAttribute('aria-disabled') === 'true') return;
  switch (b.dataset.act) {
    case 'close': return closeOverlay();
    case 'copy': return void copyTranslation();
    case 'switch': return void again(b.dataset.engine as Engine);
  }
});

/* —— 录屏：倒计时 3、2、1，数到 0 开始录制（M24） —— */
function startCountdown() {
  if (S.phase !== 'selected' || !S.sel) return;
  S.phase = 'countdown';
  update();
  const gen = S.gen, s = S.sel;
  Object.assign(count.style, { left: s.x + s.w / 2 - 75 + 'px', top: s.y + s.h / 2 - 75 + 'px' });
  count.hidden = false;
  let n = 3;
  const tick = () => {
    if (S.phase !== 'countdown' || gen !== S.gen) return;
    if (n === 0) { count.hidden = true; void startRecording(); return; }
    count.innerHTML = `<svg viewBox="0 0 150 150" aria-hidden="true"><circle cx="75" cy="75" r="68"/><circle class="prg" cx="75" cy="75" r="68"/></svg><b>${n}</b><small>按 Esc 取消</small>`;
    n--;
    cdT = window.setTimeout(tick, 1000);
  };
  tick();
}

/** 后端开始录屏前就结束这次截图、隐藏遮罩；这里同时淡出。失败时只有会话仍在（错误出在结束会话之前）才回到选区并提示，
 * 会话已被后端结束（开始期间按了停止、退出）时静默放掉像素。 */
async function startRecording() {
  const gen = S.gen, sel = { ...S.sel! };
  S.phase = 'done';
  ov.classList.add('out');
  try {
    await recStart({ generation: gen, index: S.index, rect: sel, systemAudio: S.sys, microphone: !!S.mic });
  } catch {
    if (!live(gen)) return;
    if (!(await shotAlive(gen).catch(() => false))) { if (live(gen)) release(); return; }
    if (!live(gen)) return;
    ov.classList.remove('out');
    S.phase = 'selected';
    update();
    toast('录屏没有开始', { icon: 'close', danger: true });
    toastT = window.setTimeout(() => (toastEl.dataset.open = 'false'), 2800);
    return;
  }
  if (live(gen)) release();
}

/* —— 长截图（M23 规则 13）：遮罩撤下，后端在选区四角显示边框、不停截这块区域并拼接，用户自己滚动 —— */
async function startLong() {
  if (S.phase !== 'selected' || !S.sel || S.mode !== 'shot' || S.objs.length) return;
  commitText();
  if (S.objs.length) return;
  const gen = S.gen, sel = { ...S.sel };
  S.phase = 'done';
  update();
  ov.classList.add('out');
  try {
    await shotLongStart({ generation: gen, index: S.index, rect: sel });
  } catch (e) {
    if (!live(gen)) return;
    ov.classList.remove('out');
    S.phase = 'selected';
    update();
    toast((e as AppError)?.message || '长截图没有开始', { icon: 'close', danger: true });
    toastT = window.setTimeout(() => (toastEl.dataset.open = 'false'), 2800);
    return;
  }
  if (live(gen)) release();
}

/* —— 提示条 —— */
type Lead = { thumb?: string; icon?: Icon; danger?: boolean };
function toast(msg: string, lead: Lead) {
  clearTimeout(toastT);
  toastLead.innerHTML = lead.thumb
    ? `<img class="t-th" alt="" src="${lead.thumb}">`
    : `<span class="t-ic${lead.danger ? ' danger' : ''}">${svg(lead.icon ?? 'check')}</span>`;
  toastMsg.textContent = msg;
  toastEl.dataset.open = 'true';
}

/* —— Esc / 右键：有选区先取消选区，没有选区退出；识别文字期间也能退出 —— */
function clearSelection() {
  Object.assign(S, { phase: 'idle', sel: null, objs: [], redo: [], tool: null, drag: null, cur: null });
  S.snap = S.here ? hit(S.rects, S.mx, S.my, W, H) : null;
  renderInk(); syncBar(); setCursor(); update();
}
function escape() {
  if (S.editing) return commitText();
  if (S.phase === 'selected') return clearSelection();
  // 倒计时中右键退出遮罩，不回到选区（对计划的裁定 2）
  if (S.phase === 'idle' || S.phase === 'busy' || S.phase === 'countdown' || S.phase === 'card') closeOverlay();
}

/* —— 键盘（键位对照原型） —— */
addEventListener('keydown', (e) => {
  if (S.phase === 'off' || S.phase === 'done') return;
  // 文字输入框获得焦点时按键由它自己处理；点了颜色粗细后焦点不在框里，Esc 仍提交文字
  if (S.editing) { if (e.key === 'Escape') { e.preventDefault(); commitText(); } return; }
  const mod = isMac ? e.metaKey : e.ctrlKey;
  // Esc 直接退出，有选区也不先取消（M23 规则 8；取消选区用右键）
  if (e.key === 'Escape') { e.preventDefault(); closeOverlay(); return; }
  // 译文卡片：只有回车复制、Esc 关闭；卡片里选中文字后的 Mod + C 照常。焦点在卡片按钮（如“关闭”）上时回车交给按钮
  if (S.phase === 'card') {
    if (e.key === 'Enter' && !(e.target instanceof Element && e.target.closest('button'))) { e.preventDefault(); void copyTranslation(); }
    return;
  }
  if (S.phase !== 'idle' && S.phase !== 'selected') return;
  if (e.key === 'Enter') {
    e.preventDefault();
    if (S.phase === 'selected') { if (S.mode === 'rec') startCountdown(); else void act('copy'); }
    return;
  }
  if (e.code === 'KeyF' && !mod) { e.preventDefault(); selectAll(); return; }
  if (S.phase !== 'selected') return;
  // 录屏没有标注与截图动作，只有方向键微调选区
  if (S.mode === 'rec' && !e.key.startsWith('Arrow')) return;
  if (mod && e.code === 'KeyZ') { e.preventDefault(); if (e.shiftKey) redo(); else undo(); return; }
  if (mod && e.code === 'KeyS') { e.preventDefault(); void act('save'); return; }
  if (!mod && e.code === 'KeyO') { e.preventDefault(); void act('ocr'); return; }
  if (!mod && e.code === 'KeyT') { e.preventDefault(); void act('translate'); return; }
  if (!mod && e.code === 'KeyP') { e.preventDefault(); void act('pin'); return; }
  if (!mod && e.code === 'KeyL') { e.preventDefault(); void startLong(); return; }
  const m = /^Digit([1-7])$/.exec(e.code);
  if (!mod && m) { e.preventDefault(); toggleTool(TOOLS[+m[1] - 1][0], false); return; }
  const dir = ({ ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] } as Record<string, number[]>)[e.key];
  if (dir && S.sel) {
    e.preventDefault();
    const s = S.sel;
    S.sel = e.shiftKey ? resize(s, 'se', dir[0], dir[1], W, H) : { ...s, x: clamp(s.x + dir[0], 0, W - s.w), y: clamp(s.y + dir[1], 0, H - s.h) };
    renderInk(); update();
  }
});

/* —— 启动 —— */
// 别的屏有了选区：放弃这块屏的选区；正在识别文字：这块屏先不能框选；动作完成：这块屏的蒙层一起淡出，随后放掉像素
// （只有动作那块屏调 shot_cancel）
void onShotClaim((c) => {
  if (c.generation !== S.gen || c.index === S.index || S.phase === 'off') return;
  if (c.state === 'done') {
    // 正在识别的屏直接 off：迟到的识别结果不能再起提示条和 shot_cancel 计时
    S.phase = S.phase === 'busy' ? 'off' : 'done';
    ov.classList.add('out');
    endT = window.setTimeout(release, reduced.matches ? 0 : 160);
    return;
  }
  S.locked = c.state === 'busy';
  S.here = false;
  if (S.phase === 'selected') clearSelection();
  else if (S.phase === 'idle') { S.snap = null; S.drag = null; update(); }
});
// 后端结束了这次截图（再按截图快捷键、看门狗等）：窗口已隐藏，立即放掉像素
void onShotEnd((gen) => { if (gen === S.gen && S.phase !== 'off') release(); });
// 心跳：已上屏期间每秒一次，后端 3 s 收不到就撤下遮罩（M23 规则 1）。事件丢了时靠心跳的返回值兜底放掉像素
setInterval(() => {
  const gen = S.gen;
  if (readyGen !== gen || S.phase === 'off') return;
  void shotAlive(gen).then((ok) => { if (!ok && gen === S.gen && S.phase !== 'off') release(); }, () => {});
}, 1000);
// 取帧失败（截图已结束）时什么也不做，后端 3 秒后视为卡住，下次快捷键重新开始
void onShotBegin((p) => void begin(p).catch(() => {}));
