// 颜色解析与 WCAG 对比度：从原型搬运的纯函数。

export const isHex = (s: string): boolean => /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(s.trim());

export interface HexInfo {
  hex: string;
  rgb: string;
  hsl: string;
  onWhite: number;
  onBlack: number;
}

/** 调用方先用 `isHex` 校验。 */
export const hexInfo = (h: string): HexInfo => {
  let x = h.trim().slice(1);
  if (x.length === 3) x = [...x].map((c) => c + c).join('');
  x = x.toUpperCase();
  const n = parseInt(x, 16), r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
  const R = r / 255, G = g / 255, B = b / 255;
  const mx = Math.max(R, G, B), mn = Math.min(R, G, B), l = (mx + mn) / 2, d = mx - mn;
  let hh = 0, s = 0;
  if (d) {
    s = d / (1 - Math.abs(2 * l - 1));
    hh = mx === R ? ((G - B) / d) % 6 : mx === G ? (B - R) / d + 2 : (R - G) / d + 4;
    hh = Math.round(hh * 60);
    if (hh < 0) hh += 360;
  }
  const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const L = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return {
    hex: '#' + x,
    rgb: `rgb(${r}, ${g}, ${b})`,
    hsl: `hsl(${hh}, ${Math.round(s * 100)}%, ${Math.round(l * 100)}%)`,
    onWhite: 1.05 / (L + 0.05),
    onBlack: (L + 0.05) / 0.05,
  };
};

export const grade = (c: number): 'AA' | 'AA 大字' | '不达标' => (c >= 4.5 ? 'AA' : c >= 3 ? 'AA 大字' : '不达标');
