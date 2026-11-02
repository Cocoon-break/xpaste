import { test } from 'node:test';
import assert from 'node:assert/strict';
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
  assert.equal(errorControl('invalidSettings', '翻译快捷键不能与录屏快捷键相同'), 'trHotkey');
  assert.equal(errorControl('invalidSettings', '翻译快捷键至少需要包含 Mod 或 Alt'), 'trHotkey');
  // 其余错误走提示条
  assert.equal(errorControl('invalidSettings', '请输入 2 到 99 之间的整数'), null);
  assert.equal(errorControl('internal', '录屏快捷键'), null);
});
