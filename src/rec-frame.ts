// 录屏边框页面（rec-frame.html），后端只建一个窗口、铺满被录的那块屏（M24）；长截图也用它（M23 规则 13）。
// 收到 `rec-frame`：有位置就在选区四角画直角并淡入，`null` 淡出（之后后端隐藏窗口）。
// 收到 `long-shot`：这是长截图，四角换成图片类型色，选区旁显示已拼接的高度与操作提示，另一侧是拼接结果的预览
// （最下面最新接上的部分），高度或状态变了就重新拉一次。
import './rec-frame.css';
import { longPreview, onFrameToast, onLongShot, onRecFrame, type LRect } from './api/shot';
import { frameBox, infoSide, longTip, previewBox, PREVIEW_W, type Box } from './rec-frame-box.ts';

const frame = document.getElementById('frame')!, info = document.getElementById('info')!;
const toast = document.getElementById('toast')!;
const pv = document.getElementById('pv')!, pvc = document.getElementById('pvc') as HTMLCanvasElement;
/** 预览框的位置与最大高度（选区出现时定一次） */
let pvBox: Box | null = null;
/** 当前显示的位置；推送与拉取可能各到一次，同一位置不重播淡入 */
let shown: string | null = null;
/** 选区矮于这么多逻辑像素时，提示慢一点滚：甩一下就超过整个选区，接不上（M23 规则 13） */
const SHORT = 250;
let short = false;

void onRecFrame((r: LRect | null) => {
  const key = r && [r.x, r.y, r.w, r.h].join(',');
  if (key === shown) return;
  shown = key;
  if (!r) {
    frame.classList.add('out');
    pv.classList.add('out');
    return;
  }
  short = r.h < SHORT;
  const b = frameBox(r, innerWidth, innerHeight);
  Object.assign(frame.style, { left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
  // 录屏与长截图共用窗口：新的一次先回到录屏的样子，长截图的进度随后才到
  delete frame.dataset.kind;
  info.hidden = true;
  info.dataset.side = infoSide(b, innerHeight);
  pv.hidden = true;
  toast.dataset.open = 'false';
  pvBox = previewBox(b, innerWidth, innerHeight);
  Object.assign(pv.style, { left: `${pvBox.x}px`, top: `${pvBox.y}px`, maxHeight: `${pvBox.h}px` });
  pvKey = '';
  frame.classList.remove('out');
  // 重播淡入：窗口复用，上次录制的动画早已播完
  frame.style.animation = 'none';
  void frame.offsetWidth;
  frame.style.animation = '';
});

/** 预览：同一高度与状态只拉一次；拉取中又有变化时，拉完再拉一次 */
let pvKey = '', pvBusy = false, pvAgain = false;
async function refreshPreview(key: string) {
  if (key === pvKey || !pvBox) return;
  if (pvBusy) { pvAgain = true; return; }
  pvKey = key;
  pvBusy = true;
  try {
    const r = devicePixelRatio || 1;
    // 减去标题行与描边（约 25 px），最下面最新的部分不能被裁掉
    const f = await longPreview(PREVIEW_W * r, (pvBox.h - 28) * r);
    pvc.width = f.w; pvc.height = f.h;
    pvc.style.height = `${f.h / r}px`;
    pvc.getContext('2d')!.putImageData(new ImageData(f.rgba, f.w, f.h), 0, 0);
    if (pv.hidden) {
      pv.hidden = false;
      pv.classList.remove('out');
    }
  } catch {
    // 长截图已结束：边框随后淡出
  } finally {
    pvBusy = false;
    if (pvAgain) { pvAgain = false; pvKey = ''; void refreshPreview(curKey); }
  }
}
let curKey = '';

void onLongShot((p) => {
  frame.dataset.kind = 'long';
  pv.dataset.state = p.state;
  curKey = `${p.height}/${p.state}`;
  void refreshPreview(curKey);
  info.hidden = false;
  info.dataset.state = p.state;
  const tip = longTip(p.state, short);
  info.innerHTML = `<b>长截图</b><span class="px">${p.height} px</span><span class="tip">${tip}</span><kbd>↩</kbd>完成<kbd>Esc</kbd>取消`;
});

/* 结果提示条：边框与预览淡出，提示条在左下角淡入；停留后后端隐藏窗口。
   进行中（`busy`，长截图生成中，M23 规则 13）转圈、不自动收起，下一条提示到来时替换。 */
let toastT = 0;
const CHECK = '<path d="M5 12.5l4.5 4.5L19 7.5"/>', CROSS = '<path d="M6 6l12 12M18 6 6 18"/>';
void onFrameToast((t) => {
  pv.classList.add('out');
  document.getElementById('toastIc')!.innerHTML = t.busy
    ? '<span class="spin" aria-hidden="true"></span>'
    : `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${t.ok ? CHECK : CROSS}</svg>`;
  toast.classList.toggle('danger', !t.ok);
  document.getElementById('toastMsg')!.textContent = t.text;
  toast.dataset.open = 'true';
  // 录制中的提示不隐藏窗口，自己收起；结果提示到点后窗口随之隐藏
  clearTimeout(toastT);
  if (!t.busy) toastT = window.setTimeout(() => (toast.dataset.open = 'false'), 2400);
});
