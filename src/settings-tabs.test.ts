import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tabKeyTarget } from './settings-tabs.ts';

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
