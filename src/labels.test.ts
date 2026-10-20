import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyVerb, escHint, openLabel } from './labels.ts';

test('copyVerb 按能否粘贴给出动词', () => {
  assert.equal(copyVerb(true), '粘贴');
  assert.equal(copyVerb(false), '复制');
});

test('escHint 按搜索框是否有字给出 Esc 提示', () => {
  assert.equal(escHint(true), '清空搜索');
  assert.equal(escHint(false), '隐藏');
});

test('openLabel 按类型与平台给出右键菜单的打开动作', () => {
  assert.equal(openLabel('link', false, 'macos'), '在浏览器中打开');
  assert.equal(openLabel('image', true, 'windows'), '在资源管理器中显示');
  assert.equal(openLabel('image', false, 'macos'), null);
  assert.equal(openLabel('text', false, 'macos'), null);
});
