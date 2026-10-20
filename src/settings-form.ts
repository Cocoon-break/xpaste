// 设置表单 ⇄ Settings：校验与解析。纯函数。
import type { Settings } from './api/types';

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
});
