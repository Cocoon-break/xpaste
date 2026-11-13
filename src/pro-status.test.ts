import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deviceLine, proControls, proLine } from './pro-status.ts';

test('Pro 状态的状态行', () => {
  assert.equal(proLine({ state: 'trial', daysLeft: 9 }), '试用中，还剩 9 天');
  assert.equal(proLine({ state: 'licensed', email: 'a@b.com', major: 1 }), '已授权给 a@b.com（1.x）');
  assert.equal(proLine({ state: 'licensed', email: 'a@b.com', major: null }), '已授权给 a@b.com（订阅）');
  assert.equal(proLine({ state: 'expired', reason: null }), '试用已结束');
  assert.equal(proLine({ state: 'expired', reason: '这个授权码只适用于 1.x' }), '这个授权码只适用于 1.x');
  assert.equal(proLine({ state: 'expired', reason: '授权已过期，联网后自动续期' }), '授权已过期，联网后自动续期');
  assert.equal(proLine({ state: 'unavailable' }), '');
});

test('没授权时显示输入框和购买入口，已授权时只显示停用', () => {
  const none = { activate: false, buy: false, off: false };
  assert.deepEqual(proControls({ state: 'trial', daysLeft: 3 }, true), { activate: true, buy: true, off: false });
  assert.deepEqual(proControls({ state: 'expired', reason: null }, true), { activate: true, buy: true, off: false });
  // 没有购买页：不显示购买入口
  assert.deepEqual(proControls({ state: 'expired', reason: null }, false), { activate: true, buy: false, off: false });
  assert.deepEqual(proControls({ state: 'licensed', email: 'a@b.com', major: 1 }, true), { ...none, off: true });
  assert.deepEqual(proControls({ state: 'unavailable' }, true), none);
});

test('已绑定设备的激活日期按本地时区写', () => {
  process.env.TZ = 'Asia/Shanghai';
  assert.equal(deviceLine('2026-10-02T08:15:30Z'), '激活于 2026-10-02');
  // 北京时间已经是第二天
  assert.equal(deviceLine('2026-10-02T18:15:30Z'), '激活于 2026-10-03');
  assert.equal(deviceLine(''), '');
  assert.equal(deviceLine('不是时间'), '');
});
