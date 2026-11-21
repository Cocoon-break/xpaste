import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { RecordSummary } from './api/types.ts';
import { cellHTML, gridMove, memeMeta, memeOrder } from './meme-grid.ts';

const mk = (id: number, over: Partial<RecordSummary>): RecordSummary => ({
  id, kind: 'image', title: '', app: null, copiedAt: 0, uses: 0, pinned: false,
  size: 0, lang: null, thumb: null, durationSec: null, ocrState: 0, snippetBody: null, lines: null, url: null, fileCount: null,
  meme: { source: 'final', name: '无语', style: null, gif: true }, ...over,
});

test('gridMove', () => {
  // 9 个表情包加“＋ 做一个”，5 列：做一个占第 0 格
  assert.equal(gridMove(0, 9, 5, 'ArrowRight', true), 1);
  assert.equal(gridMove(1, 9, 5, 'ArrowDown', true), 6);
  assert.equal(gridMove(8, 9, 5, 'ArrowDown', true), 9);
  assert.equal(gridMove(1, 9, 5, 'ArrowLeft', true), 0);
  assert.equal(gridMove(9, 9, 5, 'ArrowRight', true), 9);
  assert.equal(gridMove(6, 9, 5, 'ArrowUp', true), 1);
  // 搜索时没有做一个：不会到 0 之前，也不会过最后一个
  assert.equal(gridMove(0, 9, 5, 'ArrowLeft', false), 0);
  assert.equal(gridMove(2, 9, 5, 'ArrowUp', false), 0);
  assert.equal(gridMove(7, 9, 5, 'ArrowDown', false), 8);
});

test('memeOrder', () => {
  const a = mk(1, { uses: 3, copiedAt: 1 }), b = mk(2, { uses: 1, copiedAt: 9 });
  assert.deepEqual([b, a].sort(memeOrder).map((r) => r.id), [1, 2]);
  // 次数相同：上次使用新的在前（从没用过的，copiedAt 就是创建时间）
  const c = mk(3, { uses: 0, copiedAt: 5 }), d = mk(4, { uses: 0, copiedAt: 8 });
  assert.deepEqual([c, d].sort(memeOrder).map((r) => r.id), [4, 3]);
});

test('cellHTML：编号、AI / GIF 角标、做好的显示文字（最多 6 个字）', () => {
  const fin = cellHTML(mk(5, { thumb: 'asset://t/5.png', meme: { source: 'final', name: '收到！马上办好了', style: null, gif: true } }), 0, true);
  assert.match(fin, /role="option"/);
  assert.match(fin, /aria-selected="true"/);
  assert.match(fin, /<span class="no">1<\/span>/);
  assert.match(fin, /<span class="tag">GIF<\/span>/);
  assert.match(fin, /<span class="cp">收到！马上办<\/span>/);
  const ai = cellHTML(mk(6, { meme: { source: 'ai', name: '开心', style: '黏土', gif: false } }), 9, false);
  assert.match(ai, /<span class="tag">AI<\/span>/);
  assert.doesNotMatch(ai, /class="no"/);
  assert.doesNotMatch(ai, /class="cp"/);
  // 静态 PNG 的成品不标 GIF；没有文字时名字是默认的“表情包”，不当成文字
  const png = cellHTML(mk(7, { meme: { source: 'final', name: '表情包', style: null, gif: false } }), 1, false);
  assert.doesNotMatch(png, /class="tag"/);
  assert.doesNotMatch(png, /class="cp"/);
});

test('memeMeta：来源、风格或格式、尺寸', () => {
  assert.equal(memeMeta(mk(1, {}), 240, 240), '做好的 · GIF · 240 × 240');
  assert.equal(memeMeta(mk(1, { meme: { source: 'ai', name: '开心', style: '黏土', gif: false } }), 1024, 1024), 'AI 出的 · 黏土 · PNG');
  assert.equal(memeMeta(mk(1, { meme: { source: 'final', name: 'x', style: null, gif: false } }), 0, 0), '做好的 · PNG');
});
