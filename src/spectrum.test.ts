import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Kind, RecordSummary } from './api/types.ts';
import {
  centerWin, clampWin, DAY, followNow, hPct, HOUR, layoutBars, MIN_SPAN, miniWin, panWin, sameColumns, spanOf, spectrumStat,
  staggerIndex, tickStep, ticks, timeAt, winLabel, xOf, xPct, zoomWin,
} from './spectrum.ts';

const R = (id: number, at: number, size = 100, kind: Kind = 'text'): RecordSummary =>
  ({ id, kind, title: 't', app: null, copiedAt: at, uses: 0, pinned: false, size, lang: null, thumb: null, durationSec: null, ocrState: 0 });
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

// 时间窗（阶段 11）用的 now：本地 14:04，便于核对刻度与标题
const now2 = new Date(2026, 9, 20, 14, 4).getTime();
test('clampWin 夹长度与范围，达到 24h 返回 null，贴 now 为 pinned', () => {
  const w = clampWin(now2 - DAY - 1000, now2 - DAY + HOUR, now2)!;      // 左越界：整体右移
  assert.deepEqual(w, { start: now2 - DAY, end: now2 - DAY + HOUR + 1000, pinned: false });
  assert.deepEqual(clampWin(now2 - HOUR, now2 + 1000, now2), { start: now2 - HOUR - 1000, end: now2, pinned: true });
  assert.deepEqual(clampWin(now2 - 60e3, now2, now2), { start: now2 - MIN_SPAN, end: now2, pinned: true });
  assert.equal(clampWin(now2 - DAY, now2, now2), null);
  assert.equal(clampWin(now2 - 2 * DAY, now2, now2), null);
  assert.equal(clampWin(now2 - 3 * HOUR, now2 - 999, now2)!.pinned, true);
});
test('followNow 只移动 pinned 窗口', () => {
  const p = { start: now2 - HOUR, end: now2, pinned: true }, f = { start: now2 - 2 * HOUR, end: now2 - HOUR, pinned: false };
  assert.deepEqual(followNow(p, now2 + 60e3), { start: now2 + 60e3 - HOUR, end: now2 + 60e3, pinned: true });
  assert.equal(followNow(f, now2 + 60e3), f);
});
test('zoomWin 围绕指针缩放：默认视图正中放大一倍得到 18h–6h 前', () => {
  assert.deepEqual(zoomWin(null, 0.53, 0.5, now2), { start: now2 - 18 * HOUR, end: now2 - 6 * HOUR, pinned: false });
  assert.deepEqual(zoomWin(null, 1, 0.5, now2), { start: now2 - 12 * HOUR, end: now2, pinned: true });
  const w = { start: now2 - HOUR, end: now2, pinned: true };
  assert.deepEqual(zoomWin(w, 1, 0.5, now2), { start: now2 - HOUR / 2, end: now2, pinned: true });
  assert.deepEqual(zoomWin(w, 0, 0.5, now2), { start: now2 - HOUR, end: now2 - HOUR / 2, pinned: false });
  assert.equal(zoomWin(w, 0.5, 48, now2), null);                                 // 放大到 24h 回默认
  assert.equal(spanOf(zoomWin({ start: now2 - MIN_SPAN, end: now2, pinned: true }, 0.5, 0.5, now2)!), MIN_SPAN);
});
test('panWin 与 centerWin 经过夹紧', () => {
  const w = { start: now2 - 2 * HOUR, end: now2 - HOUR, pinned: false };
  assert.deepEqual(panWin(w, 30 * 60e3, now2), { start: now2 - 1.5 * HOUR, end: now2 - HOUR / 2, pinned: false });
  assert.deepEqual(panWin(w, 2 * HOUR, now2), { start: now2 - HOUR, end: now2, pinned: true });
  assert.deepEqual(centerWin(now2 - 30 * 60e3, HOUR, now2), { start: now2 - HOUR, end: now2, pinned: true });
  assert.deepEqual(centerWin(now2 - 2 * DAY, HOUR, now2), { start: now2 - DAY, end: now2 - DAY + HOUR, pinned: false });
});
test('timeAt 与 xOf 互为反函数，窗外返回 null', () => {
  const w = { start: now2 - HOUR, end: now2, pinned: true };
  assert.equal(xOf(w, now2 - 30 * 60e3, now2), 50);
  assert.equal(xOf(w, now2 - 2 * HOUR, now2), null);
  assert.equal(xOf(w, now2 - 60.5 * 60e3, now2)! < 0, true);                        // −0.83%，仍画（在 −1 内）
  assert.equal(xOf(w, now2 - 61 * 60e3, now2), null);
  assert.equal(xOf(null, now2 - DAY / 2, now2), xPct(DAY / 2));
  assert.equal(timeAt(w, 0.25, now2), now2 - 45 * 60e3);
  assert.equal(timeAt(null, 1, now2), now2);
  assert.equal(timeAt(null, 0.53, now2), now2 - DAY / 2);
  assert.equal(timeAt(null, 0.01, now2), now2 - DAY);
});
test('tickStep 按窗口长度选步长，主刻度不超过 8 个', () => {
  assert.deepEqual(tickStep(8 * 60e3), [60e3, 15e3]);
  assert.deepEqual(tickStep(10 * 60e3), [2 * 60e3, 30e3]);
  assert.deepEqual(tickStep(HOUR), [10 * 60e3, 2 * 60e3]);
  assert.deepEqual(tickStep(DAY), [3 * HOUR, HOUR]);
  for (const s of [MIN_SPAN, 7 * 60e3, 17 * 60e3, 45 * 60e3, 3 * HOUR, 9 * HOUR, DAY]) assert.ok(s / tickStep(s)[0] <= 8, String(s));
});
test('ticks 对齐整分钟，贴 now 时追加“现在”', () => {
  const start = new Date(2026, 9, 20, 13, 4).getTime(), w = { start, end: start + HOUR, pinned: true };
  const ts = ticks(w, start + HOUR);
  const majors = ts.filter((t) => t.major && !t.now);
  assert.deepEqual(majors.map((t) => new Date(t.t).getMinutes()), [10, 20, 30, 40, 50, 0]);
  assert.equal(majors[0].x, 10);
  assert.equal(ts.filter((t) => !t.major).length, 25);                           // 13:04–14:04 每 2 分钟 31 个，去掉 6 个主刻度
  assert.deepEqual(ts[ts.length - 1], { x: 100, t: start + HOUR, major: true, now: true });
  assert.ok(!ticks({ ...w, end: start + HOUR, pinned: false }, start + HOUR + 5000).some((t) => t.now));
});
test('ticks 按本地时间对齐：6 小时窗 09:30–15:30 主刻度落在本地 10:00…15:00', () => {
  const start = new Date(2026, 9, 20, 9, 30).getTime(), w = { start, end: start + 6 * HOUR, pinned: false };
  const majors = ticks(w, w.end + HOUR).filter((t) => t.major);
  assert.deepEqual(majors.map((t) => [new Date(t.t).getHours(), new Date(t.t).getMinutes()]), [10, 11, 12, 13, 14, 15].map((h) => [h, 0]));
});
test('miniWin 与 winLabel', () => {
  assert.deepEqual(miniWin({ start: now2 - 6 * HOUR, end: now2, pinned: true }, now2), { left: 75, width: 25 });
  assert.equal(miniWin({ start: now2 - MIN_SPAN, end: now2, pinned: true }, now2).width, 0.6);
  const f = (t: number) => new Date(t).toTimeString().slice(0, 5);
  assert.equal(winLabel({ start: now2 - HOUR, end: now2, pinned: true }, f), '13:04 – 14:04（1 小时）');
  assert.equal(winLabel({ start: now2 - 90 * 60e3, end: now2, pinned: true }, f), '12:34 – 14:04（1.5 小时）');
  assert.equal(winLabel({ start: now2 - 10 * 60e3, end: now2, pinned: true }, f), '13:54 – 14:04（10 分钟）');
});
test('layoutBars 带时间窗时只排窗内记录', () => {
  const recs = [R(1, now2 - 10 * 60e3), R(2, now2 - 3 * HOUR), R(3, now2 - 11 * 60e3)];
  const { bars, colOf } = layoutBars(recs, now2, 900, () => true, undefined, { start: now2 - HOUR, end: now2, pinned: true });
  assert.equal(colOf.size, 2);
  assert.ok(!colOf.has(2));
  assert.ok(bars.every((b) => b.x > 80));
});
