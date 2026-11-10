import { test } from 'node:test';
import assert from 'node:assert/strict';
import { licenseError, proLine } from './pro-status.ts';

test('四种 Pro 状态的状态行', () => {
  assert.equal(proLine({ state: 'trial', daysLeft: 9 }), '试用中，还剩 9 天');
  assert.equal(proLine({ state: 'licensed', email: 'a@b.com', major: 1 }), '已授权给 a@b.com（1.x）');
  assert.equal(proLine({ state: 'expired' }), '试用已结束');
  assert.equal(proLine({ state: 'unavailable' }), '');
});

test('四条授权码错误文案', () => {
  assert.equal(licenseError('format'), '授权码格式不对');
  assert.equal(licenseError('invalid'), '授权码无效');
  assert.equal(licenseError('revoked'), '这个授权码已停用');
  assert.equal(licenseError('oldMajor', 1), '这个授权码只适用于 1.x');
});
