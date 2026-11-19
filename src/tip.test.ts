import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tipPos } from './tip.ts';

test('提示放在元素正下方居中，下面放不下放上方，左右不出窗口', () => {
  const view = { w: 400, h: 300 }, tip = { w: 100, h: 24 };
  assert.deepEqual(tipPos({ left: 150, top: 40, bottom: 60, width: 40 }, tip, view), { x: 120, y: 66 });
  // 贴着底边：放到上方
  assert.deepEqual(tipPos({ left: 150, top: 270, bottom: 290, width: 40 }, tip, view), { x: 120, y: 240 });
  // 贴着左右边：夹在窗口里
  assert.equal(tipPos({ left: 0, top: 40, bottom: 60, width: 20 }, tip, view).x, 4);
  assert.equal(tipPos({ left: 390, top: 40, bottom: 60, width: 10 }, tip, view).x, 296);
});
