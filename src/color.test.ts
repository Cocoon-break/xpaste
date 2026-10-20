import { test } from 'node:test';
import assert from 'node:assert/strict';
import { contrast } from './color.ts';

test('contrast 黑白为 21 且与顺序无关', () => {
  assert.equal(contrast('#FFFFFF', '#000000').toFixed(2), '21.00');
  assert.equal(contrast('#000000', '#FFFFFF').toFixed(2), '21.00');
});
