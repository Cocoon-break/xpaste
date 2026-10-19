import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Settings } from './api/types';
import { formToSettings, joinIgnored, optionFromRetention, parseIgnored, parseLimit, retentionFromOption } from './settings-form.ts';

test('parseLimit 只接受 50–5000 的 50 倍数', () => {
  assert.equal(parseLimit('500'), 500);
  assert.equal(parseLimit('33'), null);
  assert.equal(parseLimit(''), null);
  assert.equal(parseLimit('5050'), null);
  assert.equal(parseLimit('50'), 50);
  assert.equal(parseLimit('5000'), 5000);
  assert.equal(parseLimit('0'), null);
});

test('parseIgnored_handles_chinese_comma_spaces_dupes_empties', () =>
  assert.deepEqual(parseIgnored(' 1Password，钥匙串访问, 1password ,, 1Password'), ['1Password', '钥匙串访问', '1password']));

test('joinIgnored 用 ", " 连接，与 parseIgnored 往返', () => {
  assert.equal(joinIgnored(['1Password', '钥匙串访问']), '1Password, 钥匙串访问');
  assert.deepEqual(parseIgnored(joinIgnored(['a', 'b'])), ['a', 'b']);
});

test('retention 选项往返', () => {
  assert.equal(retentionFromOption('forever'), null);
  assert.equal(retentionFromOption('7'), 7);
  assert.equal(optionFromRetention(30), '30');
  assert.equal(optionFromRetention(null), 'forever');
});

test('formToSettings 只改表单字段，其余沿用 base', () => {
  const base: Settings = {
    hotkey: 'CommandOrControl+Shift+V', retentionDays: 30, maxRecords: 500, ignoredApps: ['1Password'], skipSensitive: true,
    moveToTop: true, launchAtLogin: true, appearance: 'system', panelSize: [880, 600], lastFilter: 'code',
  };
  assert.deepEqual(
    formToSettings(base, {
      hotkey: 'CommandOrControl+Alt+C', retain: 'forever', limit: 1000, ignored: 'a，b', skipSensitive: false, moveToTop: false,
      launchAtLogin: false, appearance: 'dark',
    }),
    {
      hotkey: 'CommandOrControl+Alt+C', retentionDays: null, maxRecords: 1000, ignoredApps: ['a', 'b'], skipSensitive: false,
      moveToTop: false, launchAtLogin: false, appearance: 'dark', panelSize: [880, 600], lastFilter: 'code',
    },
  );
});
