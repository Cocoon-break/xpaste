import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compactGroups, compactHeadH, rowParts } from './compact.ts';
import type { RecordSummary } from './api/types';

let n = 0;
const r = (o: Partial<RecordSummary> = {}): RecordSummary => ({
  id: ++n, kind: 'text', title: '标题', app: null, copiedAt: 1, uses: 0, pinned: false, size: 1, lang: null, thumb: null,
  durationSec: null, ocrState: 0, snippetBody: null, lines: null, url: null, fileCount: null, ...o,
});

test('多行文本补充“N 行”，一行不补', () => {
  assert.equal(rowParts(r({ title: '周四会议纪要', lines: 6 }), { q: '' }).extra, '6 行');
  assert.equal(rowParts(r({ lines: 1 }), { q: '' }).extra, '');
  assert.equal(rowParts(r({ kind: 'code', lines: 3 }), { q: '' }).extra, '3 行');
});

test('有标题的链接补充域名', () => {
  const p = rowParts(r({ kind: 'link', title: 'GitHub', url: 'https://github.com/x' }), { q: '' });
  assert.deepEqual([p.title, p.extra, p.rev], ['GitHub', 'github.com', false]);
});

test('无标题链接：标题位置是域名，补充是其余部分', () => {
  const u = 'https://freemyip.com/update?t=1';
  const p = rowParts(r({ kind: 'link', title: u, url: u }), { q: '' });
  assert.deepEqual([p.title, p.extra, p.rev], ['freemyip.com', '/update?t=1', true]);
});

test('视频补充时长，多个文件补充个数，片段补充内容第一行', () => {
  assert.equal(rowParts(r({ kind: 'video', durationSec: 42 }), { q: '' }).extra, '00:42');
  assert.equal(rowParts(r({ kind: 'file', fileCount: 3 }), { q: '' }).extra, '等 3 个文件');
  assert.equal(rowParts(r({ kind: 'file', fileCount: 1 }), { q: '' }).extra, '');
  assert.equal(rowParts(r({ snippetBody: 'a\nb', lines: 2 }), { q: '' }).extra, 'a');
});

test('命中图中文字：标题换成命中片段', () => {
  const p = rowParts(r({ kind: 'image', title: '截图.png' }), { q: '837', snippet: '会议号 837 2049' });
  assert.equal(p.ocr, true);
  assert.match(p.title, /<mark>837<\/mark>/);
});

test('拼音命中按汉字高亮，补充不变', () => {
  const p = rowParts(r({ title: '周四会议纪要', lines: 6 }), { q: 'hy', mark: '会议' });
  assert.equal(p.title, '周四<mark>会议</mark>纪要');
  assert.equal(p.extra, '6 行');
});

test('标题没命中、来源应用命中：补充“来自 应用”', () => {
  const p = rowParts(r({ title: '设计稿', app: { name: 'Figma', icon: null } }), { q: 'fig' });
  assert.equal(p.extra, '来自 <mark>Fig</mark>ma');
});

test('其余命中：补充“内容含 关键词”', () => {
  const p = rowParts(r({ title: '需求评审纪要.md', kind: 'file' }), { q: '灰度方案' });
  assert.equal(p.extra, '内容含“<mark>灰度方案</mark>”');
});

test('标题命中照常高亮并转义', () => {
  assert.equal(rowParts(r({ title: '<b>会议</b>' }), { q: '会议' }).title, '&lt;b&gt;<mark>会议</mark>&lt;/b&gt;');
});

test('分组：置顶与其余之间一条细线', () => {
  const gs = compactGroups([r({ pinned: true }), r(), r()], 'all');
  assert.deepEqual(gs.map((g) => [g.name, g.rows.length]), [['置顶', 1], ['记录', 2]]);
  assert.deepEqual(gs.map((_, i) => compactHeadH(gs, i)), [0, 9]);
});

test('分组：搜索命中片段时两组各有小标题', () => {
  const gs = compactGroups([r({ snippetBody: 'x' }), r()], 'all');
  assert.deepEqual(gs.map((g) => g.name), ['片段', '记录']);
  assert.deepEqual(gs.map((_, i) => compactHeadH(gs, i)), [24, 24]);
});

test('分组：没有置顶与片段时只有一组、没有分隔', () => {
  const gs = compactGroups([r({ copiedAt: Date.now() }), r({ copiedAt: 1 })], 'all');
  assert.deepEqual(gs.map((g) => g.name), ['记录']);
  assert.equal(compactHeadH(gs, 0), 0);
  const fav = compactGroups([r({ pinned: true, uses: 9 }), r({ uses: 5 })], 'fav');
  assert.deepEqual(fav.map((g) => g.name), ['按使用次数排序']);
  assert.equal(compactHeadH(fav, 0), 0);
});
