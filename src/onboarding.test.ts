import { test } from 'node:test';
import assert from 'node:assert/strict';
import { nextStep, onboardSteps } from './onboarding.ts';

test('onboardSteps：macOS 三步，Windows 没有辅助功能', () => {
  assert.deepEqual(onboardSteps('macos'), ['hotkey', 'accessibility', 'autostart']);
  assert.deepEqual(onboardSteps('windows'), ['hotkey', 'autostart']);
});

test('nextStep 按顺序推进，最后一步之后为 null', () => {
  const mac = onboardSteps('macos'), win = onboardSteps('windows');
  assert.equal(nextStep(mac, 'hotkey'), 'accessibility');
  assert.equal(nextStep(mac, 'accessibility'), 'autostart');
  assert.equal(nextStep(win, 'hotkey'), 'autostart');
  assert.equal(nextStep(mac, 'autostart'), null);
  assert.equal(nextStep(win, 'autostart'), null);
});
