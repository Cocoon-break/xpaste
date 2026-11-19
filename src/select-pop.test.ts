import { test } from 'node:test';
import assert from 'node:assert/strict';
import { moveIndex } from './select-pop.ts';

test('下拉列表的方向键：到头停住，Home / End 跳到两端，别的键不动', () => {
  assert.equal(moveIndex(0, 'ArrowDown', 2), 1);
  assert.equal(moveIndex(1, 'ArrowDown', 2), 1);
  assert.equal(moveIndex(1, 'ArrowUp', 2), 0);
  assert.equal(moveIndex(0, 'ArrowUp', 2), 0);
  // 焦点还不在任何一项上（-1）时按下键落到第一项
  assert.equal(moveIndex(-1, 'ArrowDown', 3), 0);
  assert.equal(moveIndex(1, 'Home', 3), 0);
  assert.equal(moveIndex(0, 'End', 3), 2);
  assert.equal(moveIndex(0, 'Enter', 3), null);
  assert.equal(moveIndex(0, 'ArrowDown', 0), null);
});
