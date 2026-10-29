import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { RecordSummary } from './api/types.ts';
import { rel, type Filter } from './format.ts';
import {
  armUndo, canUndo, counts, cycleFilter, ensureSelection, groupOf, groups, inFilter, isStale, nextSelection, pruneMulti, rangeIds, visible,
} from './view.ts';

const mk = (id: number, over: Partial<RecordSummary>): RecordSummary => ({
  id, kind: 'text', title: '', app: null, copiedAt: 0, uses: 0, pinned: false,
  size: 0, lang: null, thumb: null, durationSec: null, ocrState: 0, snippetBody: null, ...over,
});

test('visible：分类 ∩ 搜索 → 置顶优先、时间倒序', () => {
  const m = new Map([mk(1, { copiedAt: 10 }), mk(2, { copiedAt: 20, pinned: true }), mk(3, { copiedAt: 30, kind: 'code' })].map((r) => [r.id, r]));
  assert.deepEqual(visible(m, 'all', null, 3).map((r) => r.id), [2, 3, 1]);
  assert.deepEqual(visible(m, 'code', null, 3).map((r) => r.id), [3]);
  assert.deepEqual(visible(m, 'all', new Set([1]), 3).map((r) => r.id), [1]);
});

test('常用视图按次数降序', () => {
  const m = new Map([mk(1, { uses: 3, copiedAt: 1 }), mk(2, { uses: 5 }), mk(3, { uses: 2 })].map((r) => [r.id, r]));
  assert.deepEqual(visible(m, 'fav', null, 3).map((r) => r.id), [2, 1]);
});

test('常用阈值可配置：阈值 2 时 uses = 2 进入常用，阈值 3 时不进入', () => {
  const m = new Map([mk(1, { uses: 2 }), mk(2, { uses: 1 })].map((r) => [r.id, r]));
  assert.deepEqual(visible(m, 'fav', null, 2).map((r) => r.id), [1]);
  assert.deepEqual(visible(m, 'fav', null, 3).map((r) => r.id), []);
  assert.equal(counts(m, 2).fav, 1);
  assert.equal(counts(m, 3).fav, 0);
});

test('groupOf 今天/昨天/更早/置顶', () => {
  const now = new Date(2026, 9, 19, 12).getTime();
  assert.equal(groupOf(mk(1, { copiedAt: now - 3_600_000 }), 'all', now), '今天');
  assert.equal(groupOf(mk(1, { copiedAt: new Date(2026, 9, 18, 23).getTime() }), 'all', now), '昨天');
  assert.equal(groupOf(mk(1, { copiedAt: now - 3 * 86_400_000 }), 'all', now), '更早');
  assert.equal(groupOf(mk(1, { pinned: true, copiedAt: 0 }), 'all', now), '置顶');
  assert.equal(groupOf(mk(1, {}), 'fav', now), '按使用次数排序');
});

test('rel 与 groupOf 对同一条记录不矛盾', () => {
  const now = new Date(2026, 9, 19, 10).getTime();
  const older = new Date(2026, 9, 17, 22).getTime();
  const yday = new Date(2026, 9, 18, 10, 6).getTime();
  assert.equal(groupOf(mk(1, { copiedAt: older }), 'all', now), '更早');
  assert.equal(rel(older, now), '前天');
  assert.equal(groupOf(mk(1, { copiedAt: yday }), 'all', now), '昨天');
  assert.equal(rel(yday, now), '昨天');
});

test('groups 按 groupOf 切成连续分组', () => {
  const now = new Date(2026, 9, 19, 12).getTime();
  const vis = [mk(1, { pinned: true }), mk(2, { copiedAt: now }), mk(3, { copiedAt: now - 1 })];
  assert.deepEqual(groups(vis, 'all', now).map((g) => [g.name, g.rows.map((r) => r.id)]), [['置顶', [1]], ['今天', [2, 3]]]);
});

