import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { RecordSummary } from './api/types.ts';
import { applyAdded, applyRemoved, applyUpdated, type Records } from './records.ts';
import { visible } from './view.ts';

const rec = (id: number, copiedAt: number, pinned = false): RecordSummary => ({
  id,
  kind: 'text',
  title: `t${id}`,
  app: null,
  copiedAt,
  uses: 0,
  pinned,
  size: 1,
  lang: null,
  thumb: null,
  durationSec: null, ocrState: 0,
});

test('applyAdded 新增后按复制时间倒序', () => {
  const m: Records = new Map();
  applyAdded(m, rec(1, 10));
  applyAdded(m, rec(2, 20));
  assert.deepEqual(visible(m, 'all', null, 3).map((r) => r.id), [2, 1]);
});

test('applyUpdated 替换已有项，id 不存在时插入', () => {
  const m: Records = new Map([[1, rec(1, 10)]]);
  applyUpdated(m, { ...rec(1, 30), uses: 1 });
  applyUpdated(m, rec(9, 5));
  assert.equal(m.get(1)?.uses, 1);
  assert.deepEqual(visible(m, 'all', null, 3).map((r) => r.id), [1, 9]);
});

test('applyRemoved 删除已有项，不存在的 id 无副作用', () => {
  const m: Records = new Map([[1, rec(1, 10)], [2, rec(2, 20)]]);
  applyRemoved(m, [2, 99]);
  assert.deepEqual([...m.keys()], [1]);
});

test('visible 置顶优先，同时间按 id 倒序', () => {
  const m: Records = new Map();
  for (const r of [rec(1, 50), rec(2, 10, true), rec(3, 50), rec(4, 99)]) applyAdded(m, r);
  assert.deepEqual(visible(m, 'all', null, 3).map((r) => r.id), [2, 4, 3, 1]);
});
