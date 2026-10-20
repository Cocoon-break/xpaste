// 随状态变化的界面文案（规格 M09、M19）
import type { Kind, PlatformInfo } from './api/types';

/** 能自动粘贴（macOS 已授权 / Windows）说“粘贴”，否则说“复制”。 */
export const copyVerb = (canPaste: boolean) => (canPaste ? '粘贴' : '复制');

/** 状态栏 Esc 提示：搜索框有字时 Esc 先清空搜索。 */
export const escHint = (hasQuery: boolean) => (hasQuery ? '清空搜索' : '隐藏');

/** 右键菜单按类型的打开动作（F07）；没有对应动作时为 null。图片只有来自文件时才能在文件管理器中显示。 */
export function openLabel(kind: Kind, hasFile: boolean, os: PlatformInfo['os']): string | null {
  if (kind === 'link') return '在浏览器中打开';
  if (kind === 'file' || kind === 'video' || (kind === 'image' && hasFile)) return os === 'macos' ? '在访达中显示' : '在资源管理器中显示';
  return null;
}
