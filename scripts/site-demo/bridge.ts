// 官网页面与嵌入的演示页面之间的消息。每个嵌入页的地址带 ?scene=<哪一幕>，来回的消息都报上这个名字，
// 两边按名字认人：WebKit 里 MessageEvent.source 有时不是真正发消息的那个窗口，不能靠它。
export const SCENE = new URLSearchParams(location.search).get('scene') ?? '';

export type Msg = { xp: true; scene: string; type: string; value?: string; [k: string]: unknown };

/** 发给官网页面；单独打开演示页面时没有外层，什么都不做 */
export const tell = (msg: { type: string; [k: string]: unknown }) => {
  if (parent !== window) parent.postMessage({ xp: true, scene: SCENE, ...msg }, '*');
};

/** 官网页面发给这一幕的消息；别的消息返回 null */
export const fromPage = (e: MessageEvent): Msg | null => {
  const m = e.data as Msg | null;
  return m && m.xp === true && m.scene === SCENE && typeof m.type === 'string' ? m : null;
};
