import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ocrLabel, ocrProgressLabel, snippetHTML } from './ocr-label.ts';

test('ocrLabel 六个状态的文案', () => {
  assert.equal(ocrLabel(0), '待识别');
  assert.equal(ocrLabel(1, '季度预算'), '4 字');
  assert.equal(ocrLabel(2), '无文字');
  assert.equal(ocrLabel(3), '识别失败');
  assert.equal(ocrLabel(4), '已跳过');
  assert.equal(ocrLabel(5), '本机不支持');
});

test('ocrLabel 字数按字符数（不按 UTF-16 码元）', () => {
  assert.equal(ocrLabel(1, '𠀀a😀'), '3 字');
  assert.equal(ocrLabel(1, null), '0 字');
});

test('ocrProgressLabel 已处理 a / b，千分位', () => {
  assert.equal(ocrProgressLabel(3, 5), '已处理 3 / 5');
  assert.equal(ocrProgressLabel(1200, 4800), '已处理 1,200 / 4,800');
});

test('snippetHTML 高亮第一处（忽略大小写），其余转义', () => {
  assert.equal(snippetHTML('…季度预算 2026…', '预算'), '…季度<mark>预算</mark> 2026…');
  assert.equal(snippetHTML('Budget budget', 'BUDGET'), '<mark>Budget</mark> budget');
  assert.equal(snippetHTML('a<b>c', 'x'), 'a&lt;b&gt;c');
  assert.equal(snippetHTML('a<b>c', '  '), 'a&lt;b&gt;c');
});

test('snippetHTML 查询里的 < 与正则元字符不能注入', () => {
  assert.equal(snippetHTML('x<img>y', '<img'), 'x<mark>&lt;img</mark>&gt;y');
  assert.equal(snippetHTML('1+1=2', '1+1'), '<mark>1+1</mark>=2');
  assert.equal(snippetHTML('abc', '<mark>'), 'abc');
});

test('snippetHTML 命中前只留 4 个字符，列表一行里看得到关键词', () => {
  assert.equal(snippetHTML('…2026 年第三季度预算\n市场部　1…', '预算'), '…第三季度<mark>预算</mark>\n市场部　1…');
  assert.equal(snippetHTML('…第三季度预算', '预算'), '…第三季度<mark>预算</mark>');
  assert.equal(snippetHTML('…𠀀𠀀𠀀𠀀𠀀预算', '预算'), '…𠀀𠀀𠀀𠀀<mark>预算</mark>');
  assert.equal(snippetHTML('<<<<<预算', '预算'), '…&lt;&lt;&lt;&lt;<mark>预算</mark>');
});

test('snippetHTML 用命中的汉字高亮', () => {
  assert.equal(snippetHTML('…会议号：482 913 776…', '会议号'), '…<mark>会议号</mark>：482 913 776…');
});
