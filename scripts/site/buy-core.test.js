// 官网购买流程的纯函数测试。跑法：node --test scripts/site/*.test.js（make test-web 也会带上）
import test from 'node:test';
import assert from 'node:assert/strict';
import * as core from '../../website/assets/buy-core.js';

const perpetual = { slug: 'pro-1x', name: 'xPaste Pro 1.x', kind: 'perpetual', major: 1, period_months: null, max_devices: 2, prices: [{ currency: 'USD', amount: 1499 }, { currency: 'CNY', amount: 9900 }] };
const yearly = { slug: 'pro-year', name: 'xPaste Pro 年付', kind: 'subscription', major: null, period_months: 12, max_devices: 1, prices: [{ currency: 'CNY', amount: 4950 }] };
const providers = [{ name: 'manual', currencies: ['CNY', 'USD'] }, { name: 'wechat', currencies: ['CNY'] }];

test('金额：最小货币单位换成给人看的写法', () => {
  assert.equal(core.formatMoney(9900, 'CNY'), '¥99');
  assert.equal(core.formatMoney(4950, 'CNY'), '¥49.50');
  assert.equal(core.formatMoney(1499, 'USD'), 'US$14.99');
  assert.equal(core.formatMoney(1500, 'JPY'), 'JP¥1,500'); // 日元没有小数位
  assert.equal(core.formatMoney(1234, '??'), '12.34 ??'); // 不合法的币种代码不抛错
});

test('商品说明', () => {
  assert.equal(core.describeProduct(perpetual), '一次买断 · 2 台设备 · 适用于 1.x 版本');
  assert.equal(core.describeProduct(yearly), '订阅 · 年付 · 1 台设备');
  assert.equal(core.periodLabel(3), '季付');
  assert.equal(core.periodLabel(24), '每 24 个月');
});

test('币种与渠道求交集，CNY 排最前', () => {
  assert.deepEqual(core.currenciesFor(perpetual, providers), ['CNY', 'USD']);
  assert.deepEqual(core.currenciesFor(perpetual, [{ name: 'stripe', currencies: ['USD', 'EUR'] }]), ['USD']);
  assert.deepEqual(core.currenciesFor(yearly, [{ name: 'stripe', currencies: ['USD'] }]), []);
  assert.deepEqual(core.providersFor(providers, 'USD').map((p) => p.name), ['manual']);
  assert.deepEqual(core.providersFor(providers, 'CNY').map((p) => p.name), ['manual', 'wechat']);
  assert.deepEqual(core.sellable([perpetual, yearly], [{ name: 'stripe', currencies: ['USD'] }]), [perpetual]);
  assert.deepEqual(core.sellable([perpetual], []), []);
});

test('价格', () => {
  assert.equal(core.priceOf(perpetual, 'CNY'), 9900);
  assert.equal(core.priceOf(yearly, 'USD'), null);
  assert.equal(core.lowestPrice([perpetual, yearly]), 4950);
  assert.equal(core.lowestPrice([], 'CNY'), null);
});

test('渠道叫法：登记过的用中文，没登记的原样显示', () => {
  assert.equal(core.providerLabel('manual'), '转账付款');
  assert.equal(core.providerLabel('mock'), '模拟付款（测试）');
  assert.equal(core.providerLabel('stripe'), 'stripe');
});

test('邮箱基本校验', () => {
  assert.ok(core.isEmail(' buyer@example.com '));
  for (const bad of ['', 'buyer', 'buyer@', '@example.com', 'a b@example.com', 'buyer@example']) assert.ok(!core.isEmail(bad), bad);
});

test('付款方式怎么展示', () => {
  assert.deepEqual(core.paymentView({ type: 'redirect', value: 'https://pay.example.com/x' }), { kind: 'redirect', value: 'https://pay.example.com/x' });
  assert.equal(core.paymentView({ type: 'redirect', value: 'mock://pay/abc' }).kind, 'note'); // 浏览器打不开，不跳
  assert.equal(core.paymentView({ type: 'redirect', value: 'javascript:alert(1)' }).kind, 'note');
  assert.equal(core.paymentView({ type: 'qr', value: 'weixin://wxpay/x' }).kind, 'qr');
  assert.equal(core.paymentView({ type: 'manual', value: '请转账\n备注订单号' }).kind, 'manual');
  assert.equal(core.paymentView(null).kind, 'manual');
});

test('按订单状态决定显示什么', () => {
  const paid = core.orderView(200, { status: 'paid', license_code: 'XP-AAAAA-BBBBB-CCCCC-DDDDD' });
  assert.deepEqual([paid.state, paid.poll, paid.code], ['paid', false, 'XP-AAAAA-BBBBB-CCCCC-DDDDD']);
  // 已付款但授权码还没签出来：接着查
  assert.deepEqual([core.orderView(200, { status: 'paid' }).state, core.orderView(200, { status: 'paid' }).poll], ['pending', true]);
  assert.equal(core.orderView(200, { status: 'pending' }).poll, true);
  for (const s of ['refunded', 'canceled', 'expired']) {
    const v = core.orderView(200, { status: s });
    assert.deepEqual([v.state, v.poll, v.rebuy], ['ended', false, true], s);
    assert.ok(v.title && v.text);
  }
  assert.notEqual(core.orderView(200, { status: 'refunded' }).title, core.orderView(200, { status: 'canceled' }).title);
  const nf = core.orderView(404, { error: 'not_found' });
  assert.deepEqual([nf.state, nf.poll, nf.title], ['notfound', false, '找不到这笔订单']);
  // 网络失败（0）、5xx、429、看不懂的状态：不下结论，接着查
  for (const [code, body] of [[0, null], [502, null], [429, null], [200, { status: '???' }], [200, null]]) {
    assert.deepEqual([core.orderView(code, body).state, core.orderView(code, body).poll], ['retry', true]);
  }
});

test('出错时的提示', () => {
  assert.equal(core.errorMessage(400, { error: 'invalid_request', message: '邮箱不合法' }), '邮箱不合法');
  assert.equal(core.errorMessage(404, { error: 'invalid_code', message: '授权码无效' }), '授权码无效');
  assert.equal(core.errorMessage(404, null), '暂时连不上，稍后再试'); // nginx 自己的 404，没有 message
  assert.equal(core.errorMessage(429, { message: 'x' }), '操作太频繁，稍后再试');
  assert.equal(core.errorMessage(500, { message: '内部错误' }), '暂时连不上，稍后再试');
  assert.equal(core.errorMessage(0, null), '暂时连不上，稍后再试');
});

test('轮询最多 30 分钟', () => {
  assert.equal(core.POLL_MS, 3000);
  assert.ok(!core.pollExpired(0, 29 * 60 * 1000));
  assert.ok(core.pollExpired(0, 30 * 60 * 1000));
});
