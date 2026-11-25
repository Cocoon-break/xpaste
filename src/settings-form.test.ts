import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Settings } from './api/types';
import {
  editApiState, editClearAsk, editRecentText, memeCaptionState, memeStatsText, MEME_PRESETS, memePreset, providerAfterEdit, addIgnored, baseUrlError, formToSettings, optionFromRetention, parseFrequent, parseIgnored, parseLimit, pickerOptions, removeIgnored, retentionFromOption,
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
    moveToTop: true, launchAtLogin: true, updateCheck: true, appearance: 'system', panelSize: [880, 600], lastFilter: 'code', spectrumCollapsed: true,
    panelStyle: 'full', compactSize: [400, 520],
    frequentMinUses: 3, ocrEnabled: true, ocrBackfill: true, searchFileText: false,
    shotHotkey: 'Alt+Shift+A', shotDir: '', shotSaveCopy: true, longAutoScroll: false,
    recHotkey: 'Alt+Shift+R', recDir: '', recSystemAudio: false, recMicrophone: false, recCopy: true,
    recQuality: 'standard', recFps: 30,
    translateEngine: '', translateBaseUrl: 'https://api.openai.com/v1', translateApiKey: '', translateModel: '', translateLang: 'zh-Hans',
    translateHotkey: 'Alt+Shift+T', lineHotkey: 'Alt+Shift+N',
    lineStyle: { rope: 'jute', thick: 'mid', peg: 'plastic', paper: 'parch', edge: 'auto' },
    lineEnabled: true,
    lineSide: 'top',
    linePinned: false, lineAppear: 'normal', lineLeave: 'normal',
    beltEnabled: true, beltAuto: false, beltHotkey: 'Alt+Shift+B', beltBackHotkey: 'Alt+Shift+Z',
    mixerEnabled: true, mixerHotkey: 'Alt+Shift+M', mixerRecipes: [],
    memeEnabled: true, memeHotkey: 'Alt+Shift+E', editEnabled: true, editHotkey: 'Alt+Shift+G', editMode: 'ai', memeImageProvider: 'openai', memeImageFormat: 'openai',
    memeImageBaseUrl: 'https://api.openai.com/v1', memeImageApiKey: '', memeImageModel: 'gpt-image-2', memeStylesUsed: [], memeStylesCustom: [], memeLastStyle: 'q', memeLastTextStyle: 'stroke', memeLastTextPos: 'bottom', memeMoreOpen: false,
  };
  assert.deepEqual(
    formToSettings(base, {
      hotkey: 'CommandOrControl+Alt+C', retain: 'forever', limit: 1000, ignored: ['a', 'b'], skipSensitive: false, moveToTop: false,
      launchAtLogin: false, updateCheck: false, frequent: 5, ocrEnabled: false, ocrBackfill: false, searchFileText: true,
      shotHotkey: 'Alt+Shift+S', shotDir: '/x', shotSaveCopy: false, longAutoScroll: true,
      recHotkey: 'Alt+Shift+T', recDir: '/y', recSystemAudio: true, recMicrophone: true, recCopy: false,
      recQuality: 'small', recFps: '60', panelStyle: 'compact', lineHotkey: 'Alt+Shift+L',
      beltEnabled: false, beltAuto: true, beltHotkey: 'Alt+Shift+J', beltBackHotkey: 'Alt+Shift+K',
      mixerEnabled: false, mixerHotkey: 'Alt+Shift+H', mixerRecipes: [{ name: 'a', prompt: 'p', show: true, builtin: null }],
    }),
    {
      hotkey: 'CommandOrControl+Alt+C', retentionDays: null, maxRecords: 1000, ignoredApps: ['a', 'b'], skipSensitive: false,
      moveToTop: false, launchAtLogin: false, updateCheck: false, appearance: 'system', panelSize: [880, 600], lastFilter: 'code', spectrumCollapsed: true,
      panelStyle: 'compact', compactSize: [400, 520],
      frequentMinUses: 5, ocrEnabled: false, ocrBackfill: false, searchFileText: true,
      shotHotkey: 'Alt+Shift+S', shotDir: '/x', shotSaveCopy: false, longAutoScroll: true,
      recHotkey: 'Alt+Shift+T', recDir: '/y', recSystemAudio: true, recMicrophone: true, recCopy: false,
      recQuality: 'small', recFps: 60,
      translateEngine: '', translateBaseUrl: 'https://api.openai.com/v1', translateApiKey: '', translateModel: '', translateLang: 'zh-Hans',
      translateHotkey: 'Alt+Shift+T', lineHotkey: 'Alt+Shift+L',
      lineStyle: { rope: 'jute', thick: 'mid', peg: 'plastic', paper: 'parch', edge: 'auto' },
      lineEnabled: true,
      lineSide: 'top',
      linePinned: false, lineAppear: 'normal', lineLeave: 'normal',
      beltEnabled: false, beltAuto: true, beltHotkey: 'Alt+Shift+J', beltBackHotkey: 'Alt+Shift+K',
      mixerEnabled: false, mixerHotkey: 'Alt+Shift+H', mixerRecipes: [{ name: 'a', prompt: 'p', show: true, builtin: null }],
      memeEnabled: true, memeHotkey: 'Alt+Shift+E', editEnabled: true, editHotkey: 'Alt+Shift+G', editMode: 'ai', memeImageProvider: 'openai', memeImageFormat: 'openai',
      memeImageBaseUrl: 'https://api.openai.com/v1', memeImageApiKey: '', memeImageModel: 'gpt-image-2', memeStylesUsed: [], memeStylesCustom: [], memeLastStyle: 'q', memeLastTextStyle: 'stroke', memeLastTextPos: 'bottom', memeMoreOpen: false,
    },
  );
});

