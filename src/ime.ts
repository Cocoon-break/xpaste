// 输入法相关的全局处理（面板、划词卡片、截图遮罩都加载）。
// 选字中的按键：WebKit 用 Esc / 回车结束选字时先发 compositionend、再发这次 keydown，
// keydown 的 isComposing 已是 false：再看 keyCode 229（选字中的按键）和刚结束选字的 100 ms。
let composedAt = -Infinity;
/** 原样上屏的拼音（小写字母间夹空格）去掉空格后的样子；不是这种上屏返回 null。
 * 只认小写：输入法候选里的英文词组（“New York”）带大写，不并 */
export const rawPinyin = (s: string): string | null => (/^[a-z]+( [a-z]+)+$/.test(s) ? s.replace(/ /g, '') : null);

// 拼音选字中途切到英文输入法：WebKit 把组字文字原样上屏，音节间的分隔空格也跟着上屏（“fz”成了“f z”）。
// 上屏的是纯字母加空格时去掉空格（M05 规则 7）；正常选字上屏的是汉字，不受影响。
// 输入框与可编辑区（截图文字标注）都管；捕获阶段先于各自的 compositionend 监听，它们读到的已是去掉空格的值。
// 改完补发 input：只听 input 的（划词卡片原文框）在组字时已记下带空格的值
globalThis.addEventListener?.(
  'compositionend',
  (e) => {
    composedAt = performance.now();
    const fixed = rawPinyin(e.data);
    if (fixed !== null) unspace(e.target, e.data, fixed);
  },
  true,
);

/** 把光标前刚上屏的 data 换成 fixed；光标前不是它就不动 */
function unspace(el: EventTarget | null, data: string, fixed: string) {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    const end = el.selectionEnd ?? 0;
    const start = end - data.length;
    if (start < 0 || el.value.slice(start, end) !== data) return;
    el.setRangeText(fixed, start, end, 'end');
  } else if (el instanceof HTMLElement && el.isContentEditable) {
    const sel = getSelection();
    const n = sel?.focusNode;
    const end = sel?.focusOffset ?? 0;
    const start = end - data.length;
    if (!(n instanceof Text) || start < 0 || n.data.slice(start, end) !== data) return;
    n.replaceData(start, data.length, fixed);
    sel!.collapse(n, start + fixed.length);
  } else return;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

/** 刚结束选字：dialog 的 cancel 事件不带按键，用它判断那次 Esc 是不是用来结束选字的 */
export const justComposed = (): boolean => performance.now() - composedAt < 100;

/** 这次按键归输入法（收起候选、上屏），界面不处理 */
export const composing = (e: KeyboardEvent): boolean => e.isComposing || e.keyCode === 229 || justComposed();
