// 设置对话框的标签栏（M15 规则 5）：WAI-ARIA Tabs 的方向键、保存失败时该切到哪个控件。纯函数。

type Mods = { meta?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean };

/** 标签栏内按键 → 目标标签下标：←/→ 相邻（首尾循环），Home / End 首尾；带 Mod、Ctrl、Alt 时（⌘ ←/→ 是面板的切换分类）与其他键都是 null */
export function tabKeyTarget(key: string, i: number, n: number, m: Mods = {}): number | null {
  if (m.meta || m.ctrl || m.alt) return null;
  switch (key) {
    case 'ArrowRight': return (i + 1) % n;
    case 'ArrowLeft': return (i - 1 + n) % n;
    case 'Home': return 0;
    case 'End': return n - 1;
    default: return null;
  }
}

/** 同 tabKeyTarget，但跳过隐藏的标签（Windows 上的“录屏”）：shown[j] 为第 j 个标签是否显示，i 与返回值都是在全部标签里的下标 */
export function shownTabTarget(key: string, i: number, shown: boolean[], m: Mods = {}): number | null {
  const idx = shown.flatMap((s, j) => (s ? [j] : []));
  const t = tabKeyTarget(key, idx.indexOf(i), idx.length, m);
  return t === null ? null : idx[t];
}

/** 打开设置时程序聚焦标签压住的焦点环，按下哪些键后照常显示：单按修饰键、带 Mod 或 Ctrl 的组合不算（例如还按着 ⌘ 再按一次 ,） */
export const revealsFocus = (e: { key: string } & Mods): boolean =>
  !['Meta', 'Control', 'Alt', 'Shift'].includes(e.key) && !e.meta && !e.ctrl;

/** 保存失败的错误该写在哪个快捷键录制框下（按控件所在标签显示红点、切换）；其余错误 null，走提示条。 */
export function errorControl(kind: string, message: string): 'hotkey' | 'shotHotkey' | 'recHotkey' | 'trHotkey' | null {
  const invalid = kind === 'invalidSettings';
  if (kind === 'hotkeyTaken') return 'hotkey';
  if (kind === 'shotHotkeyTaken' || (invalid && message.startsWith('截图快捷键'))) return 'shotHotkey';
  if (kind === 'recHotkeyTaken' || (invalid && message.startsWith('录屏快捷键'))) return 'recHotkey';
  if (kind === 'translateHotkeyTaken' || (invalid && message.startsWith('翻译快捷键'))) return 'trHotkey';
  return null;
}
