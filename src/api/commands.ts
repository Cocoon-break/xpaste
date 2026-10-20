import { invoke } from '@tauri-apps/api/core';
import type { Appearance, Bootstrap, CopyResult, RecordDetail, Settings } from './types';

export const bootstrap = () => invoke<Bootstrap>('bootstrap');
/** `animate`：没有开减少动态效果；macOS 系统材质下后端据此原生淡出整个窗口。 */
export const hidePanel = (animate: boolean) => invoke<void>('hide_panel', { animate });
/** 有对话框打开时后端不因失焦或快捷键隐藏面板。 */
export const setDialogOpen = (open: boolean) => invoke<void>('set_dialog_open', { open });
export const copy = (id: number, paste: boolean) => invoke<CopyResult>('copy', { id, paste });
export const copyText = (text: string) => invoke<void>('copy_text', { text });
export const openAccessibilitySettings = () => invoke<void>('open_accessibility_settings');
export const requestAccessibility = () => invoke<void>('request_accessibility');
export const accessibilityGranted = () => invoke<boolean>('accessibility_granted');
export const openUrl = (url: string) => invoke<void>('open_url', { url });
export const revealFile = (path: string) => invoke<void>('reveal_file', { path });

export const getRecord = (id: number) => invoke<RecordDetail | null>('get_record', { id });
export const search = (q: string) => invoke<number[]>('search', { q });
export const setPinned = (id: number, on: boolean) => invoke<void>('set_pinned', { id, on });
export const deleteRecord = (id: number) => invoke<void>('delete', { id });
export const undoDelete = (id: number) => invoke<boolean>('undo_delete', { id });
export const clearHistory = () => invoke<number>('clear_history');
export const countUnpinned = () => invoke<number>('count_unpinned');
export const prunePreview = (retentionDays: number | null, maxRecords: number) =>
  invoke<number>('prune_preview', { retentionDays, maxRecords });
export const saveSettings = (settings: Settings) => invoke<void>('save_settings', { settings });
export const previewAppearance = (appearance: Appearance) => invoke<void>('preview_appearance', { appearance });
export const setPaused = (on: boolean) => invoke<void>('set_paused', { on });
export const finishOnboarding = () => invoke<void>('finish_onboarding');
export const installedApps = () => invoke<string[]>('installed_apps');
