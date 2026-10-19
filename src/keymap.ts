// 键盘映射：按键 + 上下文 → 动作。规则顺序照原型的 keydown 处理器。纯函数。

export type KeyCtx = {
  inSearch: boolean;
  onControl: boolean;
  searchHasText: boolean;
  menuOpen: boolean;
  hasSel: boolean;
  canUndo: boolean;
};

/** `mod = ctrlKey || metaKey` */
export type KeyEv = { key: string; code: string; mod: boolean; alt: boolean; shift: boolean; repeat: boolean };

export type Action =
  | { t: 'menuClose' }
  | { t: 'menuMove'; d: 1 | -1 }
  | { t: 'move'; d: 1 | -1; instant: boolean }
  | { t: 'copy' }
  | { t: 'copyIndex'; i: number }
  | { t: 'pin' }
  | { t: 'filter'; d: 1 | -1 }
  | { t: 'undo' }
  | { t: 'delete' }
  | { t: 'clearSearch' }
  | { t: 'hide' }
  | { t: 'menu' }
  | { t: 'focusSearch' }
  | null;

export const keyAction = (e: KeyEv, c: KeyCtx): Action => {
  const k = e.key, lk = k.toLowerCase();
  const vert = k === 'ArrowDown' ? 1 : k === 'ArrowUp' ? -1 : 0;
  if (c.menuOpen) {
    if (k === 'Escape') return { t: 'menuClose' };
    return vert ? { t: 'menuMove', d: vert } : null;
  }
  if (e.mod && k === ',') return null; // 设置窗口不在本阶段
  if (vert) return { t: 'move', d: vert, instant: e.repeat };
  if (k === 'Enter' && !e.mod && !c.onControl) return { t: 'copy' };
  const digit = /^Digit([1-9])$/.exec(e.code);
  if (digit && e.mod) return { t: 'copyIndex', i: Number(digit[1]) - 1 };
  if (e.mod && lk === 'p') return { t: 'pin' };
  if (e.mod && (k === 'ArrowLeft' || k === 'ArrowRight')) return { t: 'filter', d: k === 'ArrowRight' ? 1 : -1 };
  if (e.mod && !e.shift && lk === 'z' && c.canUndo && !(c.inSearch && c.searchHasText)) return { t: 'undo' };
  if ((k === 'Delete' || k === 'Backspace') && !c.searchHasText && c.hasSel && !c.onControl) return { t: 'delete' };
  if (k === 'Escape') return c.searchHasText ? { t: 'clearSearch' } : { t: 'hide' };
  if ((k === 'ContextMenu' || (e.shift && k === 'F10')) && c.hasSel) return { t: 'menu' };
  if (k.length === 1 && !c.inSearch && !c.onControl && !e.mod && !e.alt) return { t: 'focusSearch' };
  return null;
};
