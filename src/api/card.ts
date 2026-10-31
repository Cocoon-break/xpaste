// 划词翻译卡片（card.html）与 Rust 侧 src-tauri/src/trcard.rs、commands/translate.rs 的对接（M26 规则 19）。
// 事件只发给 tr-card 窗口；除 again 外的命令都带卡片的 seq，后端据此认出旧卡片迟到的调用并忽略。
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { TRANSLATE_DELTA } from './events';
import type { CardAgain, CardDone, CardShow, TranslateDelta } from './types';

export type { CardAgain, CardDone, CardShow };
export { requestAccessibility, translateDownload, translateEngines } from './commands';

const target = { target: getCurrentWindow().label };

export const onCardShow = (cb: (p: CardShow) => void) => listen<CardShow>('trcard-show', (e) => cb(e.payload), target);
export const onCardDone = (cb: (p: CardDone) => void) => listen<CardDone>('trcard-done', (e) => cb(e.payload), target);
/** 卡片翻译的增量（`id` 为 null）与它的翻译方式标签；面板翻译的广播不收。增量不带 seq。 */
export const onCardDelta = (cb: (text: string, engine: string) => void) =>
  listen<TranslateDelta>(TRANSLATE_DELTA, (e) => { if (e.payload.id === null) cb(e.payload.text, e.payload.engine); }, target);

/** 翻译卡片上的原文 `text`，`lang` 为这张卡片选的目标语言（null 为自动）；`engine` 不给时按设置。
 *  用于输入卡片 ⌘↵、改过原文重翻、换用、下载语言包后重试；新 seq 恒为 seq + 1，`trcard-done` 可能早于返回到达。 */
export const cardTranslate = (seq: number, text: string, lang: string | null, engine?: 'system' | 'openai') =>
  invoke<CardAgain>('trcard_translate', { seq, text, lang, engine: engine ?? null });
/** 复制译文：普通写入剪切板，进历史；不关卡片。 */
export const cardCopy = (text: string) => invoke<void>('trcard_copy', { text });
/** 关闭并把键盘交还原处。 */
export const cardClose = (seq: number) => invoke<void>('trcard_close', { seq });
/** 窗口失去键盘：关闭，不交还键盘；下载语言包期间后端忽略。 */
export const cardBlur = (seq: number) => invoke<void>('trcard_blur', { seq });
/** 下载语言包取消或失败后重新拿键盘。 */
export const cardFocus = (seq: number) => invoke<void>('trcard_focus', { seq });
/** 按内容高度（逻辑像素）改窗口高度，后端限到工作区的 60%；`wide` 展开对照（加宽，限 80%）；
 *  `moved` 用户拖动过卡片，之后左上角留在原处。 */
export const cardResize = (seq: number, h: number, wide: boolean, moved: boolean) => invoke<void>('trcard_resize', { seq, h, wide, moved });
/** 唤起面板并打开设置的 `tab` 标签。先 cardClose 再调，键盘交还的顺序才对。 */
export const openPanelSettings = (tab: string) => invoke<void>('open_panel_settings', { tab });
