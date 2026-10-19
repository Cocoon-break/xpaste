import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Kind, RecordSummary } from './api/types.ts';
import { DAY, hPct, layoutBars, sameColumns, spectrumStat, staggerIndex, xPct } from './spectrum.ts';

const R = (id: number, at: number, size = 100, kind: Kind = 'text'): RecordSummary =>
  ({ id, kind, title: 't', app: null, copiedAt: at, uses: 0, pinned: false, size, lang: null, thumb: null, durationSec: null });
const now = new Date(2026, 9, 19, 12, 0).getTime();
test('xPct_maps_24h_and_earlier_zone', () => {
  assert.equal(xPct(0), 99.4);
  assert.equal(xPct(DAY / 2), 53);
  assert.equal(xPct(DAY), 6);
  assert.ok(Math.abs(xPct(DAY + 1)! - 5) < 1e-6);
  assert.equal(xPct(7 * DAY), 1);
  assert.equal(xPct(7 * DAY + 1), null);
  assert.equal(xPct(-60e3), 99.4); // 未来时间
});
test('hPct 按大小取对数，16%–100%', () => {
  assert.equal(hPct(0), 26.5); assert.equal(hPct(10), 26.5); assert.equal(hPct(1e8), 100); assert.equal(hPct(1e9), 100);
});
test('layoutBars_caps_bars_by_columns', () => {
  const recs = Array.from({ length: 5000 }, (_, i) => R(i + 1, now - i * 60e3));
  const { bars, colOf } = layoutBars(recs, now, 900, () => true);
  assert.ok(bars.length <= 450);
  assert.equal(colOf.size, 5000);
  for (let i = 1; i < bars.length; i++) assert.ok(bars[i].x >= bars[i - 1].x);
});
test('layoutBars_prefers_matching_then_fresh_then_taller', () => {
  const a = R(1, now - 1000, 10, 'text'), b = R(2, now - 2000, 1e6, 'image'), c = R(3, now - 1500, 10, 'code');
  let L = layoutBars([a, b, c], now, 900, () => true);
  assert.equal(L.bars.length, 1);
  assert.equal(L.bars[0].id, 2);
  L = layoutBars([a, b, c], now, 900, (r) => r.kind === 'code');
  assert.deepEqual([L.bars[0].id, L.bars[0].kind, L.bars[0].dim], [3, 'code', false]);
  L = layoutBars([a, b, c], now, 900, () => true, 1);
  assert.deepEqual([L.bars[0].id, L.bars[0].fresh], [1, true]);
  L = layoutBars([a, b, c], now, 900, () => false);
  assert.equal(L.bars[0].dim, true);
  assert.deepEqual([...L.bars[0].ids].sort(), [1, 2, 3]);
});
test('layoutBars_drops_older_than_7_days_and_survives_zero_width', () => {
  const L = layoutBars([R(1, now - 8 * DAY), R(2, now)], now, 0, () => true);
  assert.equal(L.bars.length, 1);
  assert.equal(L.colOf.has(1), false);
});
test('spectrumStat_counts_24h_and_peak_hour', () => {
  const at = (h: number, m: number) => new Date(2026, 9, 19, h, m).getTime();
  const recs = [R(1, at(9, 5)), R(2, at(9, 40)), R(3, at(10, 1)), R(4, at(10, 2)), R(5, at(11, 0)), R(6, now - 2 * DAY)];
  assert.deepEqual(spectrumStat(recs, now), { count: 5, peak: { hour: 9, n: 2 } });
  assert.deepEqual(spectrumStat([R(1, at(9, 0))], now), { count: 1, peak: null });
});
test('staggerIndex 把升起压在 21 步内', () => {
  assert.equal(staggerIndex(5, 10), 5); assert.equal(staggerIndex(0, 450), 0); assert.equal(staggerIndex(449, 450), 20);
});
test('sameColumns_true_when_only_dim_changes', () => {
  const recs = [R(1, now - 1000), R(2, now - 2000, 1e6), R(3, now - 6 * 3600e3, 10, 'code')];
  const all = layoutBars(recs, now, 900, () => true);
  assert.equal(sameColumns(all, layoutBars(recs, now, 900, (r) => r.kind === 'code')), true);
  assert.equal(sameColumns(all, layoutBars(recs.slice(1), now, 900, () => true)), false); // 列内记录变了
  assert.equal(sameColumns(all, layoutBars([...recs, R(4, now - 12 * 3600e3)], now, 900, () => true)), false); // 列数变了
  assert.equal(sameColumns(all, layoutBars(recs, now, 2, () => true)), false); // 宽度变窄合并成一列
});
