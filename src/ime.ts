// 输入法选字中的按键（面板、划词卡片共用）。WebKit 用 Esc / 回车结束选字时先发 compositionend、再发这次 keydown，
// keydown 的 isComposing 已是 false：再看 keyCode 229（选字中的按键）和刚结束选字的 100 ms。
let composedAt = -Infinity;
addEventListener('compositionend', () => (composedAt = performance.now()), true);

/** 刚结束选字：dialog 的 cancel 事件不带按键，用它判断那次 Esc 是不是用来结束选字的 */
export const justComposed = (): boolean => performance.now() - composedAt < 100;

/** 这次按键归输入法（收起候选、上屏），界面不处理 */
export const composing = (e: KeyboardEvent): boolean => e.isComposing || e.keyCode === 229 || justComposed();
