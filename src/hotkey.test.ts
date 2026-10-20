import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hotkeyFromEvent, hotkeyLabel, hotkeyString, parseHotkey, type ModEv } from './hotkey.ts';

test('默认快捷键的标签', () => {
  assert.equal(hotkeyLabel('CommandOrControl+Shift+V', 'macos'), '⌘ + Shift + V');
  assert.equal(hotkeyLabel('CommandOrControl+Shift+V', 'windows'), 'Ctrl + Shift + V');
  assert.equal(hotkeyLabel('CommandOrControl+Alt+C', 'macos'), '⌘ + ⌥ + C');
});
test('macOS 的 Control 是独立修饰键：标签 ⌃，Windows 上与 Ctrl 合并', () => {
  assert.equal(hotkeyLabel('Control+Z', 'macos'), '⌃ + Z');
  assert.equal(hotkeyLabel('CommandOrControl+Control+Z', 'macos'), '⌘ + ⌃ + Z');
  assert.equal(hotkeyLabel('Control+Z', 'windows'), 'Ctrl + Z');
  assert.equal(hotkeyLabel('CommandOrControl+Control+Z', 'windows'), 'Ctrl + Z');
  assert.equal(hotkeyString(parseHotkey('ctrl+z')), 'Control+Z');
  assert.equal(hotkeyString(parseHotkey('command+control+alt+z')), 'CommandOrControl+Control+Alt+Z');
});
test('字符串与结构往返，修饰键顺序固定', () => {
  assert.equal(hotkeyString(parseHotkey('shift+alt+CommandOrControl+c')), 'CommandOrControl+Alt+Shift+C');
  assert.equal(hotkeyString({ mod: false, ctrl: false, alt: true, shift: false, key: 'F2' }), 'Alt+F2');
});
test('hotkeyFromEvent_rejects_without_mod_or_alt_and_special_keys', () => {
  const ev = (o: Partial<ModEv>): ModEv => ({ key: '', code: '', ctrl: false, meta: false, alt: false, shift: false, ...o });
  assert.equal(hotkeyFromEvent(ev({ key: 'C', code: 'KeyC', shift: true }), 'macos'), null);           // 只有 Shift
  assert.equal(hotkeyFromEvent(ev({ key: 'Meta', code: 'MetaLeft', meta: true }), 'macos'), null);     // 只有修饰键
  assert.equal(hotkeyFromEvent(ev({ key: ' ', code: 'Space', meta: true }), 'macos'), null);           // 特殊键
  assert.equal(hotkeyFromEvent(ev({ key: 'ç', code: 'KeyC', meta: true, alt: true }), 'macos'), 'CommandOrControl+Alt+C'); // 用 code 不用 key
  assert.equal(hotkeyFromEvent(ev({ key: 'F3', code: 'F3', alt: true }), 'macos'), 'Alt+F3');
});
test('录制时按平台区分 Control 与 Command', () => {
  const ev = (o: Partial<ModEv>): ModEv => ({ key: '', code: '', ctrl: false, meta: false, alt: false, shift: false, ...o });
  // macOS：⌃ Z 不能变成 ⌘ Z（否则会抢走系统撤销）
  assert.equal(hotkeyFromEvent(ev({ key: 'z', code: 'KeyZ', ctrl: true }), 'macos'), 'Control+Z');
  assert.equal(hotkeyFromEvent(ev({ key: 'z', code: 'KeyZ', meta: true }), 'macos'), 'CommandOrControl+Z');
  assert.equal(hotkeyFromEvent(ev({ key: 'z', code: 'KeyZ', ctrl: true, meta: true, shift: true }), 'macos'), 'CommandOrControl+Control+Shift+Z');
  // Windows：Ctrl 就是 Mod；Win 键不参与
  assert.equal(hotkeyFromEvent(ev({ key: 'z', code: 'KeyZ', ctrl: true }), 'windows'), 'CommandOrControl+Z');
  assert.equal(hotkeyFromEvent(ev({ key: 'z', code: 'KeyZ', meta: true }), 'windows'), null);
});
