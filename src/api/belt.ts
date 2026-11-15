// 传送带，对接 src-tauri/src/commands/belt.rs 与 src-tauri/src/belt.rs。
// 面板用 beltPut；其余命令和 belt-* 事件给传送带页面用。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Pro } from './line';
import type { Kind } from './types';

/** 带子上的一件（对应 `xpaste_pro::BeltItem`）：文字类的全文不发给页面；图片、文件、视频记着原记录 id，拆出来的件为 null */
export interface BeltItem { id: number; kind: Kind; title: string; recordId: number | null }

/** 这次状态是哪种变化引起的，页面据此挑动效 */
export type BeltCause = 'put' | 'paste' | 'remove' | 'split' | 'back' | 'pick' | 'reorder' | 'clear' | 'other';

/** `belt-changed` 的载荷，也是 beltState 的结果 */
export interface BeltState {
  open: boolean;
  /** 自动模式下从底边探出了一截，还没正式开始 */
  peek: boolean;
  peekCount: number;
  /** 剪切板被别的复制改掉了，下一次粘贴先贴它 */
  yielding: boolean;
  items: BeltItem[];
  next: number | null;
  canBack: boolean;
  pro: Pro;
  cause: BeltCause;
  /** 相关的那件：贴出、拿掉、拆开、退回、点的那件，放上的最后一件 */
  causeId: number | null;
  /** 贴完最后一件时为这次贴出的件数 */
  finished: number | null;
}

/** 提示条上的按钮 */
export type BeltAction = 'undoClear' | 'undoRemove' | 'split' | 'keepWhole' | 'back' | 'license' | 'buy' | 'grant';
/** `id`：拆开、拿掉提示所说的那件 */
export interface BeltToast { text: string; actions: BeltAction[]; id?: number }

/** 窗口内的逻辑像素矩形 */
export interface HitRect { x: number; y: number; w: number; h: number }

export const beltState = () => invoke<BeltState>('belt_state');
/** 开始监听事件后调用：后端补发一次当前状态 */
export const beltReady = () => invoke<void>('belt_ready');
/** 能点的地方，其余透明区域点击穿透；收起时后端清掉，每次显示后要重报 */
export const beltHits = (rects: HitRect[]) => invoke<void>('belt_hits', { rects });
/** 面板里放上几条记录，按给的顺序排在末尾，带子没开就顺便打开 */
export const beltPut = (ids: number[]) => invoke<void>('belt_put', { ids });
export const beltPick = (id: number) => invoke<void>('belt_pick', { id });
/** 必须是带子上现有 id 的一个排列，对不上时后端按现状重发 */
export const beltReorder = (ids: number[]) => invoke<void>('belt_reorder', { ids });
export const beltRemove = (id: number) => invoke<void>('belt_remove', { id });
export const beltUndoRemove = () => invoke<void>('belt_undo_remove');
/** 拆开提示的回答：`accept` 为 false 时整段放着 */
export const beltSplit = (id: number, accept: boolean) => invoke<void>('belt_split', { id, accept });
export const beltBack = () => invoke<void>('belt_back');
export const beltClose = () => invoke<void>('belt_close');
export const beltUndoClear = () => invoke<void>('belt_undo_clear');
/** 探出时点了“开始” */
export const beltStart = () => invoke<void>('belt_start');
/** 收起的动效播完、提示条也消失了：带子仍收着时后端藏窗口，又打开了就不理。不调的话后端过 5 s 自己藏 */
export const beltRelease = () => invoke<void>('belt_release');

/** 广播：面板也要知道 */
export const onBeltChanged = (cb: (s: BeltState) => void): Promise<UnlistenFn> =>
  listen<BeltState>('belt-changed', (e) => cb(e.payload));
/** 带子收着时也会发：后端先显示窗口，提示消失后页面调 beltRelease */
export const onBeltToast = (cb: (t: BeltToast) => void): Promise<UnlistenFn> =>
  listen<BeltToast>('belt-toast', (e) => cb(e.payload));
/** 窗口刚显示出来（只发给传送带窗口）：藏起时后端清掉了点击区域，要重报 */
export const onBeltShown = (cb: () => void): Promise<UnlistenFn> =>
  listen('belt-shown', () => cb(), { target: getCurrentWindow().label });
