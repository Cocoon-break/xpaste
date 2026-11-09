import { test } from 'node:test';
import assert from 'node:assert/strict';
import { keyAction, type KeyCtx, type KeyEv } from './keymap.ts';

const ctx = (o: Partial<KeyCtx> = {}): KeyCtx => ({
  inSearch: true, onControl: false, searchHasText: false, menuOpen: false, hasSel: true, canUndo: false, dialogOpen: false, selIsSnip: false, inSnip: false, multi: 0, selTranslatable: false, selPlain: false, compact: false, peek: false, filtered: false, ...o,
});
const ev = (o: Partial<KeyEv>): KeyEv => ({ key: '', code: '', mod: false, alt: false, shift: false, repeat: false, ...o });

test('↓ 按住时 instant', () =>
  assert.deepEqual(keyAction(ev({ key: 'ArrowDown', repeat: true }), ctx()), { t: 'move', d: 1, instant: true }));

test('Mod+3 复制第 3 条', () =>
  assert.deepEqual(keyAction(ev({ code: 'Digit3', mod: true }), ctx()), { t: 'copyIndex', i: 2 }));

test('Backspace 搜索框有字时不删记录', () =>
  assert.equal(keyAction(ev({ key: 'Backspace' }), ctx({ searchHasText: true })), null));

test('单独 Backspace 搜索框为空时也不删记录', () =>
  assert.equal(keyAction(ev({ key: 'Backspace' }), ctx()), null));

test('Mod+Backspace 搜索框为空时删除', () =>
  assert.deepEqual(keyAction(ev({ key: 'Backspace', mod: true }), ctx()), { t: 'delete' }));

test('Mod+Backspace 搜索框有字时不删记录', () =>
  assert.equal(keyAction(ev({ key: 'Backspace', mod: true }), ctx({ searchHasText: true })), null));

test('Delete 搜索框为空时删除', () =>
  assert.deepEqual(keyAction(ev({ key: 'Delete' }), ctx()), { t: 'delete' }));

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
  assert.equal(keyAction(ev({ key: 'Backspace', mod: true, repeat: true }), ctx()), null);
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

test('⌃ + 字母不进搜索框，⌃ + Enter 不复制（macOS 上 ⌃ 不是 Mod）', () => {
  const base = { key: '', code: '', mod: false, alt: false, shift: false, repeat: false };
  const ctx = { inSearch: false, onControl: false, searchHasText: false, menuOpen: false, hasSel: true, canUndo: false, dialogOpen: false, selIsSnip: false, inSnip: false, multi: 0, selTranslatable: false, selPlain: false, compact: false, peek: false, filtered: false };
  assert.equal(keyAction({ ...base, key: 'a', code: 'KeyA', ctrl: true }, ctx), null);
  assert.deepEqual(keyAction({ ...base, key: 'a', code: 'KeyA' }, ctx), { t: 'focusSearch' });
  assert.equal(keyAction({ ...base, key: 'Enter', code: 'Enter', ctrl: true }, ctx), null);
  assert.deepEqual(keyAction({ ...base, key: 'Enter', code: 'Enter' }, ctx), { t: 'copy' });
});

test('片段键位', () => {
  const k = (o: Partial<KeyEv>, c: Partial<KeyCtx> = {}) => keyAction(ev(o), ctx(c));
  assert.deepEqual(k({ key: 's', mod: true }), { t: 'saveSnippet' });
  assert.equal(k({ key: 's', mod: true }, { selIsSnip: true }), null);
  assert.deepEqual(k({ key: 'e', mod: true }, { selIsSnip: true }), { t: 'editSnippet' });
  assert.equal(k({ key: 'e', mod: true }, { selIsSnip: true, multi: 2 }), null);
  assert.deepEqual(k({ key: 'n', mod: true }), { t: 'newSnippet' });
  assert.deepEqual(k({ key: 'a', mod: true }, { inSnip: true }), { t: 'selectAll' });
  assert.equal(k({ key: 'a', mod: true }, { inSnip: true, searchHasText: true }), null);
  assert.deepEqual(k({ key: 'ArrowDown', shift: true }, { inSnip: true }), { t: 'extend', d: 1 });
  assert.deepEqual(k({ key: 'ArrowDown', shift: true }), { t: 'move', d: 1, instant: false });
  assert.deepEqual(k({ key: 'Escape' }, { multi: 2, searchHasText: true }), { t: 'clearMulti' });
  for (const o of [{ key: 's', mod: true }, { key: 'n', mod: true }, { key: 'Escape' }])
    assert.equal(k(o, { dialogOpen: true, multi: 2 }), null);
});

