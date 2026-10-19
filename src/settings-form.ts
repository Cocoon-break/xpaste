// 设置表单 ⇄ Settings：校验与解析。纯函数。
import type { Appearance, Settings } from './api/types';

/** 原型 submit 的校验：非空、50–5000、50 的整数倍。 */
export function parseLimit(s: string): number | null {
  const v = Number(s);
  return s.trim() !== '' && v >= 50 && v <= 5000 && v % 50 === 0 ? v : null;
}

export const retentionFromOption = (v: string): number | null => (v === 'forever' ? null : Number(v));
export const optionFromRetention = (d: number | null): string => (d === null ? 'forever' : String(d));

/** 按 `,` 与 `，` 切分、trim、去空、去重保序（区分大小写）。 */
export const parseIgnored = (text: string): string[] => [...new Set(text.split(/[,，]/).map((s) => s.trim()).filter(Boolean))];
export const joinIgnored = (list: string[]): string => list.join(', ');

export type SettingsForm = {
  hotkey: string;
  retain: string;
  limit: number;
  ignored: string;
  skipSensitive: boolean;
  moveToTop: boolean;
  launchAtLogin: boolean;
  appearance: Appearance;
};

export const formToSettings = (base: Settings, f: SettingsForm): Settings => ({
  ...base,
  hotkey: f.hotkey,
  retentionDays: retentionFromOption(f.retain),
  maxRecords: f.limit,
  ignoredApps: parseIgnored(f.ignored),
  skipSensitive: f.skipSensitive,
  moveToTop: f.moveToTop,
  launchAtLogin: f.launchAtLogin,
  appearance: f.appearance,
});
