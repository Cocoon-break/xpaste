import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { RecordSummary } from './api/types.ts';
import type { TaskView } from './api/meme.ts';
import { cellClick, cellHTML, foldSets, gridMove, memeMeta, memeOrder, taskCellHTML, taskCellText } from './meme-grid.ts';

const mk = (id: number, over: Partial<RecordSummary>): RecordSummary => ({
  id, kind: 'image', title: '', app: null, copiedAt: 0, uses: 0, pinned: false,
  size: 0, lang: null, thumb: null, durationSec: null, ocrState: 0, snippetBody: null, lines: null, url: null, fileCount: null,
  meme: { source: 'final', name: '无语', style: null, gif: true, set: null }, ...over,
});

test('gridMove', () => {
  // 9 个表情包加“＋ 做一个”，5 列：做一个占第 0 格
  assert.equal(gridMove(0, 9, 5, 'ArrowRight', 1), 1);
  assert.equal(gridMove(1, 9, 5, 'ArrowDown', 1), 6);
  assert.equal(gridMove(8, 9, 5, 'ArrowDown', 1), 9);
  assert.equal(gridMove(1, 9, 5, 'ArrowLeft', 1), 0);
  assert.equal(gridMove(9, 9, 5, 'ArrowRight', 1), 9);
  assert.equal(gridMove(6, 9, 5, 'ArrowUp', 1), 1);
  // 搜索时前面没有格子：不会到 0 之前，也不会过最后一个
  assert.equal(gridMove(0, 9, 5, 'ArrowLeft', 0), 0);
  assert.equal(gridMove(2, 9, 5, 'ArrowUp', 0), 0);
  assert.equal(gridMove(7, 9, 5, 'ArrowDown', 0), 8);
  // 做一个加 2 个任务格在前：表情包从第 3 格起，按行移动照样对齐列
  assert.equal(gridMove(3, 9, 5, 'ArrowDown', 3), 8);
  assert.equal(gridMove(11, 9, 5, 'ArrowDown', 3), 11);
  assert.equal(gridMove(8, 9, 5, 'ArrowUp', 3), 3);
});

test('memeOrder', () => {
  const a = mk(1, { uses: 3, copiedAt: 1 }), b = mk(2, { uses: 1, copiedAt: 9 });
  assert.deepEqual([b, a].sort(memeOrder).map((r) => r.id), [1, 2]);
  // 次数相同：上次使用新的在前（从没用过的，copiedAt 就是创建时间）
  const c = mk(3, { uses: 0, copiedAt: 5 }), d = mk(4, { uses: 0, copiedAt: 8 });
  assert.deepEqual([c, d].sort(memeOrder).map((r) => r.id), [4, 3]);
});

test('cellHTML：编号、AI / GIF 角标、做好的显示文字（最多 6 个字）', () => {
  const fin = cellHTML(mk(5, { thumb: 'asset://t/5.png', meme: { source: 'final', name: '收到！马上办好了', style: null, gif: true, set: null } }), 0, true);
  assert.match(fin, /role="option"/);
  assert.match(fin, /aria-selected="true"/);
  assert.match(fin, /<span class="no">1<\/span>/);
  assert.match(fin, /<span class="tag">GIF<\/span>/);
  assert.match(fin, /<span class="cp">收到！马上办<\/span>/);
  const ai = cellHTML(mk(6, { meme: { source: 'ai', name: '开心', style: '黏土', gif: false, set: null } }), 9, false);
  assert.match(ai, /<span class="tag">AI<\/span>/);
  assert.doesNotMatch(ai, /class="no"/);
  assert.doesNotMatch(ai, /class="cp"/);
  // 静态 PNG 的成品不标 GIF；没有文字时名字是默认的“表情包”，不当成文字
  const png = cellHTML(mk(7, { meme: { source: 'final', name: '表情包', style: null, gif: false, set: null } }), 1, false);
  assert.doesNotMatch(png, /class="tag"/);
  assert.doesNotMatch(png, /class="cp"/);
});

test('memeMeta：来源、风格或格式、尺寸', () => {
  assert.equal(memeMeta(mk(1, {}), 240, 240), '做好的 · GIF · 240 × 240');
  assert.equal(memeMeta(mk(1, { meme: { source: 'ai', name: '开心', style: '黏土', gif: false, set: null } }), 1024, 1024), 'AI 出的 · 黏土 · PNG');
  assert.equal(memeMeta(mk(1, { meme: { source: 'final', name: 'x', style: null, gif: false, set: null } }), 0, 0), '做好的 · PNG');
});

