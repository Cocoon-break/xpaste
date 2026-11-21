// 表情包制作，对接 src-tauri/src/commands/meme.rs。
import { invoke } from '@tauri-apps/api/core';
import type { MemeFormat } from '../settings-form';

/** 设置页表单里（未保存）的生图接口配置 */
export interface ImageCfg { format: MemeFormat; baseUrl: string; apiKey: string; model: string }

/** 测试生成：用当前配置生成一张小图，返回 PNG 字节；失败时抛出给用户看的原因（字符串） */
export const memeTestImage = (cfg: ImageCfg) => invoke<ArrayBuffer>('meme_test_image', { cfg });

/** 表情包库的个数与占用字节数 */
export const memeLibraryStats = () => invoke<[number, number]>('meme_library_stats');
