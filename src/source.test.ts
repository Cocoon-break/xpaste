import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { RecordSummary } from './api/types.ts';
import { appCounts, bestApp, freshSug, fromApp } from './source.ts';

const mk = (id: number, app: string | null, over: Partial<RecordSummary> = {}): RecordSummary => ({
  id, kind: 'text', title: '', app: app === null ? null : { name: app, icon: null }, copiedAt: 0, uses: 0, pinned: false,
  size: 0, lang: null, thumb: null, durationSec: null, ocrState: 0, snippetBody: null, lines: null, url: null, fileCount: null, ...over,
});

test('fromApp 只留来自这个应用的记录；没有胶囊时原样返回', () => {
  const rs = [mk(1, '微信'), mk(2, 'Chrome'), mk(3, null), mk(4, '微信')];
  assert.deepEqual(fromApp(rs, '微信').map((r) => r.id), [1, 4]);
  assert.equal(fromApp(rs, null), rs);
  assert.deepEqual(fromApp(rs, '备忘录'), []);
});

test('appCounts 按应用名计数，不算没有来源的记录', () => {
  const c = appCounts([mk(1, '微信'), mk(2, 'Chrome'), mk(3, null), mk(4, '微信')]);
  assert.deepEqual([...c], [['微信', 2], ['Chrome', 1]]);
});

test('bestApp 取命中的应用里记录最多的；一样多取名字短的；一条都没有的不建议', () => {
  const c = new Map([['微信', 6], ['企业微信', 2], ['Chrome', 2], ['Chrome Canary', 2]]);
  assert.deepEqual(bestApp(['企业微信', '微信'], c), { app: '微信', n: 6 });
  assert.deepEqual(bestApp(['Chrome Canary', 'Chrome'], c), { app: 'Chrome', n: 2 });
  assert.equal(bestApp(['备忘录'], c), null);
  assert.equal(bestApp([], c), null);
});

test('freshSug：建议是为别的输入算的（结果还没回来）时不算数，Tab 不接受', () => {
  const sug = { app: '微信', n: 6, q: 'wx' };
  assert.equal(freshSug(sug, ' wx '), sug);
  assert.equal(freshSug(sug, 'wxpay'), null);
  assert.equal(freshSug(null, 'wx'), null);
});
