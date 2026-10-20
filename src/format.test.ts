import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canInlinePlay, esc, FILTERS, fmtBytes, fmtDur, fmtFps, hl, isHttpUrl, rel, truncate, videoChips } from './format.ts';

test('hl 转义正则元字符并高亮', () => {
  assert.equal(hl('a.b c++ (x)', 'c++'), 'a.b <mark>c++</mark> (x)');
  assert.equal(hl('axb', '.'), 'axb'); // 不把 . 当通配
  assert.equal(hl('<b>', ''), '&lt;b&gt;'); // 空查询只转义
});

test('esc 转义五个字符', () =>
  assert.equal(esc(`<img onerror="x" src='y'>&`), '&lt;img onerror=&quot;x&quot; src=&#39;y&#39;&gt;&amp;'));

// Intl.RelativeTimeFormat('zh-CN') 的实际输出不带空格；跨日按本地日历日差
test('rel 的边界', () => {
  const now = new Date(2026, 9, 19, 10).getTime();
  assert.equal(rel(now - 44_000, now), '刚刚');
  assert.equal(rel(now - 2 * 60_000, now), '2分钟前');
  assert.equal(rel(now - 59 * 60_000 - 45_000, now), '59分钟前'); // 不四舍五入成“60分钟前”
  assert.equal(rel(now - 50_000, now), '1分钟前');
  assert.equal(rel(now - 3 * 3_600_000, now), '3小时前');
  assert.equal(rel(now - 3 * 3_600_000 - 59 * 60_000, now), '3小时前'); // 同日按整小时截断
  assert.equal(rel(new Date(2026, 9, 18, 10, 6).getTime(), now), '昨天'); // 不足 24 小时但跨日
  assert.equal(rel(new Date(2026, 9, 17, 22).getTime(), now), '前天'); // 36 小时，日历日差 2
  assert.equal(rel(new Date(2026, 9, 14, 23).getTime(), now), '5天前');
});

test('fmtBytes / fmtDur', () => {
  assert.equal(fmtBytes(999), '999 B');
  assert.equal(fmtBytes(486912), '475.5 KB');
  assert.equal(fmtBytes(50541363), '48.2 MB');
  assert.equal(fmtDur(83), '01:23');
  assert.equal(fmtDur(3661), '1:01:01');
});

test('truncate 按码点', () => assert.equal(truncate('😀😀😀', 2), '😀😀…'));

test('isHttpUrl 区分大小写、拒绝伪协议', () => {
  assert.ok(isHttpUrl('https://a.b'));
  assert.ok(!isHttpUrl('javascript:alert(1)'));
  assert.ok(!isHttpUrl('HTTPS://a.b'));
});

test('FILTERS 为 全部、常用 加 6 种类型', () =>
  assert.deepEqual(
    FILTERS.map(([k]) => k),
    ['all', 'fav', 'text', 'link', 'code', 'image', 'video', 'file'],
  ));

test('videoChips_skips_unknown', () => {
  assert.deepEqual(videoChips({ w: 1920, h: 1080, fps: 60, codec: 'H.264' }), ['H.264', '1920 × 1080', '60 fps']);
  assert.deepEqual(videoChips({ w: 1080, h: 1920, fps: 29.97, codec: 'HEVC' }), ['HEVC', '1080 × 1920', '29.97 fps']);
  assert.deepEqual(videoChips({ w: 0, h: 0, fps: 0, codec: '' }), []);
});
test('canInlinePlay_only_webview_containers', () => {
  for (const e of ['mp4', 'MOV', 'm4v', 'webm']) assert.equal(canInlinePlay(e), true);
  for (const e of ['mkv', 'avi', 'wmv', 'flv', '']) assert.equal(canInlinePlay(e), false);
});
test('fmtFps 去掉多余的小数', () => assert.equal(fmtFps(30), '30 fps'));
