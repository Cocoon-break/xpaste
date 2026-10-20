import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyVerb, escHint } from './labels.ts';

test('copyVerb 按能否粘贴给出动词', () => {
  assert.equal(copyVerb(true), '粘贴');
  assert.equal(copyVerb(false), '复制');
});

test('escHint 按搜索框是否有字给出 Esc 提示', () => {
  assert.equal(escHint(true), '清空搜索');
  assert.equal(escHint(false), '隐藏');
});
