// WCAG 对比度：从原型搬运的纯函数（a11y 测试用来检查主题色）。

// WCAG 相对亮度；参数为 `#RRGGBB`。
const luminance = (hex: string): number => {
  const n = parseInt(hex.slice(1), 16), r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
  const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
};

/** 两个 HEX 的 WCAG 对比度：(较亮 + 0.05) / (较暗 + 0.05)。 */
export const contrast = (a: string, b: string): number => {
  const x = luminance(a), y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
};
