// 随状态变化的界面文案（规格 M09、M19）
import type { Kind, PlatformInfo } from './api/types';

/** 能自动粘贴（macOS 已授权 / Windows）说“粘贴”，否则说“复制”。 */
export const copyVerb = (canPaste: boolean) => (canPaste ? '粘贴' : '复制');

/** 状态栏 Esc 提示：多选时 Esc 先取消选择（与 keymap 的 clearMulti 同序，M25 规则 6），其次搜索框有字时清空搜索。 */
export const escHint = (hasQuery: boolean, multi = false, app = false) => (multi ? '取消选择' : hasQuery ? '清空搜索' : app ? '去掉来源' : '隐藏');

/** 右键菜单按类型的打开动作（F07）；没有对应动作时为 null。图片只有来自文件时才能在文件管理器中显示。 */
export function openLabel(kind: Kind, hasFile: boolean, os: PlatformInfo['os']): string | null {
  if (kind === 'link') return '在浏览器中打开';
  if (kind === 'file' || kind === 'video' || (kind === 'image' && hasFile)) return os === 'macos' ? '在访达中显示' : '在资源管理器中显示';
  return null;
}

/** 设置“搜索文本文件内容”的说明：PDF 只有 macOS 读得出（系统的 PDFKit，M05 规则 2） */
export const fileTextHelp = (os: PlatformInfo['os']) =>
  `复制 txt、md、csv、json、代码${os === 'macos' ? '、PDF ' : ''}等文件时读入前 64 KB 供搜索，不显示；会让数据库变大`;

/** 设置“片段备份”一行的结果文字（M25 规则 11） */
export const exportNote = (r: { exported: number; skipped: number }, file: string) =>
  `已导出 ${r.exported} 个片段到 ${file}` + (r.skipped ? `；${r.skipped} 个图片、文件片段没有导出` : '');
export const importNote = (r: { imported: number; skipped: number; invalid: number }) => {
  if (!r.imported && !r.skipped && !r.invalid) return '文件里没有片段';
  if (!r.imported && r.skipped && !r.invalid) return `没有新的片段，${r.skipped} 个都已经有了`;
  return `已导入 ${r.imported} 个片段` + (r.skipped ? `，跳过 ${r.skipped} 个已有的` : '') + (r.invalid ? `，${r.invalid} 个名字或内容不合规没有导入` : '');
};
/** 导出文件的默认名：xPaste-片段-年-月-日.json（本地日期） */
export const snippetExportName = (d: Date) =>
  `xPaste-片段-${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}.json`;