test('counts 不受搜索影响且常用按阈值', () => {
  const m = new Map([mk(1, { uses: 3 }), mk(2, { kind: 'code' })].map((r) => [r.id, r]));
  assert.deepEqual(counts(m, 3), { all: 2, fav: 1, snip: 0, text: 1, link: 0, code: 1, image: 0, video: 0, file: 0 });
});

test('nextSelection 下一条、否则上一条、否则 null', () => {
  const vis = [mk(1, {}), mk(2, {}), mk(3, {})];
  assert.equal(nextSelection(vis, 2), 3);
  assert.equal(nextSelection(vis, 3), 2);
  assert.equal(nextSelection([mk(1, {})], 1), null);
});

test('ensureSelection 选中项被过滤掉时回到第一条', () => {
  assert.equal(ensureSelection([mk(5, {})], 9), 5);
  assert.equal(ensureSelection([mk(5, {})], 5), 5);
  assert.equal(ensureSelection([], 5), null);
});

test('cycleFilter 循环 9 个分类', () => {
  let f: Filter = 'all';
  for (let i = 0; i < 8; i++) f = cycleFilter(f, 1);
  assert.equal(f, 'file');
  assert.equal(cycleFilter('code', 1), 'image');
  assert.equal(cycleFilter('file', 1), 'all');
  assert.equal(cycleFilter('all', -1), 'file');
});

test('isStale', () => {
  assert.ok(isStale(1, 2));
  assert.ok(!isStale(2, 2));
});

test('undo 只保留最后一条且 5 秒后失效', () => {
  const u1 = armUndo(null, [1], 'a', 0);
  const u2 = armUndo(u1, [2], 'b', 1000);
  assert.deepEqual(u2?.ids, [2]);
  assert.ok(canUndo(u2, 5999));
  assert.ok(!canUndo(u2, 6000));
  assert.ok(!canUndo(null, 0));
});

test('片段只在“片段”分类、或“全部”且在搜索时可见', () => {
  const s = mk(1, { kind: 'code', snippetBody: 'x' }), h = mk(2, { kind: 'code' });
  assert.equal(inFilter(s, 'all', 3, false), false);
  assert.equal(inFilter(s, 'all', 3, true), true);
  assert.equal(inFilter(s, 'code', 3, true), false);
  assert.equal(inFilter(s, 'fav', 3, true), false);
  assert.equal(inFilter(s, 'snip', 3, false), true);
  assert.equal(inFilter(h, 'snip', 3, true), false);
});
test('搜索结果里片段排在最前、分组名为“片段”', () => {
  const m = new Map([mk(1, { snippetBody: 'x', copiedAt: 1 }), mk(2, { copiedAt: 9, pinned: true })].map((r) => [r.id, r]));
  const v = visible(m, 'all', new Set([1, 2]), 3);
  assert.equal(v[0].id, 1);
  assert.equal(groups(v, 'all', 10)[0].name, '片段');
});
test('counts 的“全部”与各类型不算片段', () => {
  const m = new Map([mk(1, { kind: 'code', snippetBody: 'x' }), mk(2, { kind: 'code' })].map((r) => [r.id, r]));
  const c = counts(m, 3);
  assert.deepEqual([c.all, c.code, c.snip], [1, 1, 1]);
});
test('armUndo 记下全部 id', () => assert.deepEqual(armUndo(null, [1, 2], 't', 0)?.ids, [1, 2]));
test('pruneMulti 去掉已删的 id，不足 2 条时清空', () => {
  const m = new Map([1, 2, 3].map((i) => [i, mk(i, {})]));
  assert.deepEqual([...pruneMulti(new Set([1, 2, 9]), m)], [1, 2]);
  assert.equal(pruneMulti(new Set([1, 9]), m).size, 0);
});
test('rangeIds 包含两端且与方向无关', () => {
  const v = [1, 2, 3, 4].map((i) => mk(i, {}));
  assert.deepEqual(rangeIds(v, 2, 4), [2, 3, 4]);
  assert.deepEqual(rangeIds(v, 4, 2), [2, 3, 4]);
});
