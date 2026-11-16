// 设置“Pro”标签的状态行、显示哪些控件、已绑定设备的一行。纯函数。
import type { Pro } from './api/line';

/** 状态行：试用剩余天数、授权给谁（买断写适用的大版本，订阅写“订阅”）、试用已结束（存着的授权用不了时直接写原因）；不含晾衣绳的版本为空 */
export function proLine(p: Pro): string {
  switch (p.state) {
    case 'trial': return `试用中，还剩 ${p.daysLeft} 天`;
    case 'licensed': return `已授权给 ${p.email}（${p.major === null ? '订阅' : `${p.major}.x`}）`;
    case 'expired': return p.reason ?? '试用已结束';
    default: return '';
  }
}

/** “Pro”标签里显示哪些：没授权时是授权码输入框和购买入口（没有购买页就不显示），已授权时只有“在这台设备上停用” */
export function proControls(p: Pro, canBuy: boolean): { activate: boolean; buy: boolean; off: boolean } {
  const open = p.state === 'trial' || p.state === 'expired';
  return { activate: open, buy: open && canBuy, off: p.state === 'licensed' };
}

/** “Pro”概览最上面的授权卡片：色点、标题、一行说明和放哪几个按钮（购买没有购买页时不放） */
export function proCard(p: Pro, canBuy: boolean): { tone: 'ok' | 'trial' | 'off'; title: string; sub: string; buy: boolean; code: boolean; manage: boolean } {
  switch (p.state) {
    case 'licensed': return { tone: 'ok', title: `已授权 · ${p.major === null ? '订阅' : `${p.major}.x`}`, sub: p.email, buy: false, code: false, manage: true };
    case 'trial': return { tone: 'trial', title: `试用中 · 还剩 ${p.daysLeft} 天`, sub: '试用期内可以用全部 Pro 功能', buy: canBuy, code: true, manage: false };
    default: return { tone: 'off', title: p.state === 'expired' ? p.reason ?? '试用已结束' : '', sub: '激活后照常使用全部 Pro 功能', buy: canBuy, code: true, manage: false };
  }
}

/** 已绑定设备名字下面的一行：激活日期（本地时区）；时间读不出来为空 */
export function deviceLine(activatedAt: string): string {
  const d = new Date(activatedAt);
  if (Number.isNaN(d.getTime())) return '';
  const p = (n: number) => String(n).padStart(2, '0');
  return `激活于 ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
