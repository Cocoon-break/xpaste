// 随状态变化的界面文案（规格 M09、M19）

/** 能自动粘贴（macOS 已授权 / Windows）说“粘贴”，否则说“复制”。 */
export const copyVerb = (canPaste: boolean) => (canPaste ? '粘贴' : '复制');

/** 状态栏 Esc 提示：搜索框有字时 Esc 先清空搜索。 */
export const escHint = (hasQuery: boolean) => (hasQuery ? '清空搜索' : '隐藏');