test('Mod+T 翻译', () => {
  const t = ev({ key: 't', code: 'KeyT', mod: true });
  assert.deepEqual(keyAction(t, ctx({ hasSel: true, selTranslatable: true })), { t: 'translate' });
  assert.equal(keyAction({ ...t, repeat: true }, ctx({ hasSel: true, selTranslatable: true })), null);
  assert.equal(keyAction(t, ctx({ hasSel: true, selTranslatable: false })), null);
  assert.equal(keyAction(t, ctx({ hasSel: true, selTranslatable: true, multi: 2 })), null);
  assert.equal(keyAction(t, ctx({ hasSel: true, selTranslatable: true, dialogOpen: true })), null);
});

// 精简模式（M27 规则 7–9）
const cx = (o: Partial<KeyCtx> = {}) => ctx({ compact: true, ...o });

test('精简：搜索框为空按空格开关预览；有字时空格照常输入', () => {
  assert.deepEqual(keyAction(ev({ key: ' ' }), cx()), { t: 'peek' });
  assert.equal(keyAction(ev({ key: ' ' }), cx({ searchHasText: true })), null);
  assert.deepEqual(keyAction(ev({ key: ' ' }), cx({ peek: true })), { t: 'peek' });
  // 预览开着、搜索框有字时空格也照常输入（M27 规则 8），不关预览
  assert.equal(keyAction(ev({ key: ' ' }), cx({ peek: true, searchHasText: true })), null);
  assert.equal(keyAction(ev({ key: ' ' }), cx({ inSearch: false, onControl: true })), null);
});

test('精简：Mod+Y 有字没字都开关预览', () => {
  assert.deepEqual(keyAction(ev({ key: 'y', mod: true }), cx()), { t: 'peek' });
  assert.deepEqual(keyAction(ev({ key: 'y', mod: true }), cx({ searchHasText: true })), { t: 'peek' });
});

test('精简：Esc 先关预览，再清搜索，再回到全部，最后隐藏', () => {
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), cx({ peek: true, searchHasText: true, multi: 2 })), { t: 'peekClose' });
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), cx({ searchHasText: true, filtered: true })), { t: 'clearSearch' });
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), cx({ filtered: true })), { t: 'clearFilter' });
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), cx()), { t: 'hide' });
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), cx({ peek: true, menuOpen: true })), { t: 'menuClose' });
});

test('精简：搜索框为空按 ⌫ 去掉分类标签', () => {
  assert.deepEqual(keyAction(ev({ key: 'Backspace' }), cx({ filtered: true })), { t: 'clearFilter' });
  assert.equal(keyAction(ev({ key: 'Backspace' }), cx({ filtered: true, searchHasText: true })), null);
  assert.equal(keyAction(ev({ key: 'Backspace' }), cx()), null);
  assert.deepEqual(keyAction(ev({ key: 'Backspace', mod: true }), cx({ filtered: true })), { t: 'delete' });
});

test('精简：Mod+T 打开预览并翻译', () =>
  assert.deepEqual(keyAction(ev({ key: 't', mod: true }), cx({ selTranslatable: true })), { t: 'peekTranslate' }));

test('完整面板不受精简规则影响', () => {
  assert.deepEqual(keyAction(ev({ key: ' ' }), ctx({ inSearch: false })), { t: 'focusSearch' });
  assert.equal(keyAction(ev({ key: ' ' }), ctx()), null);
  assert.equal(keyAction(ev({ key: 'y', mod: true }), ctx()), null);
  assert.deepEqual(keyAction(ev({ key: 'Escape' }), ctx({ filtered: true })), { t: 'hide' });
  assert.equal(keyAction(ev({ key: 'Backspace' }), ctx({ filtered: true })), null);
  assert.deepEqual(keyAction(ev({ key: 't', mod: true }), ctx({ selTranslatable: true })), { t: 'translate' });
});

test('Shift+Enter：有文字的记录粘贴为纯文本，其余照常粘贴（M09 规则 7）', () => {
  const k = (o: Partial<KeyEv>, c: Partial<KeyCtx> = {}) => keyAction(ev(o), ctx(c));
  assert.deepEqual(k({ key: 'Enter', shift: true }, { selPlain: true }), { t: 'copyPlain' });
  assert.deepEqual(k({ key: 'Enter', shift: true }), { t: 'copy' });
  assert.deepEqual(k({ key: 'Enter' }, { selPlain: true }), { t: 'copy' });
  assert.equal(k({ key: 'Enter', shift: true, repeat: true }, { selPlain: true }), null);
  // 多选时退回 copy，由 copyRecord 忽略（M25 规则 6）
  assert.deepEqual(k({ key: 'Enter', shift: true }, { selPlain: true, multi: 2 }), { t: 'copy' });
});
