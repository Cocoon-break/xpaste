// 截图标注的纯计算（原型的 constrain / drawArrow / 序号编号）。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { arrowPolygon, constrain, nextStep, type Obj } from './ink.ts';

const near = (a: number, b: number) => assert.ok(Math.abs(a - b) < 1e-9, `${a} ≉ ${b}`);

test('constrain 矩形按住 Shift 得到正方形，保留拖动方向的符号', () => {
  assert.deepEqual(constrain({ t: 'rect', x0: 10, y0: 10, x1: 40, y1: 20, shift: true }), { x0: 10, y0: 10, x1: 40, y1: 40 });
  assert.deepEqual(constrain({ t: 'ellipse', x0: 10, y0: 10, x1: -10, y1: 15, shift: true }), { x0: 10, y0: 10, x1: -10, y1: 30 });
  assert.deepEqual(constrain({ t: 'rect', x0: 10, y0: 10, x1: 0, y1: -30, shift: true }), { x0: 10, y0: 10, x1: -30, y1: -30 });
});

test('constrain 不按 Shift 时原样返回', () => {
  assert.deepEqual(constrain({ t: 'rect', x0: 1, y0: 2, x1: 30, y1: 4, shift: false }), { x0: 1, y0: 2, x1: 30, y1: 4 });
});

test('constrain 箭头吸附到 45° 且长度不变', () => {
  const r = constrain({ t: 'arrow', x0: 0, y0: 0, x1: 100, y1: 90, shift: true });
  near(r.x1, r.y1);
  near(Math.hypot(r.x1, r.y1), Math.hypot(100, 90));
  const h = constrain({ t: 'arrow', x0: 0, y0: 0, x1: 100, y1: 10, shift: true });
  near(h.y1, 0);
  near(h.x1, Math.hypot(100, 10));
});

test('arrowPolygon 7 个点：箭头末端 (100,0) 是尖，首末点关于 x 轴对称', () => {
  const p = arrowPolygon(0, 0, 100, 0, 4);
  assert.equal(p.length, 7);
  assert.deepEqual(p[3], [100, 0]);
  near(p[0][0], p[6][0]);
  near(p[0][1], -p[6][1]);
  assert.ok(p[0][1] !== 0);
});

test('arrowPolygon 长度 < 1 返回 []', () => {
  assert.deepEqual(arrowPolygon(5, 5, 5, 5, 4), []);
  assert.deepEqual(arrowPolygon(0, 0, 0.5, 0.5, 4), []);
});

test('nextStep 只数序号标注', () => {
  const objs: Obj[] = [
    { t: 'step', color: '#FF3B47', size: 1, x: 0, y: 0, n: 1 },
    { t: 'rect', color: '#FF3B47', size: 1, x0: 0, y0: 0, x1: 9, y1: 9, shift: false },
    { t: 'step', color: '#FF3B47', size: 1, x: 5, y: 5, n: 2 },
    { t: 'pen', color: '#FF3B47', size: 1, pts: [[0, 0]] },
  ];
  assert.equal(nextStep(objs), 3);
  assert.equal(nextStep([]), 1);
});
