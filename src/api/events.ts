import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { RecordSummary, Settings, TranslateDelta } from './types';

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

/** 引导或对话框打开时按下快捷键，面板保持显示：`panel` 唤起（引导据此推进），`shot` 截图、`rec` 录屏（只在对话框开着且面板有焦点时发）。 */
export const HOTKEY_PRESSED = 'hotkey-pressed';

export type HotkeyRole = 'panel' | 'shot' | 'rec';
export const onHotkeyPressed = (cb: (role: HotkeyRole) => void): Promise<UnlistenFn> =>
  listen<HotkeyRole>(HOTKEY_PRESSED, (e) => cb(e.payload));

/** 截图或录屏缺少屏幕录制权限（后端已唤起面板）；`pane` 是系统设置里那一页的名字。 */
export const SHOT_NEED_PERMISSION = 'shot-need-permission';

export type NeedPermission = { pane: string; mode: 'shot' | 'rec' };
export const onShotNeedPermission = (cb: (p: NeedPermission) => void): Promise<UnlistenFn> =>
  listen<NeedPermission>(SHOT_NEED_PERMISSION, (e) => cb(e.payload));

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

/** 后端改了设置（录屏遮罩的声音开关写回，M24）：载荷是完整设置。 */
export const SETTINGS_CHANGED = 'settings-changed';

export const onSettingsChanged = (cb: (s: Settings) => void): Promise<UnlistenFn> =>
  listen<Settings>(SETTINGS_CHANGED, (e) => cb(e.payload));

/** 托盘“设置…”：面板显示后打开设置弹窗。 */
export const OPEN_SETTINGS = 'open-settings';

export const onOpenSettings = (cb: () => void): Promise<UnlistenFn> => listen(OPEN_SETTINGS, () => cb());

/** 托盘“清空历史…”：面板显示后打开清空确认。 */
export const CONFIRM_CLEAR = 'confirm-clear';

export const onConfirmClear = (cb: () => void): Promise<UnlistenFn> => listen(CONFIRM_CLEAR, () => cb());

/** 翻译增量（后端约每 50 ms 合并一次）：面板翻译时广播，截图翻译时只发给发起的遮罩窗口。 */
export const TRANSLATE_DELTA = 'translate-delta';

export const onTranslateDelta = (cb: (d: TranslateDelta) => void): Promise<UnlistenFn> =>
  listen<TranslateDelta>(TRANSLATE_DELTA, (e) => cb(e.payload));
