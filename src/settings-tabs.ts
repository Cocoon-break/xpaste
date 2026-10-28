// 设置对话框的标签栏（M15 规则 5）：WAI-ARIA Tabs 的方向键。纯函数。

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
