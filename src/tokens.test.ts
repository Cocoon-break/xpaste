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
const lightA = hexes(blockAt(css.indexOf(':root:not([data-theme="dark"])')));
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

test('light_blocks_match', () => assert.deepEqual(lightA, lightB));
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
