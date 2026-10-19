import { test } from 'node:test';
import assert from 'node:assert/strict';
import { contrast, grade, hexInfo, isHex } from './color.ts';

test('AC-M08-02 #6F8BFF 的 RGB', () => assert.equal(hexInfo('#6F8BFF').rgb, 'rgb(111, 139, 255)'));

test('AC-M08-03 #FFFFFF 对比度与等级', () => {
  const c = hexInfo('#FFFFFF');
  assert.ok(Math.abs(c.onWhite - 1) < 0.01);
  assert.ok(Math.abs(c.onBlack - 21) < 0.01);
  assert.equal(grade(c.onWhite), '不达标');
  assert.equal(grade(c.onBlack), 'AA');
  assert.equal(grade(3.2), 'AA 大字');
});

test('三位 HEX 展开并大写', () => assert.equal(hexInfo('#abc').hex, '#AABBCC'));

test('isHex 拒绝 CSS 注入', () => {
  assert.ok(isHex(' #fff '));
  assert.ok(!isHex('#fff; background:url(x)'));
  assert.ok(!isHex('red'));
});

test('contrast 与 hexInfo 一致', () => {
  assert.equal(contrast('#FFFFFF', '#000000').toFixed(2), '21.00');
  assert.equal(contrast('#000000', '#FFFFFF').toFixed(2), '21.00');
});
