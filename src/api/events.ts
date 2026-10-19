import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { RecordSummary } from './types';

export const PANEL_SHOWN = 'panel-shown';

/** `trigger`：唤起来源；托盘单击按鼠标时长进入，其余按键盘时长。 */
export type PanelShown = { t0: number | null; trigger: 'hotkey' | 'tray' | 'relaunch' };
export const onPanelShown = (cb: (p: PanelShown) => void): Promise<UnlistenFn> =>
  listen<PanelShown>(PANEL_SHOWN, (e) => cb(e.payload));

export const PANEL_HIDDEN = 'panel-hidden';

export const onPanelHidden = (cb: () => void): Promise<UnlistenFn> => listen(PANEL_HIDDEN, () => cb());

/** 快捷键 / 托盘要隐藏面板：前端淡出后调 `hide_panel`；后端 400 ms 后仍可见会自己隐藏。 */
export const PANEL_HIDE_REQUESTED = 'panel-hide-requested';

export const onPanelHideRequested = (cb: () => void): Promise<UnlistenFn> => listen(PANEL_HIDE_REQUESTED, () => cb());

/** 引导中按下唤起快捷键：面板保持显示，引导据此推进。 */
export const HOTKEY_PRESSED = 'hotkey-pressed';

export const onHotkeyPressed = (cb: () => void): Promise<UnlistenFn> => listen(HOTKEY_PRESSED, () => cb());

export const RECORD_ADDED = 'record-added';
export const RECORD_UPDATED = 'record-updated';
export const RECORDS_REMOVED = 'records-removed';
export const PAUSED_CHANGED = 'paused-changed';

export const onRecordAdded = (cb: (s: RecordSummary) => void): Promise<UnlistenFn> =>
  listen<RecordSummary>(RECORD_ADDED, (e) => cb(e.payload));

export const onRecordUpdated = (cb: (s: RecordSummary) => void): Promise<UnlistenFn> =>
  listen<RecordSummary>(RECORD_UPDATED, (e) => cb(e.payload));

export const onRecordsRemoved = (cb: (ids: number[]) => void): Promise<UnlistenFn> =>
  listen<number[]>(RECORDS_REMOVED, (e) => cb(e.payload));

export const onPausedChanged = (cb: (paused: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>(PAUSED_CHANGED, (e) => cb(e.payload));

/** 托盘“设置…”：面板显示后打开设置弹窗。 */
export const OPEN_SETTINGS = 'open-settings';

export const onOpenSettings = (cb: () => void): Promise<UnlistenFn> => listen(OPEN_SETTINGS, () => cb());

/** 托盘“清空历史…”：面板显示后打开清空确认。 */
export const CONFIRM_CLEAR = 'confirm-clear';

export const onConfirmClear = (cb: () => void): Promise<UnlistenFn> => listen(CONFIRM_CLEAR, () => cb());
