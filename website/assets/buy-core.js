// 购买流程里不碰页面的那部分逻辑：金额格式化、币种与渠道求交集、按订单状态决定显示什么。
// 首页和购买页用 <script type="module"> 引入；测试在 scripts/site/buy-core.test.js（node --test scripts/site/*.test.js）。

// 渠道名给人看的叫法。接入新渠道（微信、支付宝、Stripe…）时在这里加一行；没登记的渠道原样显示渠道名。
export const PROVIDER_LABELS = {
  manual: '转账付款',
  mock: '模拟付款（测试）',
};
export const providerLabel = (name) => PROVIDER_LABELS[name] ?? name;

const PERIODS = { 1: '月付', 3: '季付', 6: '半年付', 12: '年付' };
export const periodLabel = (months) => PERIODS[months] ?? `每 ${months} 个月`;

// 金额是最小货币单位（CNY 的 9900 是 ¥99）。每种货币有几位小数由 Intl 给出（日元是 0 位）；整数金额不带小数。
export function formatMoney(amount, currency) {
  try {
    const digits = new Intl.NumberFormat('zh-CN', { style: 'currency', currency }).resolvedOptions().maximumFractionDigits;
    const value = amount / 10 ** digits;
    return new Intl.NumberFormat('zh-CN', { style: 'currency', currency, minimumFractionDigits: Number.isInteger(value) ? 0 : digits }).format(value);
  } catch {
    return `${amount / 100} ${currency}`; // 不是合法的币种代码
  }
}

// 商品的一行说明：买断写覆盖的大版本，订阅写周期
export function describeProduct(p) {
  const devices = `${p.max_devices} 台设备`;
  if (p.kind === 'subscription') return `订阅 · ${periodLabel(p.period_months)} · ${devices}`;
  return p.major == null ? `一次买断 · ${devices}` : `一次买断 · ${devices} · 适用于 ${p.major}.x 版本`;
}

export const priceOf = (product, currency) => product.prices.find((x) => x.currency === currency)?.amount ?? null;

// 能用来买这个商品的币种：商品有这个币种的价格，并且至少一个渠道收它。CNY 排最前（默认币种）。
export function currenciesFor(product, providers) {
  const list = product.prices.map((x) => x.currency).filter((c) => providers.some((p) => p.currencies.includes(c)));
  return [...new Set(list)].sort((a, b) => (b === 'CNY') - (a === 'CNY'));
}

export const providersFor = (providers, currency) => providers.filter((p) => p.currencies.includes(currency));

// 至少有一个币种能付的商品才拿出来卖
export const sellable = (products, providers) => products.filter((p) => currenciesFor(p, providers).length > 0);

// 首页用：所有在售商品里某个币种的最低价，没有则 null
export function lowestPrice(products, currency = 'CNY') {
  const all = products.map((p) => priceOf(p, currency)).filter((x) => x != null);
  return all.length ? Math.min(...all) : null;
}

export const isEmail = (s) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(String(s).trim());

// 下单返回的 payment 该怎么展示：
//   redirect —— 跳到 value；note —— 浏览器打不开的地址（如 mock://），只显示说明；
//   qr —— 把 value 画成二维码；manual —— 把 value 当纯文本的收款说明显示
export function paymentView(payment) {
  const type = payment?.type, value = String(payment?.value ?? '');
  if (type === 'redirect') return { kind: /^https?:\/\//i.test(value) ? 'redirect' : 'note', value };
  if (type === 'qr') return { kind: 'qr', value };
  return { kind: 'manual', value };
}

// 查单结果 → 页面该显示什么。state 决定显示哪一块；poll 为真表示接着轮询；rebuy 为真表示给“重新购买”。
export function orderView(httpStatus, body) {
  if (httpStatus === 404) return { state: 'notfound', poll: false, rebuy: true, title: '找不到这笔订单', text: '订单号可能不完整，或者这笔订单不是在这里下的。' };
  if (httpStatus !== 200) return { state: 'retry', poll: true, rebuy: false, title: '', text: httpStatus === 429 ? '查询太频繁，稍后自动重试。' : '暂时连不上，稍后自动重试。' };
  switch (body?.status) {
    case 'paid':
      return body.license_code
        ? { state: 'paid', poll: false, rebuy: false, code: body.license_code, title: '付款成功', text: '' }
        : { state: 'pending', poll: true, rebuy: false, title: '已收到付款', text: '正在签发授权码，稍等几秒。' };
    case 'pending':
      return { state: 'pending', poll: true, rebuy: false, title: '等待付款', text: '付款确认后，这个页面会自动更新。' };
    case 'refunded':
      return { state: 'ended', poll: false, rebuy: true, title: '这笔订单已退款', text: '对应的授权码已经失效。需要的话可以重新购买。' };
    case 'canceled':
      return { state: 'ended', poll: false, rebuy: true, title: '这笔订单已取消', text: '没有扣款。需要的话可以重新下单。' };
    case 'expired':
      return { state: 'ended', poll: false, rebuy: true, title: '这笔订单超时未付款', text: '订单已经关闭，可以重新下单。如果你已经付了款，先不要再付一次，按页面底部的方式联系我们。' };
    default:
      return { state: 'retry', poll: true, rebuy: false, title: '', text: '暂时连不上，稍后自动重试。' };
  }
}

// 接口出错时给人看的一句话。400 / 404 的 message 是服务端写好的中文，直接用。
export function errorMessage(httpStatus, body) {
  if ((httpStatus === 400 || httpStatus === 404) && typeof body?.message === 'string' && body.message) return body.message;
  if (httpStatus === 429) return '操作太频繁，稍后再试';
  return '暂时连不上，稍后再试';
}

// 轮询节奏：等待付款时每 3 秒查一次，最多查 30 分钟
export const POLL_MS = 3000;
export const POLL_LIMIT_MS = 30 * 60 * 1000;
export const pollExpired = (startedAt, now) => now - startedAt >= POLL_LIMIT_MS;