test('Base URL 校验', () => {
  assert.equal(baseUrlError(''), null);
  assert.equal(baseUrlError(' https://api.openai.com/v1 '), null);
  assert.equal(baseUrlError('HTTP://localhost:11434/v1'), null);
  assert.equal(baseUrlError('api.openai.com/v1'), '请输入 http:// 或 https:// 开头的地址');
});

test('provider switches to custom when edited', () => {
  const p = memePreset('ark')!;
  // 选“豆包”后改模型 ID：服务商变 custom，格式和地址由调用方保持不变
  assert.equal(providerAfterEdit('ark', 'model'), 'custom');
  assert.equal(providerAfterEdit('ark', 'format'), 'custom');
  assert.equal(providerAfterEdit('ark', 'baseUrl'), 'custom');
  assert.equal(providerAfterEdit('ark', 'apiKey'), 'ark');
  assert.equal(providerAfterEdit('custom', 'model'), 'custom');
  assert.equal(p.format, 'ark');
  assert.equal(memePreset('custom'), undefined);
  assert.deepEqual(MEME_PRESETS.map((x) => x.key), ['openai', 'gemini', 'ark', 'qwen']);
});

test('memeStatsText 写个数和 MB', () => assert.equal(memeStatsText(3, 5 * 1048576 + 52429), '表情包库：3 个，占 5.1 MB'));

test('memeCaptionState 看翻译的地址和模型是否都填了', () => {
  assert.equal(memeCaptionState(' https://a/v1 ', ' gpt-4o-mini '), '已配置：gpt-4o-mini');
  assert.equal(memeCaptionState('', 'm'), '还没配好');
  assert.equal(memeCaptionState('https://a', '  '), '还没配好');
});

test('editApiState：没配时按“怎么改”写清后果', () => {
  assert.deepEqual(editApiState('local', ' https://a/v1 ', ' m '), { text: '已配好 · 和“表情包制作”共用', go: '去设置' });
  assert.deepEqual(editApiState('ai', 'https://a/v1', 'm'), { text: '已配好 · 和“表情包制作”共用', go: '去设置' });
  assert.deepEqual(editApiState('local', '', 'm'), { text: '还没配好，只能改本机做得了的', go: '现在配' });
  assert.deepEqual(editApiState('ai', 'https://a/v1', ' '), { text: '还没配接口：要用 AI 的改不了；本机做得了的，写字时选“这次用本机改”', go: '现在配' });
});

test('editRecentText：张数和占用；0 张也可能占地方', () => {
  assert.equal(editRecentText(0, 0), '还没有改过的图');
  assert.equal(editRecentText(3, 5 * 1048576 + 52429), '3 张，占 5.1 MB');
  assert.equal(editRecentText(0, 2 * 1048576), '0 张，占 2 MB');
});

test('editClearAsk：写明删几张、不能撤销', () => {
  const a = editClearAsk(3);
  assert.match(a.body, /3 张/);
  assert.match(a.body, /不能撤销/);
  assert.match(a.body, /在改的会继续，改好了还会列进来/);
  assert.equal(a.ok, '删除 3 张');
  assert.equal(a.danger, true);
  assert.equal(editClearAsk(0).ok, '删除');
});
