// 无障碍源文件检查（M21）：只读 index.html、tokens.css 与 styles.css 文本，不碰 DOM。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { contrast } from './color.ts';

const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const css = ['./tokens.css', './styles.css'].map((f) => readFileSync(new URL(f, import.meta.url), 'utf8')).join('\n');

test('every_button_in_index_html_has_a_name', () => {
  const DYNAMIC = new Set(['cfOk', 'obSkip', 'obNext', 'toastAct', 'mOpen']); // main.ts 运行时写文字（按钮或其内文字 span 的 id）
  const unnamed = [...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)]
    .filter(([, attrs, inner]) => !/aria-label="[^"]+"/.test(attrs) && !inner.replace(/<[^>]*>/g, '').trim()
      && !DYNAMIC.has(/id="([^"]+)"/.exec(attrs + inner)?.[1] ?? ''))
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

// 片段对话框与菜单项（M25 规则 5、6）
test('snippet_dialog_is_labelled_and_fields_have_labels', () => {
  assert.match(html, /<dialog id="snippet" aria-labelledby="snip-title">/);
  assert.match(html, /<h2 id="snip-title">/);
  for (const id of ['snipName', 'snipText']) {
    assert.match(html, new RegExp(`<label for="${id}">`), `#${id} 缺少 <label for>`);
    assert.match(html, new RegExp(`id="${id}"`), `缺少 #${id}`);
  }
});

test('menu_has_save_and_edit_snippet_items', () => {
  for (const m of ['saveSnippet', 'editSnippet']) assert.match(html, new RegExp(`<button[^>]*role="menuitem"[^>]*data-m="${m}"`), `菜单缺少 ${m}`);
});

// 片段色在文字上用（新建按钮、列表第二行），按正文标准 4.5 : 1 检查
test('snippet_color_meets_contrast_in_both_themes', () => {
  const dark = block(':root {'), lightMedia = block(':root:not([data-theme="dark"]) {'), light = block(':root[data-theme="light"] {');
  assert.equal(tok(lightMedia, '--c-snip'), tok(light, '--c-snip'), '两个亮色块的 --c-snip 不一致');
  for (const [theme, b] of [['暗色', dark], ['亮色', light]] as const)
    for (const bg of ['--ink', '--pop-solid'])
      assert.ok(contrast(tok(b, '--c-snip'), tok(b, bg)) >= 4.5, `${theme} --c-snip 对 ${bg} 低于 4.5`);
});

// 晾衣绳：行上的挂钩按钮与已挂刻线（main.ts 生成），设置里的 Pro 标签
test('line_hang_button_and_hung_tick_are_named', () => {
  const main = readFileSync(new URL('./main.ts', import.meta.url), 'utf8');
  assert.ok(main.includes('data-qa="hang" title="挂到晾衣绳"'), '挂钩按钮缺少 title');
  assert.ok(main.includes('<span class="hung" role="img" aria-label="已挂在晾衣绳上">'), '已挂刻线缺少 aria-label');
  assert.match(html, /<button[^>]*role="tab"[^>]*id="st-tab-pro"[^>]*>Pro<\/button>/);
  for (const id of ['proCode', 'lineHotkey']) assert.match(html, new RegExp(`<label for="${id}">`), `#${id} 缺少 <label for>`);
});

test('line_switch_and_side_picker_are_labelled', () => {
  assert.match(html, /<label class="switch"><span><b>晾衣绳便签<\/b><small id="lineOnHelp"><\/small><\/span><input type="checkbox" id="lineOn"><\/label>/);
  assert.match(html, /<div class="side-pick" id="lineSide" role="radiogroup" aria-labelledby="lineSideLabel" aria-describedby="lineSideHelp lineSideHint">/);
  for (const [v, n] of [['top', '顶部'], ['left', '左侧'], ['right', '右侧']])
    assert.match(html, new RegExp(`<label><input type="radio" name="lineSide" value="${v}">[\\s\\S]*?<span>${n}</span></label>`), `缺少“${n}”`);
});
