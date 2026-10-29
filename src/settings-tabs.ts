// 设置对话框的标签栏（M15 规则 5）：WAI-ARIA Tabs 的方向键、保存失败时该切到哪个控件。纯函数。

/** 标签栏内按键 → 目标标签下标：←/→ 相邻（首尾循环），Home / End 首尾；其他键 null */
export function tabKeyTarget(key: string, i: number, n: number): number | null {
  switch (key) {
    case 'ArrowRight': return (i + 1) % n;
    case 'ArrowLeft': return (i - 1 + n) % n;
    case 'Home': return 0;
    case 'End': return n - 1;
    default: return null;
  }
}

/** 保存失败的错误该写在哪个快捷键录制框下（按控件所在标签显示红点、切换）；其余错误 null，走提示条。 */
export function errorControl(kind: string, message: string): 'hotkey' | 'shotHotkey' | 'recHotkey' | null {
  const invalid = kind === 'invalidSettings';
  if (kind === 'hotkeyTaken') return 'hotkey';
  if (kind === 'shotHotkeyTaken' || (invalid && message.startsWith('截图快捷键'))) return 'shotHotkey';
  if (kind === 'recHotkeyTaken' || (invalid && message.startsWith('录屏快捷键'))) return 'recHotkey';
  return null;
}