test('foldSets：一套收成一格、点开摊开、搜索时不叠、只剩一张不叠', () => {
  const loose = [1, 2, 3].map((id) => ({ id, set: null as string | null }));
  const set = Array.from({ length: 16 }, (_, i) => ({ id: 10 + i, set: 'a' as string | null }));
  // 排序里散的和一套交错
  const list = [loose[0], set[0], loose[1], ...set.slice(1), loose[2]];
  const shut = foldSets(list, null, false);
  assert.equal(shut.length, 4);
  assert.deepEqual(shut.map((c) => c.item.id), [1, 10, 2, 3]);
  const stack = shut[1];
  assert.equal(stack.count, 16);
  assert.equal(stack.folded, true);
  assert.equal(stack.inSet, false);
  assert.ok(shut.filter((c) => c !== stack).every((c) => !c.folded && !c.inSet));

  const open = foldSets(list, 'a', false);
  assert.equal(open.length, 19);
  assert.equal(open.filter((c) => c.inSet).length, 16);
  assert.ok(open.every((c) => !c.folded));

  const found = foldSets(list, null, true);
  assert.equal(found.length, 19);
  assert.ok(found.every((c) => !c.folded && !c.inSet));

  // 删得只剩一张：不叠
  const one = foldSets([loose[0], set[5]], null, false);
  assert.equal(one.length, 2);
  assert.ok(one.every((c) => !c.folded && !c.inSet));
});

test('cellHTML：收着的一叠写“N 张”带叠起来的边；摊开的一套带浅色边', () => {
  const r = mk(8, { meme: { source: 'ai', name: '开心', style: '黏土', gif: false, set: 'a' } });
  const st = cellHTML(r, 0, false, false, 16);
  assert.match(st, /class="mcell stack"/);
  assert.match(st, /<span class="tag">16 张<\/span>/);
  assert.doesNotMatch(st, />AI</);
  assert.match(st, /title="一套 16 张，点开挑"/);
  const ins = cellHTML(r, 0, false, false, 0, true);
  assert.match(ins, /class="mcell inset"/);
  assert.match(ins, /<span class="tag">AI<\/span>/);
});

const task = (over: Partial<TaskView>): TaskView => ({
  id: 1, kind: 'set', state: 'run', title: '不想上班', style: '黏土', startedMs: 0, ahead: 0, ids: [], irregular: null, error: '', scene: '{}', ...over,
});

test('taskCellText：在画、排队、画好、没画出来', () => {
  assert.equal(taskCellText(task({ state: 'run', startedMs: 1_000 }), 43_500), 'AI 在画\n42 秒');
  assert.equal(taskCellText(task({ state: 'run', startedMs: null }), 43_500), 'AI 在画\n0 秒');
  assert.equal(taskCellText(task({ state: 'queue', ahead: 2 }), 0), '排队中\n前面 2 个');
  assert.equal(taskCellText(task({ state: 'queue', ahead: 0 }), 0), '排队中\n下一个');
  assert.equal(taskCellText(task({ state: 'done', ids: [3] }), 0), '画好了');
  assert.equal(taskCellText(task({ state: 'fail', error: '服务商限流' }), 0), '没画出来\n点这里看');
});

test('taskCellHTML：按钮、尺子、缩略图或对勾、新角标', () => {
  const run = taskCellHTML(task({ id: 4, state: 'run', startedMs: 0 }), 30_000, null);
  assert.match(run, /^<button class="mcell gen" type="button" data-task="4"/);
  assert.match(run, /<i style="width:50%"><\/i>/);
  assert.match(run, /AI 在画<br>30 秒/);
  // 60 秒走满一遍后从头走
  assert.match(taskCellHTML(task({ state: 'run', startedMs: 0 }), 72_000, null), /width:20%/);
  assert.match(run, /aria-label="不想上班，AI 在画，30 秒"/);
  const done = taskCellHTML(task({ state: 'done', ids: [3] }), 0, 'asset://t/3.png');
  assert.match(done, /class="mcell gen done"/);
  assert.match(done, /<img src="asset:\/\/t\/3.png"/);
  assert.match(done, /<span class="tag new">新<\/span>/);
  assert.match(taskCellHTML(task({ state: 'done', ids: [3] }), 0, null), /<svg/);
  const fail = taskCellHTML(task({ state: 'fail' }), 0, null);
  assert.match(fail, /class="mcell gen fail"/);
  assert.match(fail, /没画出来<br>点这里看/);
});

test('cellClick：摊开着 a 套时点套外的格子收回并选中它；双击的第二下忽略，双击贴第一下那张', () => {
  const b = { id: 7, set: null };
  const first = cellClick(b, 'a', 1, null);
  assert.deepEqual(first, { act: 'close', openSet: null, id: 7 });
  // 第一下收回后网格重排，第二下落点下面是别的格子（这里是 a 套的 10）：忽略，仍记着 7
  const second = cellClick({ id: 10, set: 'a' }, first.openSet, 2, first.id);
  assert.deepEqual(second, { act: 'ignore', openSet: null, id: 7 });
  // 点一叠摊开；摊开着 a 时点 b 那一叠换成 b
  assert.deepEqual(cellClick({ id: 10, set: 'a' }, null, 1, null), { act: 'open', openSet: 'a', id: 10 });
  assert.deepEqual(cellClick({ id: 20, set: 'b' }, 'a', 1, 10), { act: 'open', openSet: 'b', id: 20 });
  // 同一套里、或都不在套里：只选中
  assert.deepEqual(cellClick({ id: 11, set: 'a' }, 'a', 1, 10), { act: 'select', openSet: 'a', id: 11 });
  assert.deepEqual(cellClick(b, null, 1, null), { act: 'select', openSet: null, id: 7 });
});
