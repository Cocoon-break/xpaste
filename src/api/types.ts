// 与 crates/xpaste-core/src/model.rs、src-tauri/src/commands/*.rs 对应，改字段时同步两边

export type Kind = 'text' | 'link' | 'code' | 'image' | 'video' | 'file';

export type Appearance = 'system' | 'light' | 'dark';

/** 录屏清晰度（M24 规则 6） */
export type RecQuality = 'high' | 'standard' | 'small';

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
  ocrEnabled: boolean;
  ocrBackfill: boolean;
  searchFileText: boolean;
  shotHotkey: string;
  shotDir: string;
  shotSaveCopy: boolean;
  recHotkey: string;
  recDir: string;
  recSystemAudio: boolean;
  recMicrophone: boolean;
  recCopy: boolean;
  recQuality: RecQuality;
  /** 30 | 60 */
  recFps: number;
}

export interface PlatformInfo {
  os: 'macos' | 'windows';
  material: 'glass' | 'opaque';
  /** macOS 主版本号；Windows 为 null */
  macosMajor: number | null;
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
  /** 0 待识别 1 有文字 2 无文字 3 失败 4 跳过 5 本机不支持（crates/xpaste-core `ocr_state`） */
  ocrState: number;
}

/** 已识别（有文字或无文字）的图片数 / 图片总数；`supported` 为本机是否有可用识别引擎。 */
export interface OcrProgress {
  done: number;
  total: number;
  supported: boolean;
}

/** `snippet` 只在命中来自识别文字时出现；`mark` 只在按拼音命中时出现（命中的原汉字）。 */
export interface SearchHit {
  id: number;
  snippet: string | null;
  mark: string | null;
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
  ocrText: string | null;
}

export interface AppError {
  kind: 'fileMissing' | 'clipboardWrite' | 'hotkeyTaken' | 'shotHotkeyTaken' | 'recHotkeyTaken' | 'invalidSettings' | 'invalid' | 'internal';
  message: string;
}

export type CopyResult =
  | { result: 'copied' }
  | { result: 'copiedOnly'; reason: 'needAccessibility' | 'elevated' | 'pasteFailed' }
  | { result: 'pasted' };

/** 显示器内的逻辑像素矩形（左上为原点）。对应 `shot::LRect`。 */
export type LRect = { x: number; y: number; w: number; h: number };
/** `shot-begin` 的载荷，对应 `shot::BeginPayload`。 */
export type BeginPayload = {
  index: number;
  generation: number;
  /** 与该屏相交的窗口，由前到后 */
  rects: LRect[];
  /** 只有光标所在那块屏有值 */
  cursor: [number, number] | null;
  scale: number;
  /** 设置“保存截图时同时复制” */
  saveCopy: boolean;
  /** 截图（M23）或选录屏区域（M24） */
  mode: 'shot' | 'rec';
  /** 设置“录制系统声音”，录屏开关的初值 */
  systemAudio: boolean;
  /** 设置“录制麦克风”；本机不支持（macOS 15 以下）为 null */
  microphone: boolean | null;
  /** 录屏输出缩放（按清晰度与该屏缩放），尺寸标签显示输出像素 */
  recScale: number;
  /** 设置“录屏帧率” */
  recFps: number;
};
export type ShotAction = 'copy' | 'save' | 'ocr' | 'pin';
/** 对应 `shot::ShotOutcome`。 */
export type ShotOutcome = { action: ShotAction; chars: number | null; file: string | null; copied: boolean };
