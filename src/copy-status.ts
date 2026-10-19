// 复制 / 粘贴结果 → 状态栏文案。纯函数，不碰 DOM，便于单元测试。
// 文案与规格说明附录“提示条文案”一致。
import type { CopyResult, PlatformInfo } from './api/types';

export function pasteKey(os: PlatformInfo['os']): string {
  return os === 'macos' ? '⌘ + V' : 'Ctrl + V';
}

export function copyStatus(r: CopyResult, os: PlatformInfo['os']): string {
  switch (r.result) {
    case 'copiedOnly':
      return `已复制，在目标应用中按 ${pasteKey(os)} 粘贴`;
    case 'pasted':
      return '已粘贴';
    case 'copied':
      return '已复制';
  }
}

export function needsAccessibility(r: CopyResult): boolean {
  return r.result === 'copiedOnly' && r.reason === 'needAccessibility';
}
