// 与 crates/xpaste-core/src/model.rs、src-tauri/src/commands/*.rs 对应，改字段时同步两边

export type Kind = 'text' | 'link' | 'code' | 'color' | 'image' | 'video' | 'file';

export type Appearance = 'system' | 'light' | 'dark';

export interface Settings {
  hotkey: string;
  retentionDays: number | null;
  maxRecords: number;
  ignoredApps: string[];
  skipSensitive: boolean;
  moveToTop: boolean;
  launchAtLogin: boolean;
  appearance: Appearance;
  panelSize: [number, number];
  lastFilter: string;
}

export interface PlatformInfo {
  os: 'macos' | 'windows';
}

export interface Bootstrap {
  settings: Settings;
  platform: PlatformInfo;
}

export interface AppError {
  kind: 'fileMissing' | 'clipboardWrite' | 'hotkeyTaken' | 'invalidSettings' | 'internal';
  message: string;
}
