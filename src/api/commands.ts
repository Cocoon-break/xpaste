import { invoke } from '@tauri-apps/api/core';
import type { Bootstrap, CopyResult } from './types';

export const bootstrap = () => invoke<Bootstrap>('bootstrap');
export const hidePanel = () => invoke<void>('hide_panel');
// 阶段 1 临时命令，阶段 4 由 copy 替换
export const pasteText = (text: string) => invoke<CopyResult>('paste_text', { text });
export const openAccessibilitySettings = () => invoke<void>('open_accessibility_settings');
