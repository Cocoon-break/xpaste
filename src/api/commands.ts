import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type {
  Bootstrap,
  CopyResult,
  OcrProgress,
  RecordDetail,
  SearchHit,
  Settings,
  TranslateStatus,
  Translated,
} from './types';

export const bootstrap = () => invoke<Bootstrap>('bootstrap');
/** `animate`：没有开减少动态效果；macOS 系统材质下后端据此原生淡出整个窗口。 */
export const hidePanel = (animate: boolean) => invoke<void>('hide_panel', { animate });
/** 启动数据已渲染：后端据此放行面板显示，并补发就绪前积压的那次唤起。 */
export const frontendReady = () => invoke<void>('frontend_ready');
/** 有对话框打开时后端不因失焦或快捷键隐藏面板。 */
export const setDialogOpen = (open: boolean) => invoke<void>('set_dialog_open', { open });
export const copy = (id: number, paste: boolean) => invoke<CopyResult>('copy', { id, paste });
/** 只写纯文本，标记为自身写入，不产生新记录。 */
export const copyTextPlain = (text: string) => invoke<void>('copy_text_plain', { text });
export const requestAccessibility = () => invoke<void>('request_accessibility');
export const accessibilityGranted = () => invoke<boolean>('accessibility_granted');
export const requestScreenCapture = () => invoke<void>('request_screen_capture');
export const screenCaptureGranted = () => invoke<boolean>('screen_capture_granted');
/** 设置开着时按截图或录屏快捷键、焦点不在录制框里：开始截图或录屏（面板与对话框保留）。 */
export const overlayBegin = (rec: boolean) => invoke<void>('overlay_begin', { rec });
/** 系统的选择文件夹对话框；取消返回 null。 */
export const pickFolder = (defaultPath?: string) => open({ directory: true, defaultPath }) as Promise<string | null>;
export const openUrl = (url: string) => invoke<void>('open_url', { url });
export const revealFile = (path: string) => invoke<void>('reveal_file', { path });

export const getRecord = (id: number) => invoke<RecordDetail | null>('get_record', { id });
export const search = (q: string) => invoke<SearchHit[]>('search', { q });
export const setPinned = (id: number, on: boolean) => invoke<void>('set_pinned', { id, on });
export const deleteRecords = (ids: number[]) => invoke<void>('delete', { ids });
export const undoDelete = (ids: number[]) => invoke<number>('undo_delete', { ids });
export const saveSnippet = (id: number, name: string) => invoke<number>('save_snippet', { id, name });
export const createSnippet = (name: string, text: string) => invoke<number>('create_snippet', { name, text });
export const updateSnippet = (id: number, name: string, text: string | null) =>
  invoke<void>('update_snippet', { id, name, text });
export const clearHistory = () => invoke<number>('clear_history');
export const countUnpinned = () => invoke<number>('count_unpinned');
/** 只对失败、跳过、本机不支持的记录生效。 */
export const reocr = (id: number) => invoke<void>('reocr', { id });
export const ocrProgress = () => invoke<OcrProgress>('ocr_progress');
export const prunePreview = (retentionDays: number | null, maxRecords: number) =>
  invoke<number>('prune_preview', { retentionDays, maxRecords });
export const saveSettings = (settings: Settings) => invoke<void>('save_settings', { settings });
export const setPaused = (on: boolean) => invoke<void>('set_paused', { on });
export const finishOnboarding = () => invoke<void>('finish_onboarding');
export const installedApps = () => invoke<string[]>('installed_apps');

/** again 为 false 且已有存库译文时直接返回它，不发请求；只有“重新翻译”传 true（M26 规则 4）。 */
export const translateRecord = (id: number, again: boolean) => invoke<Translated>('translate_record', { id, again });
/** 测试连接：用表单里（未保存）的值翻译固定短句，返回译文。 */
export const translateTest = (engine: string, baseUrl: string, apiKey: string, model: string, lang: string) =>
  invoke<string>('translate_test', { engine, baseUrl, apiKey, model, lang });
export const translateStatus = (lang: string) => invoke<TranslateStatus>('translate_status', { lang });
export const openTranslationSettings = () => invoke<void>('open_translation_settings');
