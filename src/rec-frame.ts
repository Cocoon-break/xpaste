// 录屏边框页面（rec-frame.html），后端只建一个窗口、铺满被录的那块屏（M24）。
// 收到 `rec-frame`：有位置就在选区四角画直角并淡入，`null` 淡出（之后后端隐藏窗口）。
import './rec-frame.css';
import { onRecFrame, type LRect } from './api/shot';
import { frameBox } from './rec-frame-box.ts';

const frame = document.getElementById('frame')!;
/** 当前显示的位置；推送与拉取可能各到一次，同一位置不重播淡入 */
let shown: string | null = null;

void onRecFrame((r: LRect | null) => {
  const key = r && [r.x, r.y, r.w, r.h].join(',');
  if (key === shown) return;
  shown = key;
  if (!r) {
    frame.classList.add('out');
    return;
  }
  const b = frameBox(r, innerWidth, innerHeight);
  Object.assign(frame.style, { left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
  frame.classList.remove('out');
  // 重播淡入：窗口复用，上次录制的动画早已播完
  frame.style.animation = 'none';
  void frame.offsetWidth;
  frame.style.animation = '';
});
