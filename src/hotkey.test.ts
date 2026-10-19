import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hotkeyFromEvent, hotkeyLabel, hotkeyString, parseHotkey } from './hotkey.ts';
import type { KeyEv } from './keymap.ts';

test('默认快捷键的标签', () => {
  assert.equal(hotkeyLabel('CommandOrControl+Shift+V', 'macos'), '⌘ + Shift + V');
  assert.equal(hotkeyLabel('CommandOrControl+Shift+V', 'windows'), 'Ctrl + Shift + V');
  assert.equal(hotkeyLabel('CommandOrControl+Alt+C', 'macos'), '⌘ + ⌥ + C');
});
test('字符串与结构往返，修饰键顺序固定', () => {
  assert.equal(hotkeyString(parseHotkey('shift+alt+CommandOrControl+c')), 'CommandOrControl+Alt+Shift+C');
  assert.equal(hotkeyString({ mod: false, alt: true, shift: false, key: 'F2' }), 'Alt+F2');
});
test('hotkeyFromEvent_rejects_without_mod_or_alt_and_special_keys', () => {
  const ev = (o: Partial<KeyEv>): KeyEv => ({ key: '', code: '', mod: false, alt: false, shift: false, repeat: false, ...o });
  assert.equal(hotkeyFromEvent(ev({ key: 'C', code: 'KeyC', shift: true })), null);           // 只有 Shift
  assert.equal(hotkeyFromEvent(ev({ key: 'Meta', code: 'MetaLeft', mod: true })), null);      // 只有修饰键
  assert.equal(hotkeyFromEvent(ev({ key: ' ', code: 'Space', mod: true })), null);            // 特殊键
  assert.equal(hotkeyFromEvent(ev({ key: 'ç', code: 'KeyC', mod: true, alt: true })), 'CommandOrControl+Alt+C'); // 用 code 不用 key
  assert.equal(hotkeyFromEvent(ev({ key: 'F3', code: 'F3', alt: true })), 'Alt+F3');
});
