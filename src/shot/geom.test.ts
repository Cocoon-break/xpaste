// 截图遮罩的选区几何（原型 docs/specs/2026-10-28-截图与录屏-原型.html 的 norm / resize / hitWin / place / update / drawLoupe）。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { badgePos, hit, loupePos, norm, placeBar, resize } from './geom.ts';

const W = 1440, H = 900;

test('norm 反向拖动得到正宽高，并夹在屏内', () => {
  assert.deepEqual(norm(300, 200, 100, 50, W, H), { x: 100, y: 50, w: 200, h: 150 });
  assert.deepEqual(norm(1400, 880, 1600, 1000, W, H), { x: 1400, y: 880, w: 40, h: 20 });
  assert.deepEqual(norm(20, 30, -50, -10, W, H), { x: 0, y: 0, w: 20, h: 30 });
});

test('resize 拖 nw 越过对角时翻转，宽高不小于 1', () => {
  const r = { x: 100, y: 100, w: 50, h: 40 };
  assert.deepEqual(resize(r, 'nw', 80, 60, W, H), { x: 150, y: 140, w: 30, h: 20 });
  assert.deepEqual(resize(r, 'nw', 50, 10, W, H), { x: 150, y: 110, w: 1, h: 30 });
  assert.deepEqual(resize(r, 'e', 10, 99, W, H), { x: 100, y: 100, w: 60, h: 40 });
  assert.deepEqual(resize(r, 's', 0, 2000, W, H), { x: 100, y: 100, w: 50, h: 800 });
});

test('hit 命中最前的窗口，点在空白处返回整屏', () => {
  const front = { x: 100, y: 100, w: 200, h: 200 }, back = { x: 50, y: 50, w: 500, h: 500 };
  assert.deepEqual(hit([front, back], 150, 150, W, H), front);
  assert.deepEqual(hit([front, back], 400, 400, W, H), back);
  assert.deepEqual(hit([front, back], 1000, 800, W, H), { x: 0, y: 0, w: W, h: H });
  assert.deepEqual(hit([], 10, 10, W, H), { x: 0, y: 0, w: W, h: H });
});

test('placeBar 下方放得下：放在选区下方、右对齐', () => {
  const p = placeBar({ x: 200, y: 100, w: 600, h: 300 }, { w: 400, h: 42 }, 0, W, H);
  assert.deepEqual(p, { x: 400, y: 408, above: false, inside: false });
});

test('placeBar 下方不够、上方够：above', () => {
  const p = placeBar({ x: 200, y: 400, w: 600, h: 470 }, { w: 400, h: 42 }, 40, W, H);
  assert.deepEqual(p, { x: 400, y: 350, above: true, inside: false });
});

test('placeBar 上下都放不下：放进选区右下角（inside）', () => {
  const p = placeBar({ x: 0, y: 20, w: W, h: 860 }, { w: 400, h: 42 }, 0, W, H);
  assert.deepEqual(p, { x: W - 400 - 8, y: 20 + 860 - 8 - 42, above: false, inside: true });
});

test('placeBar 的 x 夹在 [8, W - w - 8]', () => {
  assert.equal(placeBar({ x: 0, y: 100, w: 100, h: 100 }, { w: 400, h: 42 }, 0, W, H).x, 8);
  assert.equal(placeBar({ x: 1300, y: 100, w: 200, h: 100 }, { w: 400, h: 42 }, 0, W, H).x, W - 400 - 8);
});

test('badgePos 默认在选区左上角上方 30 px', () => {
  assert.deepEqual(badgePos({ x: 200, y: 100, w: 300, h: 200 }, 90, false, W), { x: 200, y: 70 });
});

test('badgePos 工具栏在上方或选区贴顶时放进选区内 (x+6, y+6)', () => {
  assert.deepEqual(badgePos({ x: 200, y: 100, w: 300, h: 200 }, 90, true, W), { x: 206, y: 106 });
  assert.deepEqual(badgePos({ x: 200, y: 10, w: 300, h: 200 }, 90, false, W), { x: 206, y: 16 });
});

test('badgePos 的 x 不超出屏幕右边', () => {
  assert.equal(badgePos({ x: 1400, y: 100, w: 40, h: 40 }, 90, false, W).x, W - 90 - 4);
});

test('loupePos 默认在光标右下，靠右下边缘时翻到左上', () => {
  assert.deepEqual(loupePos(100, 100, W, H), { x: 122, y: 122 });
  assert.deepEqual(loupePos(1400, 850, W, H), { x: 1400 - 22 - 124, y: 850 - 22 - 152 });
});
