// 表情包制作，对接 src-tauri/src/commands/meme.rs。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { MemeFormat } from '../settings-form';

/** 设置页表单里（未保存）的生图接口配置 */
export interface ImageCfg { format: MemeFormat; baseUrl: string; apiKey: string; model: string }

/** 测试生成：用当前配置生成一张小图，返回 PNG 字节；失败时抛出给用户看的原因（字符串） */
export const memeTestImage = (cfg: ImageCfg) => invoke<ArrayBuffer>('meme_test_image', { cfg });

/** 表情包库的个数与占用字节数 */
export const memeLibraryStats = () => invoke<[number, number]>('meme_library_stats');

/** 一张参考图：历史记录的 id，文件记录里是第几个文件（位图记录为 null）。页面里的草图用负数 id 占位 */
export type RefKey = { id: number; file: number | null };

/** 打开或追加到制作窗口（后端发给页面）：`refs` 是追加之后窗口里的全部参考图；`idea` 只在“想表达什么”为空时放进去；
 *  `scene` 是接着改的做法，`sceneId` 是它在库里的 id；`draft` 是空白打开新窗口时的草稿；
 *  `expired` 时试用已结束，不带参考图和文字，页面只显示过期提示 */
export interface MemeOpen {
  refs: RefKey[]; idea: string; scene: string | null; sceneId: number | null; sceneFinal: boolean; draft: string | null; cutout: boolean; expired: boolean;
}

/** 面板的笑脸按钮、Mod+E：带着这些记录打开（空 = 空白打开）；全都做不了表情包时抛出 AppError */
export const memeOpen = (ids: number[]) => invoke<void>('meme_open', { ids });
/** 页面的 Esc、关闭按钮 */
export const memeClose = () => invoke<void>('meme_close');
export const memeReveal = (id: number) => invoke<void>('meme_reveal', { id });
/** 页面的参考图变了：后端据此判断再从面板带进来时还能加几张 */
export const memeSetRefs = (refs: RefKey[]) => invoke<void>('meme_set_refs', { refs });
/** 一张参考图的 PNG（动图取第一帧）；原记录没了时抛出 "gone" */
export const memeRefImage = (k: RefKey) => invoke<ArrayBuffer>('meme_ref_image', { id: k.id, file: k.file });
/** “从历史添加”：最近 8 张图片 */
export const memeRecentImages = () => invoke<RefKey[]>('meme_recent_images');

/** 一种内置风格：键名、显示名、类别 */
export interface MemeStyle { key: string; name: string; cat: string }
/** 风格库：内置风格，设置里挑过的（键名）和自己写的（描述） */
export interface MemeStyles { styles: MemeStyle[]; used: string[]; custom: string[] }
export const memeStyles = () => invoke<MemeStyles>('meme_styles');
/** 挑过的、自己写的风格变了：后端去重、限个数后写回设置 */
export const memeStylesTouch = (used: string[], custom: string[]) => invoke<void>('meme_styles_touch', { used, custom });

/** 出文案：5 条候选（翻译里配置的 AI 接口）。`sketch` 是第一张为草图时它的文件名，`avoid` 是“换一批”前给过的；失败时抛出原因 */
export const memeCaptions = (idea: string, refs: RefKey[], sketch: string | null, avoid: string[]) =>
  invoke<string[]>('meme_captions', { idea, refs, sketch, avoid });

