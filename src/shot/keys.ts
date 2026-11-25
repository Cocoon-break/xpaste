// 截图遮罩的键盘映射：按键 + 此刻的状态 → 动作（键位对照原型）。纯函数。
import type { BeginPayload, ShotAction } from '../api/types';

export type ShotCtx = {
  phase: 'off' | 'idle' | 'selected' | 'countdown' | 'busy' | 'card' | 'done';
  mode: BeginPayload['mode'];
  /** 正在输入文字标注 */
  typing: boolean;
  /** 改字可用 */
  edit: boolean;
  /** 有“用刚复制的图”卡片 */
  clip: boolean;
  /** 焦点在按钮上（译文卡片的回车交给它） */
  onButton: boolean;
};
export type ShotKey = { key: string; code: string; mod: boolean; shift: boolean };
export type ShotKeyAct =
  /** 收下文字标注 */
  | { t: 'commit' }
  | { t: 'close' }
  | { t: 'copyTranslation' }
  | { t: 'act'; a: ShotAction }
  /** 录屏开始 */
  | { t: 'start' }
  | { t: 'all' }
  | { t: 'clip' }
  | { t: 'undo' }
  | { t: 'redo' }
  | { t: 'long' }
  | { t: 'tool'; i: number }
  | { t: 'nudge'; dx: number; dy: number; resize: boolean }
  /** 只拦下，不做事 */
  | { t: 'none' };

const ACTS: Record<string, ShotAction> = { KeyO: 'ocr', KeyT: 'translate', KeyP: 'pin' };
const DIR: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };

export function shotKey(e: ShotKey, c: ShotCtx): ShotKeyAct | null {
  if (c.phase === 'off' || c.phase === 'done') return null;
  // 文字输入框获得焦点时按键由它自己处理；点了颜色粗细后焦点不在框里，Esc 仍提交文字
  if (c.typing) return e.key === 'Escape' ? { t: 'commit' } : null;
  // Esc 直接退出，有选区也不先取消（M23 规则 8；取消选区用右键）
  if (e.key === 'Escape') return { t: 'close' };
  // 译文卡片：只有回车复制、Esc 关闭；卡片里选中文字后的 Mod + C 照常。焦点在卡片按钮（如“关闭”）上时回车交给按钮
  if (c.phase === 'card') return e.key === 'Enter' && !c.onButton ? { t: 'copyTranslation' } : null;
  if (c.phase !== 'idle' && c.phase !== 'selected') return null;
  if (e.key === 'Enter') {
    if (c.phase === 'idle') return { t: 'none' };
    return c.mode === 'rec' ? { t: 'start' } : { t: 'act', a: c.mode === 'edit' ? 'edit' : 'copy' };
  }
  if (e.code === 'KeyF' && !e.mod) return { t: 'all' };
  if (c.phase === 'idle') return e.code === 'Space' && !e.mod && c.mode === 'edit' && c.clip ? { t: 'clip' } : null;
  const dir = DIR[e.key];
  if (dir) return { t: 'nudge', dx: dir[0], dy: dir[1], resize: e.shift };
  // 录屏没有标注与截图动作，只有方向键微调选区
  if (c.mode === 'rec') return null;
  if (e.mod) {
    if (e.code === 'KeyZ') return { t: e.shift ? 'redo' : 'undo' };
    return e.code === 'KeyS' ? { t: 'act', a: 'save' } : null;
  }
  if (e.code === 'KeyE' && c.edit) return { t: 'act', a: 'edit' };
  if (e.code === 'KeyL') return { t: 'long' };
  if (ACTS[e.code]) return { t: 'act', a: ACTS[e.code] };
  const m = /^Digit([1-7])$/.exec(e.code);
  return m ? { t: 'tool', i: +m[1] - 1 } : null;
}
