import { invoke } from '@tauri-apps/api/core';
import type { Bootstrap, CopyResult, RecordDetail, Settings } from './types';

export const bootstrap = () => invoke<Bootstrap>('bootstrap');
export const hidePanel = () => invoke<void>('hide_panel');
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
export const setPaused = (on: boolean) => invoke<void>('set_paused', { on });
export const finishOnboarding = () => invoke<void>('finish_onboarding');
