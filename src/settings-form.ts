// 设置表单 ⇄ Settings：校验与解析。纯函数。
import type { PanelStyle, Recipe, RecQuality, Settings } from './api/types';

/** 原型 submit 的校验：非空、50–5000、50 的整数倍。 */
export function parseLimit(s: string): number | null {
  const v = Number(s);
  return s.trim() !== '' && v >= 50 && v <= 5000 && v % 50 === 0 ? v : null;
}

/** 进入“常用”的使用次数：2–99 的整数（与 `Settings::validate` 同口径）。 */
export function parseFrequent(s: string): number | null {
  const v = Number(s);
  return s.trim() !== '' && Number.isInteger(v) && v >= 2 && v <= 99 ? v : null;
}

export const retentionFromOption = (v: string): number | null => (v === 'forever' ? null : Number(v));
export const optionFromRetention = (d: number | null): string => (d === null ? 'forever' : String(d));

/** 按 `,` 与 `，` 切分、trim、去空、去重保序（区分大小写）。 */
export const parseIgnored = (text: string): string[] => [...new Set(text.split(/[,，]/).map((s) => s.trim()).filter(Boolean))];

/** 与 `capture::is_ignored_app` 同口径：去首尾空白、小写后相等。 */
export const sameApp = (a: string, b: string): boolean => a.trim().toLowerCase() === b.trim().toLowerCase();

/** 把输入（可含逗号）逐个追加到清单，已有的（`sameApp`）跳过。返回新数组。 */
export const addIgnored = (list: string[], text: string): string[] =>
  parseIgnored(text).reduce((acc, n) => (acc.some((m) => sameApp(m, n)) ? acc : [...acc, n]), list);

export const removeIgnored = (list: string[], name: string): string[] => list.filter((n) => n !== name);

/** 选择器候选：去空白去重保留首个 → 去掉已选 → 包含过滤 → 前缀匹配在前 → 各段内按中文排序 → 取前 `limit`。 */
export function pickerOptions(apps: string[], chosen: string[], q: string, limit = 8): string[] {
  const needle = q.trim().toLowerCase();
  const uniq: string[] = [];
  for (const a of apps.map((s) => s.trim()).filter(Boolean)) if (!uniq.some((u) => sameApp(u, a))) uniq.push(a);
  const hits = uniq.filter((a) => !chosen.some((c) => sameApp(c, a)) && a.toLowerCase().includes(needle));
  const cmp = (x: string, y: string) => x.localeCompare(y, 'zh-CN');
  const pre = hits.filter((a) => a.toLowerCase().startsWith(needle)).sort(cmp);
  return [...pre, ...hits.filter((a) => !pre.includes(a)).sort(cmp)].slice(0, limit);
}

export type SettingsForm = {
  hotkey: string;
  retain: string;
  limit: number;
  frequent: number;
  ignored: string[];
  skipSensitive: boolean;
  moveToTop: boolean;
  launchAtLogin: boolean;
  updateCheck: boolean;
  ocrEnabled: boolean;
  ocrBackfill: boolean;
  searchFileText: boolean;
  shotHotkey: string;
  shotDir: string;
  shotSaveCopy: boolean;
  longAutoScroll: boolean;
  recHotkey: string;
  recDir: string;
  recSystemAudio: boolean;
  recMicrophone: boolean;
  recCopy: boolean;
  /** 两个 `<select>` 的值 */
  recQuality: string;
  recFps: string;
  panelStyle: PanelStyle;
  lineHotkey: string;
  beltEnabled: boolean;
  beltAuto: boolean;
  beltHotkey: string;
  beltBackHotkey: string;
  mixerEnabled: boolean;
  mixerHotkey: string;
  mixerRecipes: Recipe[];
};

export const formToSettings = (base: Settings, f: SettingsForm): Settings => ({
  ...base,
  hotkey: f.hotkey,
  retentionDays: retentionFromOption(f.retain),
  maxRecords: f.limit,
  frequentMinUses: f.frequent,
  ignoredApps: f.ignored,
  skipSensitive: f.skipSensitive,
  moveToTop: f.moveToTop,
  launchAtLogin: f.launchAtLogin,
  updateCheck: f.updateCheck,
  ocrEnabled: f.ocrEnabled,
  ocrBackfill: f.ocrBackfill,
  searchFileText: f.searchFileText,
  shotHotkey: f.shotHotkey,
  shotDir: f.shotDir,
  shotSaveCopy: f.shotSaveCopy,
  longAutoScroll: f.longAutoScroll,
  recHotkey: f.recHotkey,
  recDir: f.recDir,
  recSystemAudio: f.recSystemAudio,
  recMicrophone: f.recMicrophone,
  recCopy: f.recCopy,
  recQuality: f.recQuality as RecQuality,
  recFps: Number(f.recFps),
  panelStyle: f.panelStyle,
  lineHotkey: f.lineHotkey,
  beltEnabled: f.beltEnabled,
  beltAuto: f.beltAuto,
  beltHotkey: f.beltHotkey,
  beltBackHotkey: f.beltBackHotkey,
  mixerEnabled: f.mixerEnabled,
  mixerHotkey: f.mixerHotkey,
  mixerRecipes: f.mixerRecipes,
});

/** 翻译接口地址（M26 规则 6）：空视为未填；否则必须以 http:// 或 https:// 开头。 */
export const baseUrlError = (v: string): string | null => {
  const t = v.trim();
  return t === '' || /^https?:\/\//i.test(t) ? null : '请输入 http:// 或 https:// 开头的地址';
};

export type MemeFormat = 'openai' | 'ark' | 'gemini';
export interface MemePreset { key: string; label: string; format: MemeFormat; baseUrl: string; model: string }

// 预设待按官方文档核实：地址、模型 ID 和接口形态取自第三方资料，核不实的先去掉
export const MEME_PRESETS: MemePreset[] = [
  { key: 'openai', label: 'OpenAI', format: 'openai', baseUrl: 'https://api.openai.com/v1', model: 'gpt-image-2' },
  { key: 'gemini', label: 'Gemini', format: 'gemini', baseUrl: 'https://generativelanguage.googleapis.com/v1beta', model: 'gemini-3-pro-image-preview' },
  { key: 'ark', label: '豆包（火山方舟）', format: 'ark', baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seedream-4-0-250828' },
  { key: 'qwen', label: '通义（阿里云百炼）', format: 'openai', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-image' },
];
export const memePreset = (key: string): MemePreset | undefined => MEME_PRESETS.find((p) => p.key === key);

export type MemeField = 'format' | 'baseUrl' | 'model' | 'apiKey';
/** 改了格式、地址或模型 ID 以后，服务商变成“自定义”；只改 Key 不变。 */
export const providerAfterEdit = (prev: string, field: MemeField): string => (field === 'apiKey' ? prev : 'custom');

/** 设置页底部一行：表情包库的个数与占用。 */
export const memeStatsText = (count: number, bytes: number): string =>
  `表情包库：${count} 个，占 ${(bytes / 1048576).toFixed(1)} MB`;

/** “文案”一组的状态：翻译的接口地址和模型 ID 都填了才算配好（取表单里未保存的值）。 */
export const memeCaptionState = (baseUrl: string, model: string): string =>
  baseUrl.trim() && model.trim() ? `已配置：${model.trim()}` : '还没配好';
