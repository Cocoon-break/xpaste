import { test } from 'node:test';
import assert from 'node:assert/strict';
import { errorControl, tabKeyTarget } from './settings-tabs.ts';

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
  // 其余错误走提示条
  assert.equal(errorControl('invalidSettings', '请输入 2 到 99 之间的整数'), null);
  assert.equal(errorControl('internal', '录屏快捷键'), null);
});