/** 生成请求：one 一张、set 一套 16 张、set_part 分 4 次里的第 part 次（0–3）、act 以库里 base 那张为底的 AI 动作 */
export interface GenReq {
  kind: 'one' | 'set' | 'set_part' | 'act'; part: number | null; act: string | null;
  /** 内置风格的键名，自定义时为空串 */
  style: string; custom: string | null;
  idea: string; caption: string;
  /** 草图（负数 id）由页面滤掉，另经 `sketch` 带文件名 */
  refs: RefKey[]; sketch: string | null; base: number | null;
}
/** done：库里新存的那几张；irregular：网格没画规整，整张图已存成 id 这一张 */
export type GenOut = { status: 'done'; ids: number[] } | { status: 'irregular'; id: number };
/** 取消或关窗口之后才回来的结果：已进库，页面不显示、不提示 */
export const MEME_LATE = '已取消';
/** 生成，结果立刻进库；失败时抛出给用户看的原因（字符串） */
export const memeGenerate = (req: GenReq) => invoke<GenOut>('meme_generate', { req });
export const memeCancel = () => invoke<void>('meme_cancel');
/** 生图接口配好了没有；这个服务商上次的一套没画规整（数量旁边提示） */
export const memeGenInfo = () => invoke<{ ready: boolean; shaky: boolean }>('meme_gen_info');
/** 抠图贴纸：抠出主体加白边的 PNG；没抠出主体时抛出原因 */
export const memeCutout = (png: Uint8Array) => invoke<ArrayBuffer>('meme_cutout', png);
/** 存草图，返回文件名 */
export const memeSaveSketch = (png: Uint8Array) => invoke<string>('meme_save_sketch', png);
/** 库里一个表情包的第 n 帧 PNG（AI 动作的 GIF）；读不到时抛出 "gone" */
export const memeFrame = (id: number, n: number) => invoke<ArrayBuffer>('meme_frame', { id, n });

/** 库里这几个表情包的名字（一套 16 张时是各格的情绪名）；读不到的为空串 */
export const memeNames = (ids: number[]) => invoke<string[]>('meme_names', { ids });

/** 导出的一级：帧间隔、颜色数、边长 */
export type MemeStep = { delayMs: number; colors: number; side: number };
/** 编码一级（一帧为 PNG，多帧为 GIF），后端只留最近一次；返回它的序号和字节数 */
export function memeEncode(frames: Uint8Array[], step: MemeStep) {
  const head = new TextEncoder().encode(JSON.stringify({ ...step, count: frames.length }) + '\n');
  const body = new Uint8Array(head.length + frames.reduce((n, f) => n + f.length, 0));
  body.set(head);
  let at = head.length;
  for (const f of frames) { body.set(f, at); at += f.length; }
  return invoke<{ token: number; bytes: number }>('meme_encode', body);
}
/** 参与搜索的几段（名字另传） */
export interface SearchParts { idea: string; caption: string; emotions: string[]; styleName: string }
/** 存进表情包库（做好的），copy 时同时放进剪切板（不进历史）；返回库里的 id，失败时抛出原因。
 *  `clipError`：已进库、但没能放进剪切板时的提示 */
export const memeStore = (token: number, name: string, scene: string, searchParts: SearchParts, copy: boolean) =>
  invoke<{ id: number; clipError: string | null }>('meme_store', { token, name, scene, searchParts, copy });
/** 把库里这一个放进剪切板（和上次存的一样时）；不在了时抛出 "gone" */
export const memeCopy = (id: number) => invoke<void>('meme_copy', { id });
/** 接着改：按库里这个表情包的做法打开制作窗口 */
export const memeOpenScene = (id: number) => invoke<void>('meme_open_scene', { id });
/** 留草稿（替换旧的），null 时丢掉 */
export const memeDraftSet = (scene: string | null) => invoke<void>('meme_draft_set', { scene });
/** 做法里一张草图的 PNG；读不到时抛出 "gone" */
export const memeSketchImage = (name: string) => invoke<ArrayBuffer>('meme_sketch_image', { name });

/** 打开设置：meme 是 Pro → 表情包制作（生图接口），translate 是“翻译”（出文案用的 AI 接口） */
export const openMemeSettings = (tab: 'meme' | 'translate') => invoke<void>('open_panel_settings', { tab });

export const onMemeOpen = (cb: (p: MemeOpen) => void): Promise<UnlistenFn> =>
  listen<MemeOpen>('meme-open', (e) => cb(e.payload));
export const onMemeToast = (cb: (text: string) => void): Promise<UnlistenFn> =>
  listen<string>('meme-toast', (e) => cb(e.payload));
/** 制作窗口里存进了表情包（后端已唤起面板）：面板切到“表情包”分类选中这个 id */
export const onMemeReveal = (cb: (id: number) => void): Promise<UnlistenFn> =>
  listen<number>('meme-reveal', (e) => cb(e.payload));
/** 窗口关掉了（Esc、关闭按钮、设置里关掉开关）：页面清空 */
export const onMemeClosed = (cb: () => void): Promise<UnlistenFn> => listen('meme-closed', () => cb());
