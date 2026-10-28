import { test } from 'node:test';
import assert from 'node:assert/strict';

import { clampPos, zoomAt } from './zoom.ts';

const WIN = { x: 100, y: 100, w0: 200, h0: 100 };

test('放大一格：×1.1，光标在窗口里的相对位置不变', () => {
  const r = zoomAt(1, -100, { x: 150, y: 150 }, WIN);
  assert.ok(Math.abs(r.k - 1.1) < 1e-9);
  assert.equal(r.w, 220);
  assert.equal(r.h, 110);
  assert.equal((150 - r.x) / r.w, 0.25);
  assert.equal((150 - r.y) / r.h, 0.5);
});

test('缩小一格：÷1.1；结果取整', () => {
  const r = zoomAt(1, 100, { x: 133, y: 117 }, WIN);
  assert.ok(Math.abs(r.k - 1 / 1.1) < 1e-9);
  for (const v of [r.x, r.y, r.w, r.h]) assert.ok(Number.isInteger(v));
  assert.equal(r.w, 182);
});

test('连续缩小停在 20%，连续放大停在 400%', () => {
  let k = 1;
  for (let i = 0; i < 40; i++) k = zoomAt(k, 100, { x: 150, y: 150 }, WIN).k;
  assert.equal(k, 0.2);
  for (let i = 0; i < 40; i++) k = zoomAt(k, -100, { x: 150, y: 150 }, WIN).k;
  assert.equal(k, 4);
});

test('接近 100% 时吸附为 1，尺寸回到原始', () => {
  const r = zoomAt(1.03 / 1.1, -100, { x: 150, y: 150 }, WIN);
  assert.equal(r.k, 1);
  assert.equal(r.w, 200);
  assert.equal(r.h, 100);
  // 从 110% 缩回一格正好落在 1
  assert.equal(zoomAt(1.1, 100, { x: 150, y: 150 }, WIN).k, 1);
});

test('缩放后锚点仍按当前尺寸算：从 200% 起放大', () => {
  // 当前 400×200，光标在右下四分之一处
  const r = zoomAt(2, -100, { x: 400, y: 250 }, WIN);
  assert.equal(r.w, 440);
  assert.equal((400 - r.x) / r.w, 0.75);
  assert.equal((250 - r.y) / r.h, 0.75);
});

test('clampPos：至少留 40 px 在屏内，顶边不出屏', () => {
  const screen = { w: 1440, h: 900 };
  assert.deepEqual(clampPos(-500, 300, 200, 100, screen), { x: 40 - 200, y: 300 });
  assert.deepEqual(clampPos(2000, 300, 200, 100, screen), { x: 1440 - 40, y: 300 });
  assert.deepEqual(clampPos(10, -50, 200, 100, screen), { x: 10, y: 0 });
  assert.deepEqual(clampPos(10, 2000, 200, 100, screen), { x: 10, y: 900 - 40 });
  assert.deepEqual(clampPos(10, 20, 200, 100, screen), { x: 10, y: 20 });
});
