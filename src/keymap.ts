// 键盘映射：按键 + 上下文 → 动作。规则顺序照原型的 keydown 处理器。纯函数。

export type KeyCtx = {
  inSearch: boolean;
  onControl: boolean;
  searchHasText: boolean;
  menuOpen: boolean;
  hasSel: boolean;
  canUndo: boolean;
  /** 任一 <dialog> 打开：面板按键全部交给对话框（Esc 由对话框的 cancel 处理） */
  dialogOpen: boolean;
  /** 选中的是片段 */
  selIsSnip: boolean;
  /** 当前在“片段”分类 */
  inSnip: boolean;
  /** 多选条数 */
  multi: number;
};

/** `mod` 是平台的 Mod 键：macOS 只认 ⌘（metaKey），Windows 只认 Ctrl；`ctrl` 是字面的 Control 键（macOS 的 ⌃），
 *  只用来排除——⌃ + 字母在 macOS 上是系统 / 文本编辑组合，不当作搜索输入或 Enter 复制 */
export type KeyEv = { key: string; code: string; mod: boolean; ctrl?: boolean; alt: boolean; shift: boolean; repeat: boolean };

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
  | { t: 'settings' }
  | { t: 'saveSnippet' }
  | { t: 'editSnippet' }
  | { t: 'newSnippet' }
  | { t: 'selectAll' }
  | { t: 'extend'; d: 1 | -1 }
  | { t: 'clearMulti' }
  | null;

export const keyAction = (e: KeyEv, c: KeyCtx): Action => {
  const k = e.key, lk = k.toLowerCase();
  const vert = k === 'ArrowDown' ? 1 : k === 'ArrowUp' ? -1 : 0;
  if (c.dialogOpen) return null;
  if (c.menuOpen) {
    if (k === 'Escape' || k === 'Tab') return { t: 'menuClose' };
    return vert ? { t: 'menuMove', d: vert } : null;
  }
  if (e.mod && k === ',') return e.repeat ? null : { t: 'settings' };
  // 片段（设计 §3.4、§3.5）
  if (e.mod && !e.shift && !e.alt && !e.repeat) {
    if (lk === 's' && c.hasSel && !c.selIsSnip) return { t: 'saveSnippet' };
    if (lk === 'e' && c.selIsSnip && c.multi < 2) return { t: 'editSnippet' };
    if (lk === 'n') return { t: 'newSnippet' };
    if (lk === 'a' && c.inSnip && !c.searchHasText) return { t: 'selectAll' };
  }
  if (vert && e.shift && c.inSnip && !e.mod) return { t: 'extend', d: vert };
  if (vert) return { t: 'move', d: vert, instant: e.repeat };
  // 按住不连发：删除、复制（次数 +1）、置顶（来回翻转）都只认第一次按下
  if (k === 'Enter' && !e.mod && !e.ctrl && !c.onControl && !e.repeat) return { t: 'copy' };
  const digit = /^Digit([1-9])$/.exec(e.code);
  if (digit && e.mod) return e.repeat ? null : { t: 'copyIndex', i: Number(digit[1]) - 1 };
  if (e.mod && lk === 'p') return e.repeat ? null : { t: 'pin' };
  if (e.mod && (k === 'ArrowLeft' || k === 'ArrowRight')) return { t: 'filter', d: k === 'ArrowRight' ? 1 : -1 };
  if (e.mod && !e.shift && lk === 'z' && c.canUndo && !(c.inSearch && c.searchHasText)) return { t: 'undo' };
  // 单独 ⌫ 只作用于搜索框；删记录用 ⌦ 或 Mod+⌫
  if (((k === 'Delete' && !e.mod) || (k === 'Backspace' && e.mod)) && !c.searchHasText && c.hasSel && !c.onControl && !e.repeat) return { t: 'delete' };
  if (k === 'Escape' && c.multi >= 2) return { t: 'clearMulti' };
  if (k === 'Escape') return c.searchHasText ? { t: 'clearSearch' } : { t: 'hide' };
  if ((k === 'ContextMenu' || (e.shift && k === 'F10')) && c.hasSel) return { t: 'menu' };
  if (k.length === 1 && !c.inSearch && !c.onControl && !e.mod && !e.ctrl && !e.alt) return { t: 'focusSearch' };
  return null;
};
