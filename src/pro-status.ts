// 设置“Pro”标签的状态行。纯函数。
import type { Pro } from './api/line';

/** 状态行：试用剩余天数、授权给谁和适用的大版本、试用已结束；不含晾衣绳的版本为空 */
export function proLine(p: Pro): string {
  switch (p.state) {
    case 'trial': return `试用中，还剩 ${p.daysLeft} 天`;
    case 'licensed': return `已授权给 ${p.email}（${p.major}.x）`;
    case 'expired': return '试用已结束';
    default: return '';
  }
}
