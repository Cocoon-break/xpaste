import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SEARCH_GAP_MS, searchGate } from './search-gate.ts';

// 手动时钟与定时器：测试里不等真实时间
function harness() {
  let t = 1000;
  const due: { at: number; fn: () => void }[] = [];
  const timers = {
    set: (fn: () => void, ms: number) => {
      const h = { at: t + ms, fn };
      due.push(h);
      return h;
    },
    clear: (h: unknown) => {
      const i = due.indexOf(h as (typeof due)[0]);
      if (i >= 0) due.splice(i, 1);
    },
  };
  const calls: string[] = [];
  let resolvers: (() => void)[] = [];
  const run = (v: string) => {
    calls.push(v);
    return new Promise<void>((r) => resolvers.push(r));
  };
  const g = searchGate(run, SEARCH_GAP_MS, () => t, timers);
  const advance = (ms: number) => {
    t += ms;
    const fire = due.filter((h) => h.at <= t);
    fire.forEach((h) => due.splice(due.indexOf(h), 1));
    fire.forEach((h) => h.fn());
  };
  const settle = async () => {
    const rs = resolvers;
    resolvers = [];
    rs.forEach((r) => r());
    await Promise.resolve();
    await Promise.resolve();
  };
  return { g, calls, advance, settle, pending: () => due.length };
}

test('first input runs immediately', () => {
  const h = harness();
  h.g.push('a');
  assert.deepEqual(h.calls, ['a']);
});

test('inputs inside the gap coalesce into one trailing query with the latest text', async () => {
  const h = harness();
  h.g.push('a');
  await h.settle();
  h.advance(10);
  h.g.push('ab');
  h.advance(10);
  h.g.push('abc');
  assert.deepEqual(h.calls, ['a']);
  h.advance(SEARCH_GAP_MS);
  assert.deepEqual(h.calls, ['a', 'abc']);
});

test('while a query is in flight, inputs wait and the latest runs once it returns', async () => {
  const h = harness();
  h.g.push('a');
  h.advance(SEARCH_GAP_MS * 2);
  h.g.push('ab');
  h.g.push('abc');
  assert.deepEqual(h.calls, ['a']);
  assert.equal(h.pending(), 0);
  await h.settle();
  assert.deepEqual(h.calls, ['a', 'abc']);
});

test('flush runs a pending input now; cancel drops it', async () => {
  const h = harness();
  h.g.push('a');
  await h.settle();
  h.advance(10);
  h.g.push('ab');
  h.g.flush();
  assert.deepEqual(h.calls, ['a', 'ab']);
  await h.settle();
  h.advance(10);
  h.g.push('abc');
  h.g.cancel();
  h.advance(SEARCH_GAP_MS);
  assert.deepEqual(h.calls, ['a', 'ab']);
});

test('an input after the gap with nothing in flight runs immediately', async () => {
  const h = harness();
  h.g.push('a');
  await h.settle();
  h.advance(SEARCH_GAP_MS + 1);
  h.g.push('b');
  assert.deepEqual(h.calls, ['a', 'b']);
});
