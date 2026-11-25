import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { errorControl, revealsFocus, shownTabTarget, tabKeyTarget } from './settings-tabs.ts';

test('←/→ 切到相邻标签，首尾循环', () => {
  assert.equal(tabKeyTarget('ArrowRight', 0, 4), 1);
  assert.equal(tabKeyTarget('ArrowRight', 3, 4), 0);
  assert.equal(tabKeyTarget('ArrowLeft', 2, 4), 1);
  assert.equal(tabKeyTarget('ArrowLeft', 0, 4), 3);
});

test('Home / End 到第一个、最后一个', () => {
  assert.equal(tabKeyTarget('Home', 2, 4), 0);
  assert.equal(tabKeyTarget('End', 1, 4), 3);
});

test('带 Mod、Ctrl、Alt 的方向键不切标签（⌘ ←/→ 是面板的切换分类）', () => {
  for (const m of [{ meta: true }, { ctrl: true }, { alt: true }]) {
    assert.equal(tabKeyTarget('ArrowRight', 0, 4, m), null);
    assert.equal(tabKeyTarget('Home', 2, 4, m), null);
  }
  assert.equal(tabKeyTarget('ArrowRight', 0, 4, { shift: true }), 1);
});

test('跳过隐藏的标签（Windows 上的“录屏”），下标仍按全部标签算', () => {
  // 通用 记录 搜索 截图 [录屏] 翻译
  const win = [true, true, true, true, false, true], mac = win.map(() => true);
  assert.equal(shownTabTarget('ArrowRight', 3, win), 5);
  assert.equal(shownTabTarget('ArrowLeft', 5, win), 3);
  assert.equal(shownTabTarget('ArrowRight', 5, win), 0);
  assert.equal(shownTabTarget('End', 0, win), 5);
  assert.equal(shownTabTarget('ArrowRight', 3, mac), 4);
  assert.equal(shownTabTarget('ArrowRight', 3, win, { meta: true }), null);
  assert.equal(shownTabTarget('Enter', 3, win), null);
});

test('只有导航类按键才显示打开时压住的焦点环', () => {
  for (const k of ['Meta', 'Control', 'Alt', 'Shift']) assert.equal(revealsFocus({ key: k }), false);
  assert.equal(revealsFocus({ key: ',', meta: true }), false);
  assert.equal(revealsFocus({ key: 'ArrowRight', meta: true }), false);
  assert.equal(revealsFocus({ key: 'ArrowRight' }), true);
  assert.equal(revealsFocus({ key: 'Tab', shift: true }), true);
  assert.equal(revealsFocus({ key: 'a' }), true);
});

test('其他键不处理', () => {
  for (const k of ['ArrowUp', 'ArrowDown', 'Enter', ' ', 'Tab', 'Escape', 'a']) assert.equal(tabKeyTarget(k, 1, 4), null);
});

test('保存失败的快捷键错误落到对应录制框（出错标签跟着控件走）', () => {
  assert.equal(errorControl('hotkeyTaken', '该快捷键已被其他应用占用，请换一个'), 'hotkey');
  assert.equal(errorControl('shotHotkeyTaken', '截图快捷键已被其他应用占用，请换一个'), 'shotHotkey');
  assert.equal(errorControl('invalidSettings', '截图快捷键不能与唤起快捷键相同'), 'shotHotkey');
  assert.equal(errorControl('recHotkeyTaken', '录屏快捷键已被其他应用占用，请换一个'), 'recHotkey');
  assert.equal(errorControl('invalidSettings', '录屏快捷键不能与截图快捷键相同'), 'recHotkey');
  assert.equal(errorControl('invalidSettings', '录屏快捷键至少需要包含 Mod 或 Alt'), 'recHotkey');
  assert.equal(errorControl('translateHotkeyTaken', '翻译快捷键已被其他应用占用，请换一个'), 'trHotkey');
  assert.equal(errorControl('mixerHotkeyTaken', '搅拌机快捷键已被其他应用占用，请换一个'), 'mixerHotkey');
  assert.equal(errorControl('invalidSettings', '搅拌机快捷键不能与传送带快捷键相同'), 'mixerHotkey');
  assert.equal(errorControl('invalidSettings', '翻译快捷键不能与录屏快捷键相同'), 'trHotkey');
  assert.equal(errorControl('invalidSettings', '翻译快捷键至少需要包含 Mod 或 Alt'), 'trHotkey');
  assert.equal(errorControl('lineHotkeyTaken', '便签快捷键已被其他应用占用，请换一个'), 'lineHotkey');
  assert.equal(errorControl('invalidSettings', '便签快捷键不能与唤起快捷键相同'), 'lineHotkey');
  // 其余错误走提示条
  assert.equal(errorControl('invalidSettings', '请输入 2 到 99 之间的整数'), null);
  assert.equal(errorControl('internal', '录屏快捷键'), null);
});

