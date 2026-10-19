import { test } from 'node:test';
import assert from 'node:assert/strict';
import { keyAction, type KeyCtx, type KeyEv } from './keymap.ts';

const ctx = (o: Partial<KeyCtx> = {}): KeyCtx => ({
  inSearch: true, onControl: false, searchHasText: false, menuOpen: false, hasSel: true, canUndo: false, dialogOpen: false, ...o,
});
const ev = (o: Partial<KeyEv>): KeyEv => ({ key: '', code: '', mod: false, alt: false, shift: false, repeat: false, ...o });

test('↓ 按住时 instant', () =>
  assert.deepEqual(keyAction(ev({ key: 'ArrowDown', repeat: true }), ctx()), { t: 'move', d: 1, instant: true }));

test('Mod+3 复制第 3 条', () =>
  assert.deepEqual(keyAction(ev({ code: 'Digit3', mod: true }), ctx()), { t: 'copyIndex', i: 2 }));

test('Backspace 搜索框有字时不删记录', () =>
  assert.equal(keyAction(ev({ key: 'Backspace' }), ctx({ searchHasText: true })), null));

test('Backspace 搜索框为空时删除', () =>
  assert.deepEqual(keyAction(ev({ key: 'Backspace' }), ctx()), { t: 'delete' }));

test('Esc 先清搜索再隐藏', () => {
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), ctx({ searchHasText: true })), { t: 'clearSearch' });
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), ctx()), { t: 'hide' });
});

test('Mod+Z 搜索框有字时交给输入框', () => {
  assert.equal(keyAction(ev({ key: 'z', mod: true }), ctx({ canUndo: true, searchHasText: true })), null);
  assert.deepEqual(keyAction(ev({ key: 'z', mod: true }), ctx({ canUndo: true })), { t: 'undo' });
});

test('Mod+Shift+Z 不触发撤销', () =>
  assert.equal(keyAction(ev({ key: 'Z', mod: true, shift: true }), ctx({ canUndo: true })), null));

test('菜单打开时只有 Esc 和 ↑↓', () => {
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), ctx({ menuOpen: true })), { t: 'menuClose' });
  assert.deepEqual(keyAction(ev({ key: 'ArrowUp' }), ctx({ menuOpen: true })), { t: 'menuMove', d: -1 });
  assert.equal(keyAction(ev({ key: 'Enter' }), ctx({ menuOpen: true })), null);
});

test('Shift+F10 打开菜单；焦点在按钮上时字符不转搜索', () => {
  assert.deepEqual(keyAction(ev({ key: 'F10', shift: true }), ctx({ inSearch: false })), { t: 'menu' });
  assert.deepEqual(keyAction(ev({ key: 'a' }), ctx({ inSearch: false })), { t: 'focusSearch' });
  assert.equal(keyAction(ev({ key: 'a' }), ctx({ inSearch: false, onControl: true })), null);
});

test('Mod+→ 切换分类', () =>
  assert.deepEqual(keyAction(ev({ key: 'ArrowRight', mod: true }), ctx()), { t: 'filter', d: 1 }));

test('按住不重复触发删除、复制、置顶', () => {
  assert.equal(keyAction(ev({ key: 'Backspace', repeat: true }), ctx()), null);
  assert.equal(keyAction(ev({ key: 'Delete', repeat: true }), ctx()), null);
  assert.equal(keyAction(ev({ key: 'Enter', repeat: true }), ctx()), null);
  assert.equal(keyAction(ev({ key: 'p', mod: true, repeat: true }), ctx()), null);
});

test('菜单打开时 Tab 关闭菜单', () =>
  assert.deepEqual(keyAction(ev({ key: 'Tab' }), ctx({ menuOpen: true })), { t: 'menuClose' }));

test('按住 Mod+数字不连续复制', () =>
  assert.equal(keyAction(ev({ code: 'Digit3', mod: true, repeat: true }), ctx()), null));

test('dialog_open_blocks_panel_actions', () => {
  const c = ctx({ dialogOpen: true, hasSel: true });
  for (const k of ['Escape', 'Delete', 'Enter']) assert.equal(keyAction(ev({ key: k }), c), null);
  assert.equal(keyAction(ev({ key: '1', code: 'Digit1', mod: true }), c), null);
});

test('Mod+, 打开设置', () => assert.deepEqual(keyAction(ev({ key: ',', mod: true }), ctx()), { t: 'settings' }));
