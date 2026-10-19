// 无障碍源文件检查（M21）：只读 index.html 与 styles.css 文本，不碰 DOM。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { contrast } from './color.ts';

const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');

test('every_button_in_index_html_has_a_name', () => {
  const DYNAMIC = new Set(['cfOk', 'obSkip', 'obNext', 'toastAct']); // main.ts 运行时写文字
  const unnamed = [...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)]
    .filter(([, attrs, inner]) => !/aria-label="[^"]+"/.test(attrs) && !inner.replace(/<[^>]*>/g, '').trim()
      && !DYNAMIC.has(/id="([^"]+)"/.exec(attrs)?.[1] ?? ''))
    .map(([m]) => m.slice(0, 80));
  assert.deepEqual(unnamed, []);
});

const block = (start: string) => { const i = css.indexOf(start); assert.ok(i >= 0, start); return css.slice(i, css.indexOf('}', i)); };
const tok = (b: string, n: string) => new RegExp(`(?:^|[\\s;])${n}:\\s*(#[0-9A-Fa-f]{6})`).exec(b)![1];
const NAMES = ['--fg', '--muted', '--faint', '--ink', '--pop-solid'];
test('text_tokens_meet_contrast_in_both_themes', () => {
  const dark = block(':root {'), lightMedia = block(':root:not([data-theme="dark"]) {'), light = block(':root[data-theme="light"] {');
  for (const n of NAMES) assert.equal(tok(lightMedia, n), tok(light, n), `两个亮色块的 ${n} 不一致`);
  for (const [theme, b] of [['暗色', dark], ['亮色', light]] as const)
    for (const bg of ['--ink', '--pop-solid'])
      for (const [fg, min] of [['--fg', 4.5], ['--muted', 4.5], ['--faint', 3]] as const)
        assert.ok(contrast(tok(b, fg), tok(b, bg)) >= min, `${theme} ${fg} 对 ${bg} 低于 ${min}`);
});
