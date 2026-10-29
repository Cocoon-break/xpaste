import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Settings } from './api/types';
import {
  addIgnored, baseUrlError, formToSettings, optionFromRetention, parseFrequent, parseIgnored, parseLimit, pickerOptions, removeIgnored, retentionFromOption,
} from './settings-form.ts';

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

test('addIgnored_dedupes_case_insensitively', () => {
  assert.deepEqual(addIgnored(['1Password'], ' 1password '), ['1Password']);
  assert.deepEqual(addIgnored(['1Password'], 'Figma，Slack'), ['1Password', 'Figma', 'Slack']);
  assert.deepEqual(addIgnored([], '  '), []);
});
test('pickerOptions 去重、排除已选、前缀优先', () => {
  const apps = ['Safari', 'safari', '1Password 7', 'Slack', '访达', 'Visual Studio Code', 'Sketch'];
  assert.deepEqual(pickerOptions(apps, ['slack'], 's'), ['Safari', 'Sketch', '1Password 7', 'Visual Studio Code']);
  assert.equal(pickerOptions(apps, [], '', 3).length, 3);
  assert.deepEqual(pickerOptions(apps, [], '访'), ['访达']);
});
test('removeIgnored 按原名移除', () => assert.deepEqual(removeIgnored(['A', 'B'], 'A'), ['B']));

test('parseFrequent 只接受 2–99 的整数', () => {
  assert.equal(parseFrequent('2'), 2);
  assert.equal(parseFrequent('99'), 99);
  assert.equal(parseFrequent(' 5 '), 5);
  for (const bad of ['1', '100', '2.5', '', 'abc']) assert.equal(parseFrequent(bad), null);
});

test('retention 选项往返', () => {
  assert.equal(retentionFromOption('forever'), null);
  assert.equal(retentionFromOption('7'), 7);
  assert.equal(optionFromRetention(30), '30');
  assert.equal(optionFromRetention(null), 'forever');
});

test('formToSettings 只改表单字段，其余（含外观：点选即保存，不经表单）沿用 base', () => {
  const base: Settings = {
    hotkey: 'CommandOrControl+Shift+V', retentionDays: 30, maxRecords: 500, ignoredApps: ['1Password'], skipSensitive: true,
    moveToTop: true, launchAtLogin: true, appearance: 'system', panelSize: [880, 600], lastFilter: 'code', spectrumCollapsed: true,
    frequentMinUses: 3, ocrEnabled: true, ocrBackfill: true, searchFileText: false,
    shotHotkey: 'Alt+Shift+A', shotDir: '', shotSaveCopy: true,
    recHotkey: 'Alt+Shift+R', recDir: '', recSystemAudio: false, recMicrophone: false, recCopy: true,
    recQuality: 'standard', recFps: 30,
    translateEngine: '', translateBaseUrl: 'https://api.openai.com/v1', translateApiKey: '', translateModel: '', translateLang: 'zh-Hans',
  };
  assert.deepEqual(
    formToSettings(base, {
      hotkey: 'CommandOrControl+Alt+C', retain: 'forever', limit: 1000, ignored: ['a', 'b'], skipSensitive: false, moveToTop: false,
      launchAtLogin: false, frequent: 5, ocrEnabled: false, ocrBackfill: false, searchFileText: true,
      shotHotkey: 'Alt+Shift+S', shotDir: '/x', shotSaveCopy: false,
      recHotkey: 'Alt+Shift+T', recDir: '/y', recSystemAudio: true, recMicrophone: true, recCopy: false,
      recQuality: 'small', recFps: '60',
    }),
    {
      hotkey: 'CommandOrControl+Alt+C', retentionDays: null, maxRecords: 1000, ignoredApps: ['a', 'b'], skipSensitive: false,
      moveToTop: false, launchAtLogin: false, appearance: 'system', panelSize: [880, 600], lastFilter: 'code', spectrumCollapsed: true,
      frequentMinUses: 5, ocrEnabled: false, ocrBackfill: false, searchFileText: true,
      shotHotkey: 'Alt+Shift+S', shotDir: '/x', shotSaveCopy: false,
      recHotkey: 'Alt+Shift+T', recDir: '/y', recSystemAudio: true, recMicrophone: true, recCopy: false,
      recQuality: 'small', recFps: 60,
      translateEngine: '', translateBaseUrl: 'https://api.openai.com/v1', translateApiKey: '', translateModel: '', translateLang: 'zh-Hans',
    },
  );
});

test('Base URL 校验', () => {
  assert.equal(baseUrlError(''), null);
  assert.equal(baseUrlError(' https://api.openai.com/v1 '), null);
  assert.equal(baseUrlError('HTTP://localhost:11434/v1'), null);
  assert.equal(baseUrlError('api.openai.com/v1'), '请输入 http:// 或 https:// 开头的地址');
});
