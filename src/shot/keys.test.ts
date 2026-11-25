import { test } from 'node:test';
import assert from 'node:assert/strict';
import { shotKey, type ShotCtx, type ShotKey } from './keys.ts';

const ctx = (o: Partial<ShotCtx> = {}): ShotCtx => ({ phase: 'selected', mode: 'shot', typing: false, edit: true, clip: false, onButton: false, ...o });
const ev = (code: string, o: Partial<ShotKey> = {}): ShotKey => ({ key: code.replace(/^Key|^Digit/, '').toLowerCase(), code, mod: false, shift: false, ...o });
const E = ev('KeyE'), ENTER = ev('', { key: 'Enter' }), ESC = ev('', { key: 'Escape' }), SPACE = ev('Space', { key: ' ' });

test('E 改字：有选区、开着才有', () => {
  assert.deepEqual(shotKey(E, ctx()), { t: 'act', a: 'edit' });
  assert.deepEqual(shotKey(E, ctx({ mode: 'edit' })), { t: 'act', a: 'edit' });
  assert.equal(shotKey(E, ctx({ edit: false })), null);
  assert.equal(shotKey(E, ctx({ phase: 'idle' })), null);
  assert.equal(shotKey(E, ctx({ mode: 'rec' })), null);
  assert.equal(shotKey({ ...E, mod: true }, ctx()), null);
});

test('文字标注输入期间 E 不触发，别的键也不，Esc 收下文字', () => {
  for (const k of [E, ev('KeyO'), ev('KeyT'), ev('KeyP'), ev('KeyL'), ev('KeyF'), ev('Digit1'), ENTER, SPACE, ev('KeyZ', { mod: true })]) {
    assert.equal(shotKey(k, ctx({ typing: true })), null, k.code || k.key);
    assert.equal(shotKey(k, ctx({ typing: true, mode: 'edit', clip: true })), null, k.code || k.key);
  }
  assert.deepEqual(shotKey(ESC, ctx({ typing: true })), { t: 'commit' });
});

test('回车：截图复制、框选改字是改字、录屏开始；选区前不做事', () => {
  assert.deepEqual(shotKey(ENTER, ctx()), { t: 'act', a: 'copy' });
  assert.deepEqual(shotKey(ENTER, ctx({ mode: 'edit' })), { t: 'act', a: 'edit' });
  assert.deepEqual(shotKey(ENTER, ctx({ mode: 'rec' })), { t: 'start' });
  assert.deepEqual(shotKey(ENTER, ctx({ phase: 'idle' })), { t: 'none' });
});

test('空格用刚复制的图：只在框选改字、还没有选区、有卡片时', () => {
  const at = { mode: 'edit', phase: 'idle', clip: true } as const;
  assert.deepEqual(shotKey(SPACE, ctx(at)), { t: 'clip' });
  assert.equal(shotKey(SPACE, ctx({ ...at, clip: false })), null);
  assert.equal(shotKey(SPACE, ctx({ ...at, phase: 'selected' })), null);
  assert.equal(shotKey(SPACE, ctx({ ...at, mode: 'shot' })), null);
});

test('别的快捷键不变', () => {
  const acts: [ShotKey, unknown][] = [
    [ev('KeyS', { mod: true }), { t: 'act', a: 'save' }], [ev('KeyO'), { t: 'act', a: 'ocr' }], [ev('KeyT'), { t: 'act', a: 'translate' }],
    [ev('KeyP'), { t: 'act', a: 'pin' }], [ev('KeyL'), { t: 'long' }], [ev('KeyF'), { t: 'all' }], [ev('Digit7'), { t: 'tool', i: 6 }],
    [ev('KeyZ', { mod: true }), { t: 'undo' }], [ev('KeyZ', { mod: true, shift: true }), { t: 'redo' }],
    [ev('', { key: 'ArrowLeft' }), { t: 'nudge', dx: -1, dy: 0, resize: false }], [ev('', { key: 'ArrowDown', shift: true }), { t: 'nudge', dx: 0, dy: 1, resize: true }],
  ];
  for (const [k, want] of acts) for (const mode of ['shot', 'edit'] as const) assert.deepEqual(shotKey(k, ctx({ mode })), want, k.code || k.key);
  // 单独的 S、Mod+字母不做事
  for (const k of [ev('KeyS'), ev('KeyO', { mod: true }), ev('KeyT', { mod: true }), ev('KeyP', { mod: true }), ev('KeyL', { mod: true }), ev('Digit1', { mod: true }), ev('Digit8')]) assert.equal(shotKey(k, ctx()), null);
  // 选区前只有回车、F、Esc
  for (const k of [ev('KeyO'), ev('KeyS', { mod: true }), ev('Digit1'), ev('', { key: 'ArrowLeft' })]) assert.equal(shotKey(k, ctx({ phase: 'idle' })), null);
  assert.deepEqual(shotKey(ev('KeyF'), ctx({ phase: 'idle' })), { t: 'all' });
  // 录屏只有方向键微调
  assert.equal(shotKey(ev('KeyO'), ctx({ mode: 'rec' })), null);
  assert.deepEqual(shotKey(ev('', { key: 'ArrowUp' }), ctx({ mode: 'rec' })), { t: 'nudge', dx: 0, dy: -1, resize: false });
});

test('Esc 哪一步都退出；译文卡片上只有回车复制，焦点在按钮上时交给按钮', () => {
  for (const phase of ['idle', 'selected', 'busy', 'countdown', 'card'] as const) assert.deepEqual(shotKey(ESC, ctx({ phase })), { t: 'close' });
  assert.deepEqual(shotKey(ENTER, ctx({ phase: 'card' })), { t: 'copyTranslation' });
  assert.equal(shotKey(ENTER, ctx({ phase: 'card', onButton: true })), null);
  assert.equal(shotKey(E, ctx({ phase: 'card' })), null);
  for (const phase of ['off', 'done'] as const) for (const k of [ESC, E, ENTER]) assert.equal(shotKey(k, ctx({ phase })), null);
  assert.equal(shotKey(ENTER, ctx({ phase: 'busy' })), null);
});
