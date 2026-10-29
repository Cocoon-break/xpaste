import { test } from 'node:test';
import assert from 'node:assert/strict';
import { frameBox } from './rec-frame-box.ts';

test('frameBox 在选区外 3 px', () => {
  assert.deepEqual(frameBox({ x: 100, y: 50, w: 400, h: 300 }, 1440, 900), { x: 97, y: 47, w: 406, h: 306 });
});

test('frameBox 全屏时收回到屏幕边上', () => {
  assert.deepEqual(frameBox({ x: 0, y: 0, w: 1440, h: 900 }, 1440, 900), { x: 0, y: 0, w: 1440, h: 900 });
});

test('frameBox 只收回贴边的那几边', () => {
  assert.deepEqual(frameBox({ x: 0, y: 200, w: 500, h: 700 }, 1440, 900), { x: 0, y: 197, w: 503, h: 703 });
});
