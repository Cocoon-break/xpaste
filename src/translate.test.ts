import { test } from 'node:test';
import assert from 'node:assert/strict';
import { engineLabel, otherEngine, translatable, trAfterDelta, trAfterEnd, trBlockHTML } from './translate.ts';

test('可翻译的类型', () => {
  const r = (kind: string, ocrState = 0) => ({ kind, ocrState }) as any;
  assert.deepEqual(['text', 'code', 'link', 'video', 'image', 'file'].map((k) => translatable(r(k))), [true, true, false, false, false, false]);
  assert.equal(translatable(r('image', 1)), true);
  assert.equal(translatable(r('file', 1)), true);
  assert.equal(translatable(r('image', 3)), false);
});

test('增量与结束', () => {
  let t = trAfterDelta(undefined, 'zh-Hans', 'system', '构建');
  t = trAfterDelta(t, 'zh-Hans', 'system', '失败');
  assert.deepEqual(t, { text: '构建失败', lang: 'zh-Hans', engine: 'system', state: 'busy' });
  const done = trAfterEnd(t, { ok: true, text: '构建失败。', lang: 'zh-Hans', engine: 'system' });
  assert.deepEqual(done, { text: '构建失败。', lang: 'zh-Hans', engine: 'system', state: 'done' });
  assert.deepEqual(trAfterEnd(t, { ok: false, error: '翻译超时', engine: 'system' }), { text: '构建失败', lang: 'zh-Hans', engine: 'system', state: 'cut', error: '翻译超时' });
  assert.equal(trAfterEnd(undefined, { ok: false, error: '翻译超时', engine: 'system' }).state, 'fail');
  const f = trAfterEnd({ text: '', lang: '', engine: 'system', state: 'busy' }, { ok: false, error: '翻译超时', engine: 'openai:m' });
  assert.equal(f.state, 'fail');
  assert.equal(f.engine, 'openai:m');
});

test('区块标题、按钮与转义', () => {
  const h = trBlockHTML({ text: '<b>', lang: 'en', engine: 'system', state: 'done' }, false, null);
  assert.match(h, /^<section class="tr-block" data-state="done"/);
  assert.match(h, /译文 · English/);
  assert.match(h, /data-tr="copy"/);
  assert.match(h, /data-tr="again"/);
  assert.match(h, /&lt;b&gt;/);
  const busy = trBlockHTML({ text: '', lang: 'en', engine: 'system', state: 'busy' }, false, null);
  assert.match(busy, /翻译中…/);
  assert.doesNotMatch(busy, /data-tr=/);
  const f = trBlockHTML({ text: '半', lang: 'en', engine: 'system', state: 'cut', error: '翻译超时' }, true, null);
  assert.match(f, /翻译中断/); assert.match(f, /翻译超时/); assert.match(f, /class="tr-text mono"/);
  assert.doesNotMatch(f, /data-tr="copy"/);
  assert.match(f, /data-tr="again"/);
  const x = trBlockHTML({ text: '', lang: '', engine: 'system', state: 'fail', error: '<x>' }, false, null);
  assert.match(x, /翻译失败/); assert.match(x, /&lt;x&gt;/); assert.match(x, /data-tr="again"/);
});

test('翻译方式标注', () => {
  assert.equal(engineLabel('system'), '系统翻译');
  assert.equal(engineLabel('openai:gpt-4o-mini'), 'AI（gpt-4o-mini）');
  assert.equal(engineLabel('openai:'), 'AI');
  assert.equal(engineLabel(''), '');
});
test('换用另一种只在可用时', () => {
  const a = { system: true, openai: true, model: 'm' };
  assert.equal(otherEngine('system', a), 'openai');
  assert.equal(otherEngine('openai:m', a), 'system');
  assert.equal(otherEngine('system', { ...a, openai: false }), null);
  assert.equal(otherEngine('openai:m', { ...a, system: false }), null);
  assert.equal(otherEngine('', a), null);
});
test('标题行与换用按钮', () => {
  const done = trBlockHTML({ text: '你好', lang: 'zh-Hans', engine: 'system', state: 'done' }, false, 'openai');
  assert.match(done, /译文 · 简体中文 · 系统翻译/);
  assert.match(done, /data-tr="switch"[^>]*data-engine="openai"[^>]*>换用 AI 翻译/);
  const busy = trBlockHTML({ text: '', lang: '', engine: 'openai:m', state: 'busy' }, false, 'system');
  assert.match(busy, /翻译中… · AI（m）/);
  assert.doesNotMatch(busy, /data-tr="switch"/);
  assert.match(trBlockHTML({ text: '', lang: '', engine: 'system', state: 'fail', error: 'x' }, false, 'openai'), /翻译失败 · 系统翻译[\s\S]*换用 AI 翻译/);
  assert.match(trBlockHTML({ text: '半', lang: 'en', engine: 'openai:m', state: 'cut', error: 'x' }, false, 'system'), /翻译中断 · AI（m）[\s\S]*data-engine="system"[^>]*>换用系统翻译/);
  // 模型 ID 太长截断，完整值放 title；没有标签时不加“ · ”
  const long = 'x'.repeat(30);
  const h = trBlockHTML({ text: 'a', lang: 'en', engine: `openai:${long}`, state: 'done' }, false, null);
  assert.match(h, new RegExp(`title="AI（${long}）"`));
  assert.match(h, new RegExp(`AI（${'x'.repeat(24)}…）`));
  assert.match(trBlockHTML({ text: 'a', lang: 'en', engine: '', state: 'done' }, false, null), /<span>译文 · English<\/span>/);
});
