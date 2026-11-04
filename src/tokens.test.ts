import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// 守护「刻度」token（M16）：亮暗两套对比度、两个亮色块一致、已删 token 不回流。
const css = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');

/** 取块内 `--name: #RRGGBB` 纯色 token（rgb(... / a) 跳过）。 */
function hexes(block: string): Record<string, string> {
  const o: Record<string, string> = {};
  for (const m of block.matchAll(/(--[\w-]+):\s*(#[0-9a-fA-F]{6})\b/g)) o[m[1]] = m[2].toUpperCase();
  return o;
}
/** 从 start 之后第一个 `{` 起取配对的 `}` 之间的文本。 */
function blockAt(start: number): string {
  const open = css.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}' && --depth === 0) return css.slice(open + 1, i);
  }
  throw new Error('括号不配对');
}
const dark = hexes(blockAt(css.indexOf(':root {')));
const lightB = hexes(blockAt(css.indexOf(':root[data-theme="light"] {')));

function lum(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}
function contrast(a: string, b: string): number {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}
const TEXT = ['--fg', '--muted', '--faint', '--c-text', '--c-link', '--c-code', '--c-image', '--c-video', '--c-file', '--c-fav', '--c-snip'];

/** 取块内全部 `--name: value;` 声明（含 rgb()、阴影），值去掉多余空白。 */
function decls(block: string): Record<string, string> {
  const o: Record<string, string> = {};
  for (const m of block.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) o[m[1]] = m[2].replace(/\s+/g, ' ').trim();
  return o;
}

// 跟随系统的亮色块与手动亮色块必须逐条一致，rgb() 与阴影漂移也算。
test('light_blocks_match', () => {
  const a = decls(blockAt(css.indexOf(':root:not([data-theme="dark"])')));
  const b = decls(blockAt(css.indexOf(':root[data-theme="light"] {')));
  assert.ok(Object.keys(a).length > 30, '亮色块解析过少');
  assert.deepEqual(a, b);
});
test('text_contrast_dark', () => {
  for (const k of TEXT) assert.ok(contrast(dark[k], '#252527') >= 4.5, k);
});
test('text_contrast_light', () => {
  for (const k of TEXT) assert.ok(contrast(lightB[k], '#F2F2F4') >= 4.5, k);
});
test('white_on_fills', () => {
  for (const b of [dark, lightB]) for (const k of ['--beam', '--danger']) assert.ok(contrast('#FFFFFF', b[k]) >= 4.5, k);
});
test('no_bundled_fonts_or_removed_tokens', () => {
  for (const s of ['@font-face', '--spectrum', '--glow-a', '--glow-b', '--sp-glow-a', '--sp-glow-b', '--sp-mirror-a', '--sp-under', '--sp-top-mix'])
    assert.ok(!css.includes(s), s);
});

// 守护「刻度」原则 2、4：六个样式表里不出现彩色发光、彩虹、脉冲与已删的内置字体。
const SHEETS = ['./tokens.css', './styles.css', './shot/shot.css', './pin/pin.css', './card/card.css', './rec-frame.css'];
// 只抓模糊半径 > 0 的彩色阴影；`0 0 0 3px …var(--beam)…` 这类焦点圈（规格 7.6 保留）是第 4 个值，不算。
const GLOW = /(box-shadow|filter)\s*:[^;]*(?<![\d.]px |\d )\b0 0 [1-9]\d*(\.\d+)?px[^;]*var\(--(tc|beam|c-[a-z]+|danger)\)/;

test('glow_regex_samples', () => {
  assert.ok(GLOW.test('box-shadow: 0 0 8px var(--tc)'));
  assert.ok(GLOW.test('filter: drop-shadow(0 0 4px var(--c-video))'));
  assert.ok(GLOW.test('box-shadow: 0 1px 2px #000, 0 0 12px color-mix(in srgb, var(--beam) 40%, transparent)'));
  assert.ok(!GLOW.test('box-shadow: 0 0 0 3px color-mix(in srgb, var(--beam) 18%, transparent)'));
  assert.ok(!GLOW.test('box-shadow: inset 0 0 0 1.5px var(--c-image)'));
  assert.ok(!GLOW.test('box-shadow: 0 0 0 2px #fff, 0 1px 4px rgb(0 0 0 / .45)'));
});
test('no_glow_or_rainbow_in_css', () => {
  for (const f of SHEETS) {
    const src = readFileSync(new URL(f, import.meta.url), 'utf8');
    for (const s of ['--spectrum', '--glow-', 'nowPulse', '@keyframes ping', 'Geist', 'Unbounded', 'JetBrains']) assert.ok(!src.includes(s), `${f}: ${s}`);
    const m = src.match(GLOW);
    assert.equal(m, null, `${f}: ${m?.[0]}`);
  }
});

test('搜索框聚焦描边是完整 --beam（M21 无障碍，非文本 3:1）', () => {
  const styles = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  const m = styles.match(/\.search:focus-within\s*\{([^}]*)\}/);
  assert.ok(m);
  assert.match(m[1], /border-color:\s*var\(--beam\)\s*[;}]/);
});

test('截图标注画布字体栈与 --font-ui 一致（M16）', () => {
  const ui = css.match(/--font-ui:\s*([^;]+);/)![1].trim();
  const ink = readFileSync(new URL('./shot/ink.ts', import.meta.url), 'utf8');
  assert.ok(ink.includes(`'${ui}'`), 'ink.ts 的 UI 字体栈与 tokens.css 的 --font-ui 不同');
});
