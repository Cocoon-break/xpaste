import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cardHTML, cardInner, clipSource, type CardState } from './view.ts';

const base = { source: 'Hello', engine: 'system', text: '', noAccess: false, other: null } as const;
type Tr = Extract<CardState, { kind: 'busy' | 'done' | 'fail' }>;
const st = (s: Partial<Tr> & Pick<Tr, 'kind'>): CardState => ({ ...base, ...s });

test('提示：无文字、排除应用，没有按钮', () => {
  const e = cardHTML({ kind: 'empty', hotkey: '⌥ + Shift + T' });
  assert.match(e, /没有可翻译的文字：先选中文字再按 ⌥ \+ Shift \+ T/);
  assert.doesNotMatch(e, /<button/);
  const b = cardHTML({ kind: 'blocked' });
  assert.match(b, /这个应用里的内容不翻译/);
  assert.doesNotMatch(b, /<button/);
});

test('翻译中：标题带方式，正文占位，复制不可用，没有换用', () => {
  const h = cardHTML(st({ kind: 'busy', other: 'openai' }));
  assert.match(h, /翻译中… · 系统翻译/);
  assert.match(h, /data-act="copy" aria-disabled="true"/);
  assert.match(h, /关闭<kbd>Esc<\/kbd>/);
  assert.match(h, /class="c-body c-wait">翻译中…/);
  assert.doesNotMatch(h, /换用/);
});

test('收完：译文标题、复制可用、按另一种给换用按钮', () => {
  const h = cardHTML(st({ kind: 'done', text: '你好', engine: 'openai:gpt-4o-mini', other: 'system' }));
  assert.match(h, /译文 · AI（gpt-4o-mini）/);
  assert.match(h, /data-act="copy" aria-disabled="false">复制译文<kbd>↵<\/kbd>/);
  assert.match(h, /data-act="switch" data-engine="system">换用系统翻译</);
  assert.match(cardHTML(st({ kind: 'done', text: 'x', other: 'openai' })), />换用 AI 翻译</);
  assert.doesNotMatch(cardHTML(st({ kind: 'done', text: 'x', other: null })), /data-act="switch"/);
});

test('失败：原因、下载语言包 / 去设置，中断保留已收到的部分', () => {
  const pack = cardHTML(st({ kind: 'fail', error: '系统翻译还没有英语和简体中文的语言包', errorKind: 'translatePack', other: 'openai' }));
  assert.match(pack, /翻译失败 · 系统翻译/);
  assert.match(pack, /class="c-err">系统翻译还没有英语和简体中文的语言包/);
  assert.match(pack, /data-act="pack">下载语言包</);
  assert.match(pack, /data-act="switch" data-engine="openai">换用 AI 翻译</);
  assert.doesNotMatch(pack, /去设置/);
  assert.doesNotMatch(pack, /data-act="copy"/); // 失败只能关闭（M26 规则 19），按钮放不下
  const cfg = cardHTML(st({ kind: 'fail', error: '未配置', errorKind: 'translateConfig' }));
  assert.match(cfg, /data-act="settings">去设置</);
  assert.doesNotMatch(cfg, /下载语言包/);
  const cut = cardHTML(st({ kind: 'fail', text: '已收到', error: '翻译超时', errorKind: 'translate' }));
  assert.match(cut, /翻译中断 · 系统翻译/);
  assert.match(cut, /已收到/);
  assert.doesNotMatch(cut, /data-act="(pack|settings)"/);
});

test('没有辅助功能权限：底部提示加前往授权', () => {
  const h = cardHTML(st({ kind: 'done', text: 'x', noAccess: true }));
  assert.match(h, /没有辅助功能权限，翻译的是剪切板里的文字/);
  assert.match(h, /data-act="access">前往授权</);
  assert.doesNotMatch(cardHTML(st({ kind: 'done', text: 'x' })), /前往授权/);
});

test('复制后：显示已复制译文，没有按钮', () => {
  const h = cardHTML(st({ kind: 'done', text: 'x', copied: true, other: 'openai' }));
  assert.match(h, /已复制译文/);
  assert.doesNotMatch(h, /<button/);
});

test('原文提示取前 40 个字，超出加 …（后端已截过的再截一次不变）', () => {
  const s40 = '一'.repeat(40);
  assert.equal(clipSource(s40), s40);
  assert.equal(clipSource(s40 + '二'), s40 + '…');
  assert.equal(clipSource(clipSource(s40 + '二三')), s40 + '…');
  assert.equal(clipSource('😀'.repeat(41)), '😀'.repeat(40) + '…');
  assert.match(cardHTML(st({ kind: 'busy', source: s40 + '二' })), new RegExp(`class="c-src">${s40}…<`));
});

test('所有文本转义', () => {
  const h = cardHTML(st({ kind: 'fail', source: '<i>', text: '<b>&', engine: 'openai:<m>', error: '<x>', errorKind: 'translate' }));
  assert.doesNotMatch(h, /<i>|<b>|<m>|<x>/);
  assert.match(h, /&lt;i&gt;/);
  assert.match(h, /&lt;b&gt;&amp;/);
  assert.match(h, /AI（&lt;m&gt;）/);
  assert.match(cardHTML({ kind: 'empty', hotkey: '<k>' }), /&lt;k&gt;/);
});

test('截图卡片（cardInner）：没有原文提示，标题行带方式，有换用按钮（M26 规则 5、16）', () => {
  const t = { kind: 'done', source: '', engine: 'system', text: 'x', noAccess: false, other: 'openai' } as const;
  const h = cardInner(t);
  assert.doesNotMatch(h, /c-src|role="dialog"/);
  assert.match(h, /class="c-title" title="译文 · 系统翻译">译文 · 系统翻译</);
  assert.match(h, /data-act="switch" data-engine="openai">换用 AI 翻译</);
  assert.equal(cardHTML(t), `<div class="card" role="dialog" aria-label="译文">${h}</div>`);
});

test('截图卡片缺语言包：不给下载语言包，原因与换用照常（截图遮罩挂不了系统的下载确认）', () => {
  const h = cardInner({ kind: 'fail', source: '', engine: 'system', text: '', noAccess: false, other: 'openai', error: '系统翻译还没有English和简体中文的语言包', errorKind: undefined });
  assert.match(h, /翻译失败 · 系统翻译/);
  assert.match(h, /class="c-err">系统翻译还没有English和简体中文的语言包/);
  assert.match(h, /data-act="switch" data-engine="openai">换用 AI 翻译</);
  assert.doesNotMatch(h, /下载语言包<|data-act="(pack|settings)"/);
});
