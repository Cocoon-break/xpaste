// 设置“Pro”标签的状态行与授权码错误文案。纯函数。
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

/** 授权码错误文案（同后端 `From<LicenseError>`）；`oldMajor` 带授权码适用的大版本 */
export function licenseError(kind: string, major?: number): string {
  switch (kind) {
    case 'format': return '授权码格式不对';
    case 'invalid': return '授权码无效';
    case 'revoked': return '这个授权码已停用';
    case 'oldMajor': return `这个授权码只适用于 ${major}.x`;
    default: return '';
  }
}
