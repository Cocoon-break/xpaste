import { test } from 'node:test';
import assert from 'node:assert/strict';
import { translatable, trAfterDelta, trAfterEnd, trBlockHTML } from './translate.ts';

test('可翻译的类型', () => {
  const r = (kind: string, ocrState = 0) => ({ kind, ocrState }) as any;
  assert.deepEqual(['text', 'code', 'link', 'video', 'image', 'file'].map((k) => translatable(r(k))), [true, true, false, false, false, false]);
  assert.equal(translatable(r('image', 1)), true);
  assert.equal(translatable(r('file', 1)), true);
  assert.equal(translatable(r('image', 3)), false);
});

test('增量与结束', () => {
  let t = trAfterDelta(undefined, 'zh-Hans', '构建');
  t = trAfterDelta(t, 'zh-Hans', '失败');
  assert.deepEqual(t, { text: '构建失败', lang: 'zh-Hans', state: 'busy' });
  const done = trAfterEnd(t, { ok: true, text: '构建失败。', lang: 'zh-Hans' });
  assert.deepEqual(done, { text: '构建失败。', lang: 'zh-Hans', state: 'done' });
  assert.deepEqual(trAfterEnd(t, { ok: false, error: '翻译超时' }), { text: '构建失败', lang: 'zh-Hans', state: 'cut', error: '翻译超时' });
  assert.equal(trAfterEnd(undefined, { ok: false, error: '翻译超时' }).state, 'fail');
  assert.equal(trAfterEnd({ text: '', lang: '', state: 'busy' }, { ok: false, error: '翻译超时' }).state, 'fail');
});

test('区块标题、按钮与转义', () => {
  const h = trBlockHTML({ text: '<b>', lang: 'en', state: 'done' }, false);
  assert.match(h, /^<section class="tr-block" data-state="done"/);
  assert.match(h, /译文 · English/);
  assert.match(h, /data-tr="copy"/);
  assert.match(h, /data-tr="again"/);
  assert.match(h, /&lt;b&gt;/);
  const busy = trBlockHTML({ text: '', lang: 'en', state: 'busy' }, false);
  assert.match(busy, /翻译中…/);
  assert.doesNotMatch(busy, /data-tr=/);
  const f = trBlockHTML({ text: '半', lang: 'en', state: 'cut', error: '翻译超时' }, true);
  assert.match(f, /翻译中断/); assert.match(f, /翻译超时/); assert.match(f, /class="tr-text mono"/);
  assert.doesNotMatch(f, /data-tr="copy"/);
  assert.match(f, /data-tr="again"/);
  const x = trBlockHTML({ text: '', lang: '', state: 'fail', error: '<x>' }, false);
  assert.match(x, /翻译失败/); assert.match(x, /&lt;x&gt;/); assert.match(x, /data-tr="again"/);
});
