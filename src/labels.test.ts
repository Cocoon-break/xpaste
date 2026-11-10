import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyVerb, escHint, exportNote, fileTextHelp, importNote, openLabel, snippetExportName } from './labels.ts';

test('copyVerb 按能否粘贴给出动词', () => {
  assert.equal(copyVerb(true), '粘贴');
  assert.equal(copyVerb(false), '复制');
});

test('escHint 按搜索框是否有字给出 Esc 提示', () => {
  assert.equal(escHint(true), '清空搜索');
  assert.equal(escHint(false), '隐藏');
});

test('escHint 多选时 Esc 先取消选择（M25 规则 6）', () => {
  assert.equal(escHint(false, true), '取消选择');
  assert.equal(escHint(true, true), '取消选择');
});

test('openLabel 按类型与平台给出右键菜单的打开动作', () => {
  assert.equal(openLabel('link', false, 'macos'), '在浏览器中打开');
  assert.equal(openLabel('image', true, 'windows'), '在资源管理器中显示');
  assert.equal(openLabel('image', false, 'macos'), null);
  assert.equal(openLabel('text', false, 'macos'), null);
});

test('escHint 有来源胶囊、搜索框为空时提示去掉来源（M29 规则 5）', () => {
  assert.equal(escHint(false, false, true), '去掉来源');
  assert.equal(escHint(true, false, true), '清空搜索');
  assert.equal(escHint(false, true, true), '取消选择');
});

test('fileTextHelp：macOS 的说明里带 PDF，Windows 不带（M05 规则 2）', () => {
  assert.equal(fileTextHelp('macos'), '复制 txt、md、csv、json、代码、PDF 等文件时读入前 64 KB 供搜索，不显示；会让数据库变大');
  assert.equal(fileTextHelp('windows'), '复制 txt、md、csv、json、代码等文件时读入前 64 KB 供搜索，不显示；会让数据库变大');
});

test('片段导出、导入的结果文字（M25 规则 11）', () => {
  assert.equal(exportNote({ exported: 14, skipped: 0 }, 'a.json'), '已导出 14 个片段到 a.json');
  assert.equal(exportNote({ exported: 14, skipped: 2 }, 'a.json'), '已导出 14 个片段到 a.json；2 个图片、文件片段没有导出');
  assert.equal(importNote({ imported: 12, skipped: 0, invalid: 0 }), '已导入 12 个片段');
  assert.equal(importNote({ imported: 12, skipped: 2, invalid: 0 }), '已导入 12 个片段，跳过 2 个已有的');
  assert.equal(importNote({ imported: 0, skipped: 3, invalid: 0 }), '没有新的片段，3 个都已经有了');
  assert.equal(importNote({ imported: 1, skipped: 0, invalid: 2 }), '已导入 1 个片段，2 个名字或内容不合规没有导入');
  assert.equal(importNote({ imported: 0, skipped: 0, invalid: 0 }), '文件里没有片段');
  assert.equal(snippetExportName(new Date(2026, 10, 10)), 'xPaste-片段-2026-11-10.json');
});
