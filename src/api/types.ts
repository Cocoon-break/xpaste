// 与 crates/xpaste-core/src/model.rs、src-tauri/src/commands/*.rs 对应，改字段时同步两边

export type Kind = 'text' | 'link' | 'code' | 'image' | 'video' | 'file';

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
  spectrumCollapsed: boolean;
  frequentMinUses: number;
}

export interface PlatformInfo {
  os: 'macos' | 'windows';
  material: 'glass' | 'opaque';
}

export interface Bootstrap {
  settings: Settings;
  platform: PlatformInfo;
  records: RecordSummary[];
  firstRun: boolean;
  paused: boolean;
}

/** `icon`、`thumb`、`image.src` 为 asset 协议 URL，可直接用作 `<img src>`。 */
export interface AppSummary {
  name: string;
  icon: string | null;
}

export interface RecordSummary {
  id: number;
  kind: Kind;
  title: string;
  app: AppSummary | null;
  /** Unix 毫秒 */
  copiedAt: number;
  uses: number;
  pinned: boolean;
  size: number;
  lang: string | null;
  thumb: string | null;
  durationSec: number | null;
}

export interface FileEntry {
  path: string;
  name: string;
  ext: string;
  size: number;
  exists: boolean;
}

export interface ImageInfo {
  src: string;
  w: number;
  h: number;
}

export interface VideoInfo {
  src: string | null;
  w: number;
  h: number;
  fps: number;
  codec: string;
}

export interface RecordDetail extends RecordSummary {
  text: string | null;
  truncated: boolean;
  linkTitle: string | null;
  files: FileEntry[];
  image: ImageInfo | null;
  video: VideoInfo | null;
}

export interface AppError {
  kind: 'fileMissing' | 'clipboardWrite' | 'hotkeyTaken' | 'invalidSettings' | 'invalid' | 'internal';
  message: string;
}

export type CopyResult =
  | { result: 'copied' }
  | { result: 'copiedOnly'; reason: 'needAccessibility' | 'elevated' | 'pasteFailed' }
  | { result: 'pasted' };
