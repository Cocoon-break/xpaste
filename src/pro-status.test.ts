import { test } from 'node:test';
import assert from 'node:assert/strict';
import { proLine } from './pro-status.ts';

test('四种 Pro 状态的状态行', () => {
  assert.equal(proLine({ state: 'trial', daysLeft: 9 }), '试用中，还剩 9 天');
  assert.equal(proLine({ state: 'licensed', email: 'a@b.com', major: 1 }), '已授权给 a@b.com（1.x）');
  assert.equal(proLine({ state: 'expired', reason: null }), '试用已结束');
  assert.equal(proLine({ state: 'expired', reason: '这个授权码只适用于 1.x' }), '这个授权码只适用于 1.x');
  assert.equal(proLine({ state: 'unavailable' }), '');
});