test('errorControl：传送带快捷键冲突', () => {
  assert.equal(errorControl('beltHotkeyTaken', '传送带快捷键已被其他应用占用，请换一个'), 'beltHotkey');
  assert.equal(errorControl('invalidSettings', '退回快捷键至少需要包含 Mod 或 Alt'), 'beltBackHotkey');
  assert.equal(errorControl('invalidSettings', '退回快捷键不能与传送带快捷键相同'), 'beltBackHotkey');
});

test('errorControl：改字快捷键冲突', () => {
  assert.equal(errorControl('editHotkeyTaken', '改字快捷键已被其他应用占用，请换一个'), 'editHotkey');
  assert.equal(errorControl('invalidSettings', '改字快捷键不能与表情包快捷键相同'), 'editHotkey');
  assert.equal(errorControl('invalidSettings', '改字快捷键至少需要包含 Mod 或 Alt'), 'editHotkey');
  assert.equal(errorControl('invalidSettings', '改字方式只能是只在本机改或交给生图接口'), null);
});

// —— 设置 → Pro → 改字 ——（只读 index.html、main.ts 的文本，同 a11y.test.ts）
const read = (f: string) => readFileSync(new URL(f, import.meta.url), 'utf8');
const html = read('../index.html'), main = read('./main.ts');
const proPanel = /<section[^>]*id="st-pro"[\s\S]*?<\/section>/.exec(html)![0];

test('改字只在“Pro”标签里：概览一行加一个子页，没有 Pro 时整个标签不出现', () => {
  for (const id of ['proFeatEdit', 'proEditPage', 'editOn', 'editHotkey', 'editMode', 'editApi', 'editRecent']) {
    assert.ok(proPanel.includes(`id="${id}"`), `“Pro”标签里缺少 #${id}`);
    assert.equal(html.split(`id="${id}"`).length, 2, `#${id} 不止一个`);
  }
  assert.match(proPanel, /data-pro-go="proEditPage"[^>]*>[\s\S]*?<b>改字<\/b>/);
  assert.match(html, /id="st-tab-pro"[^>]* hidden>/);
  assert.match(proPanel, /^<section[^>]* hidden>/);
  assert.ok(main.includes("$('#st-tab-pro').hidden = $('#st-pro').hidden = !S.line.available;"));
});

test('“怎么改”是同一组单选：方向键由浏览器切换', () => {
  const group = /<div class="tr-pick" id="editMode" role="radiogroup" aria-labelledby="editModeLabel">[\s\S]*?<\/div>/.exec(proPanel)![0];
  assert.deepEqual([...group.matchAll(/<input type="radio" name="editMode" value="(\w+)">/g)].map((m) => m[1]), ['local', 'ai']);
  assert.ok(!/tabindex|preventDefault/.test(group));
});

test('清空“最近改过”之前先确认', () => {
  assert.equal(main.split('teRecentClear()').length, 2, '只有一处会清空');
  assert.match(main, /if \(!\(await confirm\(editClearAsk\(n\)\)\)\) return;[\s\S]{0,200}teRecentClear\(\)/);
});
