import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rawPinyin } from './ime.ts';

test('原样上屏的拼音去掉音节间空格', () => {
  assert.equal(rawPinyin('f z'), 'fz');
  assert.equal(rawPinyin('ni hao ma'), 'nihaoma');
  assert.equal(rawPinyin('分支'), null);
  assert.equal(rawPinyin('fz'), null);
  assert.equal(rawPinyin('New York'), null);
  assert.equal(rawPinyin('a 分'), null);
  assert.equal(rawPinyin(' f'), null);
});
