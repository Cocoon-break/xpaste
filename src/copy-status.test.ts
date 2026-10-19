import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyStatus, needsAccessibility, pasteKey } from './copy-status.ts';

test('pasteKey 按平台给出修饰键', () => {
  assert.equal(pasteKey('macos'), '⌘ + V');
  assert.equal(pasteKey('windows'), 'Ctrl + V');
});

test('copyStatus 文案与规格附录一致', () => {
  assert.equal(copyStatus({ result: 'pasted' }, 'macos'), '已粘贴');
  assert.equal(copyStatus({ result: 'copied' }, 'macos'), '已复制');
  assert.equal(
    copyStatus({ result: 'copiedOnly', reason: 'elevated' }, 'windows'),
    '已复制，在目标应用中按 Ctrl + V 粘贴',
  );
  assert.equal(
    copyStatus({ result: 'copiedOnly', reason: 'needAccessibility' }, 'macos'),
    '已复制，在目标应用中按 ⌘ + V 粘贴',
  );
});

test('只有缺少辅助功能权限时才需要“前往授权”', () => {
  assert.equal(needsAccessibility({ result: 'copiedOnly', reason: 'needAccessibility' }), true);
  assert.equal(needsAccessibility({ result: 'copiedOnly', reason: 'pasteFailed' }), false);
  assert.equal(needsAccessibility({ result: 'pasted' }), false);
});
