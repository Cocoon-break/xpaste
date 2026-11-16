import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Recipe } from './api/types.ts';
import { dropIndex, isChanged, moveRecipe, placeOf, recipeError, sideOf } from './recipes.ts';

const r = (name: string, show = true, builtin: string | null = null, prompt = 'p'): Recipe => ({ name, prompt, show, builtin });

test('placeOf 只数打开的', () => {
  const l = [r('a'), r('b', false), r('c'), r('d'), r('e')];
  assert.deepEqual(l.map((_, i) => placeOf(l, i)), ['key', null, 'key', 'key', 'more']);
});

test('moveRecipe 移到最前', () => {
  const l = [r('a'), r('b'), r('c'), r('d')];
  assert.deepEqual(moveRecipe(l, 2, 0).map((x) => x.name), ['c', 'a', 'b', 'd']);
  assert.deepEqual(moveRecipe(l, 0, 3).map((x) => x.name), ['b', 'c', 'd', 'a']);
  // 不改原数组
  assert.deepEqual(l.map((x) => x.name), ['a', 'b', 'c', 'd']);
});

test('dropIndex 越过中点才换位', () => {
  assert.equal(dropIndex([27, 81, 135], 2, 80), 1);
  assert.equal(dropIndex([27, 81, 135], 2, 82), 2);
  assert.equal(dropIndex([27, 81, 135], 0, 82), 1);
  assert.equal(dropIndex([27, 81, 135], 0, 10), 0);
});

test('recipeError 三种', () => {
  const l = [r('写成提问'), r('自己的')];
  assert.equal(recipeError(l, 1, '  ', 'x'), '请给配方起个名字');
  assert.equal(recipeError(l, 1, '写成提问', 'x'), '已经有同名的配方了');
  assert.equal(recipeError(l, 1, ' 写成提问 ', 'x'), '已经有同名的配方了');
  assert.equal(recipeError(l, 1, '新名字', ' \n'), '提示词不能为空');
  assert.equal(recipeError(l, 1, '自己的', 'x'), null);
  assert.equal(recipeError(l, 0, '写成提问', 'x'), null);
});

test('isChanged 只看自带的', () => {
  const defs = [r('写成提问', true, 'ask', '默认')];
  assert.equal(isChanged(r('写成提问', true, 'ask', '改了'), defs), true);
  assert.equal(isChanged(r('问', true, 'ask', '默认'), defs), true);
  assert.equal(isChanged(r('写成提问', false, 'ask', '默认'), defs), false);
  assert.equal(isChanged(r('写成提问', true, null, '改了'), defs), false);
});

test('sideOf 新配方只占位', () => {
  const defs = [r('写成提问', true, 'ask', '默认')];
  assert.equal(sideOf(r('新的'), defs, true), null);
  assert.equal(sideOf(r('新的'), defs, false), 'del');
  assert.equal(sideOf(r('写成提问', true, 'ask', '改了'), defs, false), 'restore');
  assert.equal(sideOf(r('写成提问', true, 'ask', '默认'), defs, false), null);
});
