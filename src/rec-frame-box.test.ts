import { test } from 'node:test';
import assert from 'node:assert/strict';
import { frameBox, infoSide, previewBox } from './rec-frame-box.ts';

test('frameBox 在选区外 3 px', () => {
  assert.deepEqual(frameBox({ x: 100, y: 50, w: 400, h: 300 }, 1440, 900), { x: 97, y: 47, w: 406, h: 306 });
});

test('frameBox 全屏时收回到屏幕边上', () => {
  assert.deepEqual(frameBox({ x: 0, y: 0, w: 1440, h: 900 }, 1440, 900), { x: 0, y: 0, w: 1440, h: 900 });
});

test('frameBox 只收回贴边的那几边', () => {
  assert.deepEqual(frameBox({ x: 0, y: 200, w: 500, h: 700 }, 1440, 900), { x: 0, y: 197, w: 503, h: 703 });
});

test('长截图提示：下方放得下放下方，否则上方，都不行放在选区里', () => {
  assert.equal(infoSide({ x: 0, y: 100, w: 400, h: 300 }, 900), 'below');
  assert.equal(infoSide({ x: 0, y: 100, w: 400, h: 780 }, 900), 'above');
  assert.equal(infoSide({ x: 0, y: 0, w: 1440, h: 900 }, 900), 'inside');
});

test('长截图预览：右侧放不下放左侧，都放不下放选区里；高同选区、不出屏', () => {
  assert.deepEqual(previewBox({ x: 100, y: 50, w: 400, h: 300 }, 1440, 900), { x: 514, y: 50, w: 242, h: 300 });
  assert.equal(previewBox({ x: 1100, y: 50, w: 300, h: 300 }, 1440, 900).x, 1100 - 14 - 242);
  assert.equal(previewBox({ x: 0, y: 0, w: 1440, h: 900 }, 1440, 900).x, 1440 - 242 - 8);
  assert.deepEqual(previewBox({ x: 100, y: 0, w: 400, h: 900 }, 1440, 900), { x: 514, y: 8, w: 242, h: 884 });
  assert.equal(previewBox({ x: 100, y: 850, w: 400, h: 40 }, 1440, 900).h, 160);
});
