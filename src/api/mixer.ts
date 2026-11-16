// 搅拌机，对接 src-tauri/src/commands/mixer.rs 与 src-tauri/src/mixer.rs。
// 面板用 mixerPut、onMixerPutSelected；其余命令和 mixer-* 事件给搅拌机页面用。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { HitRect } from './belt';
import type { Pro } from './line';
import type { Recipe } from './types';

export type IngredientKind = 'text' | 'link' | 'code' | 'image' | 'file';
/** 杯里的一样（对应 `xpaste_pro::Ingredient`）：全文不发给页面；`preview` 是悬停提示用的前三行，图片为空串；
 *  图片记着原记录 id，读缩略图用 */
export interface Ingredient { id: number; kind: IngredientKind; title: string; recordId: number | null; preview: string }

export type MixerPhase = 'idle' | 'blending' | 'ready' | 'failed';
/** 这次状态是哪种变化引起的，页面据此挑动效 */
export type MixerCause = 'put' | 'remove' | 'clear' | 'blend' | 'stop' | 'ready' | 'fail' | 'paste' | 'other';

/** `mixer-changed` 的载荷，也是 mixerState 的结果 */
export interface MixerState {
  open: boolean;
  phase: MixerPhase;
  /** 当前这一次的序号：`mixer-delta` 带的 gen 和它不同时是旧请求的，丢掉 */
  gen: number;
  items: Ingredient[];
  /** 打开的配方名，按顺序：前三个在底座上，其余在 ⋯ 里 */
  keys: string[];
  /** 结果卡片的标题：配方名、“配方名 + “那句””或“按你写的：“那句””；没有结果时为 null */
  title: string | null;
  model: string;
  /** 结果正文。搅的时候只是已收到的部分，最后一段不另发 delta：收到 ready、failed 时以它为准 */
  text: string;
  /** failed 时的原因 */
  reason: string | null;
  /** 剪切板里此刻是不是结果 */
  clipIsResult: boolean;
  pro: Pro;
  cause: MixerCause;
  causeId: number | null;
  /** 这次要抖一下 */
  shake: boolean;
}

/** 正在生成的结果又多了一段 */
export interface MixerDelta { gen: number; text: string }

/** 提示条上的按钮 */
export type MixerAction = 'undoClear' | 'undoRemove' | 'retry' | 'settings' | 'again' | 'rename' | 'license' | 'buy';
/** `name`：“改名字”要打开的那个配方 */
export interface MixerToast { text: string; actions: MixerAction[]; name?: string }

export const mixerState = () => invoke<MixerState>('mixer_state');
/** 开始监听事件后调用：后端补发一次当前状态 */
export const mixerReady = () => invoke<void>('mixer_ready');
/** 能点的地方，其余透明区域点击穿透；藏起时后端清掉，每次显示后要重报 */
export const mixerHits = (rects: HitRect[]) => invoke<void>('mixer_hits', { rects });
/** 面板里放进几条记录，按给的顺序排在最后，搅拌机没开就顺便打开 */
export const mixerPut = (ids: number[]) => invoke<void>('mixer_put', { ids });
export const mixerRemove = (id: number) => invoke<void>('mixer_remove', { id });
export const mixerUndoRemove = () => invoke<void>('mixer_undo_remove');
export const mixerClose = () => invoke<void>('mixer_close');
export const mixerUndoClear = () => invoke<void>('mixer_undo_clear');
/** 收起的动效播完、提示条也消失了：仍收着时后端藏窗口。不调的话后端过 5 s 自己藏 */
export const mixerRelease = () => invoke<void>('mixer_release');
/** 补一句展开时拿键盘（true），收起时交还（false） */
export const mixerKeyboard = (on: boolean) => invoke<void>('mixer_keyboard', { on });
/** 自带配方的默认名字和提示词（设置页“恢复默认”用） */
export const mixerDefaults = () => invoke<Recipe[]>('mixer_defaults');
/** 按配方开搅；`recipe` 为 null 时只按补的那句 */
export const mixerBlend = (recipe: string | null, extra: string) => invoke<void>('mixer_blend', { recipe, extra });
export const mixerStop = () => invoke<void>('mixer_stop');
/** 重搅（null）或换配方，都沿用这次补的那句 */
export const mixerReblend = (recipe: string | null) => invoke<void>('mixer_reblend', { recipe });
export const mixerRetry = () => invoke<void>('mixer_retry');
/** 倒掉结果，原料还在 */
export const mixerDump = () => invoke<void>('mixer_dump');
/** 剪切板换成了别的之后，把结果再写回去 */
export const mixerPutBack = () => invoke<void>('mixer_put_back');
/** 贴出后提示条上的“再搅一次” */
export const mixerAgain = () => invoke<void>('mixer_again');
/** 把补的那句存成配方，返回新名字 */
export const mixerSaveRecipe = (text: string) => invoke<string>('mixer_save_recipe', { text });
/** 打开设置的搅拌机页；给了 `edit` 时展开这个配方的编辑 */
export const mixerSettings = (edit: string | null) => invoke<void>('mixer_settings', { edit });

export const onMixerChanged = (cb: (s: MixerState) => void): Promise<UnlistenFn> =>
  listen<MixerState>('mixer-changed', (e) => cb(e.payload));
export const onMixerDelta = (cb: (d: MixerDelta) => void): Promise<UnlistenFn> =>
  listen<MixerDelta>('mixer-delta', (e) => cb(e.payload));
/** 收着时也会发：后端先显示窗口，提示消失后页面调 mixerRelease */
export const onMixerToast = (cb: (t: MixerToast) => void): Promise<UnlistenFn> =>
  listen<MixerToast>('mixer-toast', (e) => cb(e.payload));
/** 面板可见且有键盘时按了搅拌机快捷键（后端只发给面板） */
export const onMixerPutSelected = (cb: () => void): Promise<UnlistenFn> => listen('mixer-put-selected', () => cb());
/** 窗口刚显示出来（只发给搅拌机窗口）：藏起时后端清掉了点击区域，要重报 */
export const onMixerShown = (cb: () => void): Promise<UnlistenFn> =>
  listen('mixer-shown', () => cb(), { target: getCurrentWindow().label });
