import { test } from 'node:test';
import assert from 'node:assert/strict';
import { layout, range, scrollTo, topOf } from './virtual-list.ts';

test('layout 累加标题与行高', () => {
  const { cells, total } = layout([{ name: '置顶', rows: [{ id: 1 }] }, { name: '今天', rows: [{ id: 2 }, { id: 3 }] }]);
  assert.deepEqual(cells.map((c) => c.top), [0, 34, 90, 124, 180]);
  assert.equal(total, 236);
  assert.deepEqual(cells.map((c) => (c.kind === 'row' ? c.index : -1)), [-1, 0, -1, 1, 2]);
  assert.equal(topOf(cells, 3), 180);
  assert.equal(topOf(cells, 9), undefined);
});

test('range 含上下各 8 行预渲染', () => {
  // cells[0] 是标题；cells[k] 是第 k-1 行，top = 34 + 56 (k-1)
  // scrollTop 5600、视口 560：可见行为 cells[100..=110]，上下各扩 8 → [92, 119)
  const { cells } = layout([{ name: 'g', rows: Array.from({ length: 1000 }, (_, i) => ({ id: i + 1 })) }]);
  assert.deepEqual(range(cells, 5600, 560), [92, 119]);
  assert.deepEqual(range(cells, 0, 560), [0, 19]); // 可见 cells[0..=10]（cells[10].top = 538），向下扩 8
  assert.deepEqual(range([], 0, 560), [0, 0]);
});

test('scrollTo 最小滚动', () => {
  assert.equal(scrollTo(1000, 56, 0, 560), 1056 - 560);
  assert.equal(scrollTo(100, 56, 500, 560), 100);
  assert.equal(scrollTo(600, 56, 500, 560), 500);
});
