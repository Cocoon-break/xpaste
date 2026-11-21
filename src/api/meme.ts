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

/** 一张参考图：历史记录的 id，文件记录里是第几个文件（位图记录为 null） */
export type RefKey = { id: number; file: number | null };

/** 打开或追加到制作窗口（后端发给页面）：`refs` 是追加之后窗口里的全部参考图；`idea` 只在“想表达什么”为空时放进去；
 *  `expired` 时试用已结束，不带参考图和文字，页面只显示过期提示 */
export interface MemeOpen { refs: RefKey[]; idea: string; scene: string | null; draft: string | null; cutout: boolean; expired: boolean }

/** 面板的笑脸按钮、Mod+E：带着这些记录打开（空 = 空白打开）；全都做不了表情包时抛出 AppError */
export const memeOpen = (ids: number[]) => invoke<void>('meme_open', { ids });
/** 页面的 Esc、关闭按钮 */
export const memeClose = () => invoke<void>('meme_close');
/** 页面的参考图变了：后端据此判断再从面板带进来时还能加几张 */
export const memeSetRefs = (refs: RefKey[]) => invoke<void>('meme_set_refs', { refs });
/** 一张参考图的 PNG（动图取第一帧）；原记录没了时抛出 "gone" */
export const memeRefImage = (k: RefKey) => invoke<ArrayBuffer>('meme_ref_image', { id: k.id, file: k.file });
/** “从历史添加”：最近 8 张图片 */
export const memeRecentImages = () => invoke<RefKey[]>('meme_recent_images');

export const onMemeOpen = (cb: (p: MemeOpen) => void): Promise<UnlistenFn> =>
  listen<MemeOpen>('meme-open', (e) => cb(e.payload));
export const onMemeToast = (cb: (text: string) => void): Promise<UnlistenFn> =>
  listen<string>('meme-toast', (e) => cb(e.payload));
/** 窗口关掉了（Esc、关闭按钮、设置里关掉开关）：页面清空 */
export const onMemeClosed = (cb: () => void): Promise<UnlistenFn> => listen('meme-closed', () => cb());
