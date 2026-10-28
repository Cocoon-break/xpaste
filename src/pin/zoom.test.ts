import { test } from 'node:test';
import assert from 'node:assert/strict';

import { clampPos, wheelSteps, zoomAt, type Wheel } from './zoom.ts';

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

/** 依次喂事件 [deltaY, 时间 ms, deltaMode?]，返回每次的格数 */
function feed(events: [number, number, number?][]): number[] {
  let s: Wheel = { acc: 0, t: -Infinity };
  return events.map(([dy, t, mode]) => {
    const r = wheelSteps(s, dy, mode ?? 0, t, 800);
    s = r.next;
    return r.steps;
  });
}

test('wheelSteps：停顿后的第一下固定一格（鼠标一格就是一格，不论 delta 大小）', () => {
  assert.deepEqual(feed([[4, 0], [-4, 400], [100, 1000], [-1, 2000]]), [1, -1, 1, -1]);
});

test('wheelSteps：连续的小 delta 按距离累计，每 50 px 一格，余数留到下次', () => {
  // 第一下算一格并清零；之后 30 + 30 = 60 → 一格，余 10；再 +40 = 50 → 一格
  assert.deepEqual(feed([[2, 0], [30, 16], [30, 32], [40, 48]]), [1, 0, 1, 1]);
});

test('wheelSteps：触控板一串事件的格数有上限（按距离，不按事件数）', () => {
  // 100 个 3 px 的事件，16 ms 一个：共 300 px → 首格 + 297/50 取整 5 格
  const steps = feed(Array.from({ length: 100 }, (_, i) => [-3, i * 16] as [number, number]));
  assert.equal(steps.reduce((a, b) => a + b, 0), -6);
});

test('wheelSteps：按行 / 按页的 delta 换算成像素；一个事件最多一格', () => {
  // 3 行 = 48 px，不足一格；再 1 行 → 64，一格，余 14
  assert.deepEqual(feed([[1, 0, 1], [3, 10, 1], [1, 20, 1]]), [1, 0, 1]);
  // 一页 = 800 px，也只算一格，余数不超过一格
  assert.deepEqual(feed([[1, 0], [1, 10, 2], [1, 20]]), [1, 1, 0]);
});
