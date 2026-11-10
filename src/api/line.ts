// 晾衣绳便签与 Pro 授权，对接 src-tauri/src/commands/line.rs 与 src-tauri/src/line.rs。
// 面板用挂、拖到顶端、Pro 状态与授权码；其余命令和 line-* 事件给晾衣绳页面用。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Kind } from './types';

/** 挂在绳子上的一张便签（对应 `xpaste_pro::Note`）。 */
export interface Note {
  id: number;
  kind: Kind;
  /** 文字、代码的正文；链接的网址；图片、视频、文件的名字 */
  body: string;
  /** 链接标题 */
  title: string | null;
  memo: string | null;
  /** `line/` 下的文件名（图片原图、视频封面） */
  image: string | null;
  size: number | null;
  fileKind: string | null;
  durationSec: number | null;
  /** 文件、视频的源路径 */
  paths: string[];
  recordId: number | null;
  source: string | null;
  /** Unix 秒 */
  createdAt: number;
}

/** `Note` 加上图片的 asset URL，可直接用作 `<img src>`。 */
export interface NoteDto extends Note {
  imageUrl: string | null;
}

/** Pro 状态：试用剩余天数（不足一天按一天）、授权邮箱与适用的大版本 */
export type Pro =
  | { state: 'unavailable' }
  | { state: 'trial'; daysLeft: number }
  | { state: 'licensed'; email: string; major: number }
  | { state: 'expired' };

export interface LineView {
  /** 这个构建带不带晾衣绳 */
  available: boolean;
  pro: Pro;
  notes: NoteDto[];
  /** 已挂着的记录 id：面板据此画刻线 */
  hung: number[];
  /** 便签快捷键的显示标签，没启用为空串 */
  hotkey: string;
}

/** 晾衣绳放下的方式：`peek` 只露绳子，`mouse` 鼠标放下，`flash` 快捷键挂上后亮一下，`kbd` 键盘打开，`drag` 拖着东西到顶端 */
export type How = 'peek' | 'mouse' | 'flash' | 'kbd' | 'drag';

/** `line_copy` 的结果：`copiedPath` 为原记录没了，复制的是文件路径文字 */
export type CopyNote = { result: 'copied' } | { result: 'copiedPath' };

/** 提示条：`action` 为 `pro` 时带“输入授权码”按钮；`id` 是已挂着的那张便签 */
export type LineToast = { text: string; action?: 'pro'; id?: number };

export const lineView = () => invoke<LineView>('line_view');
/** `at` 为插入位置，不给挂在末尾 */
export const lineHangRecord = (id: number, at?: number) => invoke<NoteDto>('line_hang_record', { id, at: at ?? null });
export const lineHangPaths = (paths: string[], at?: number) => invoke<NoteDto[]>('line_hang_paths', { paths, at: at ?? null });
/** 面板的挂钩按钮与便签快捷键：挂上后放下绳子亮一下，没挂成由晾衣绳页面提示；`id` 为 null 挂最新一条 */
export const lineHangFromPanel = (id: number | null) => invoke<void>('line_hang_from_panel', { id });
/** 新写一张空白文字便签 */
export const lineAdd = (at?: number) => invoke<NoteDto>('line_add', { at: at ?? null });
/** 文字、代码改正文，其余改备注；便签已摘下时返回 null */
export const lineEdit = (id: number, body: string | null, memo: string | null) => invoke<NoteDto | null>('line_edit', { id, body, memo });
/** 往手写便签里贴 PNG */
export const linePasteImage = (id: number, png: Uint8Array) =>
  invoke<NoteDto>('line_paste_image', png, { headers: { 'x-note-id': String(id) } });
export const lineReorder = (ids: number[]) => invoke<void>('line_reorder', { ids });
/** 返回摘下的便签与它原来的位置，撤销时交给 lineRestore */
export const lineRemove = (id: number) => invoke<[NoteDto, number]>('line_remove', { id });
export const lineRestore = (note: Note, at: number) => invoke<void>('line_restore', { note, at });
export const lineCopy = (id: number) => invoke<CopyNote>('line_copy', { id });
/** 链接用浏览器打开，文件在访达 / 资源管理器里显示 */
export const lineOpen = (id: number) => invoke<void>('line_open', { id });
/** 大卡片里播放视频：源视频的 asset URL；源文件已移动或删除时为 null */
export const linePreviewSrc = (id: number) => invoke<string | null>('line_preview_src', { id });
/** 面板里拖着的记录：晾衣绳页面画跟手的影子便签用。`thumbUrl` 为图片、视频缩略图的 asset URL */
export type DragItem = { id: number; kind: Kind; title: string; thumbUrl: string | null };
/** 面板里把行拖出顶边：坐标为屏幕逻辑像素。第一次放下绳子，之后把位置和记录转给晾衣绳页面 */
export const lineDragHover = (x: number, y: number, item: DragItem) => invoke<void>('line_drag_hover', { x, y, item });
/** 拖到顶端松手：晾衣绳页面按 x 算出位置后挂上 */
export const lineDragDrop = (id: number, x: number) => invoke<void>('line_drag_drop', { id, x });
export const proStatus = () => invoke<Pro>('pro_status');
export const proActivate = (code: string) => invoke<Pro>('pro_activate', { code });

// 晾衣绳页面
export const lineReady = () => invoke<void>('line_ready');
/** 按内容报窗口高度（逻辑像素） */
export const lineResize = (h: number) => invoke<void>('line_resize', { h });
/** 要收起：后端发 line-hide，页面播完再调 lineHidden */
export const lineHide = () => invoke<void>('line_hide');
export const lineHidden = () => invoke<void>('line_hidden');
/** 拿键盘（true）或交还（false） */
export const lineKeyboard = (on: boolean) => invoke<void>('line_keyboard', { on });
/** 试用结束的提示条上点“输入授权码”：唤起面板并打开设置的 Pro 标签 */
export const openProSettings = () => invoke<void>('open_panel_settings', { tab: 'pro' });

/** 从别的应用拖文件到晾衣绳窗口：`x` 为窗口内的逻辑坐标 */
export type FileDrag = { type: 'over'; x: number } | { type: 'drop'; x: number; paths: string[] } | { type: 'leave' };
export const onLineFileDrag = (cb: (d: FileDrag) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent(({ payload: p }) => {
    if (p.type === 'leave') return cb({ type: 'leave' });
    const x = p.position.x / devicePixelRatio;
    cb(p.type === 'drop' ? { type: 'drop', x, paths: p.paths } : { type: 'over', x });
  });

const target = () => ({ target: getCurrentWindow().label });

/** 便签或 Pro 状态变了，载荷是完整视图（广播） */
export const onLineChanged = (cb: (v: LineView) => void): Promise<UnlistenFn> =>
  listen<LineView>('line-changed', (e) => cb(e.payload));
/** 面板可见时按了便签快捷键：只发给面板，挂选中的记录 */
export const onLineHangSelected = (cb: () => void): Promise<UnlistenFn> =>
  listen('line-hang-selected', () => cb(), target());

// 下面只发给 `line` 窗口
/** `id`：要选中的便签（快捷键挂的内容已经挂着、绳子放下着时，键盘落在它上面） */
export const onLineShow = (cb: (how: How, id: number | null) => void): Promise<UnlistenFn> =>
  listen<{ how: How; id?: number | null }>('line-show', (e) => cb(e.payload.how, e.payload.id ?? null), target());
export const onLineHide = (cb: () => void): Promise<UnlistenFn> => listen('line-hide', () => cb(), target());
export const onLineToast = (cb: (t: LineToast) => void): Promise<UnlistenFn> =>
  listen<LineToast>('line-toast', (e) => cb(e.payload), target());
/** 面板里拖着行到了顶端：光标的屏幕坐标（逻辑像素）与拖着的记录 */
export const onLineDrag = (cb: (x: number, y: number, item: DragItem) => void): Promise<UnlistenFn> =>
  listen<{ x: number; y: number } & DragItem>('line-drag', ({ payload: { x, y, ...item } }) => cb(x, y, item), target());
/** 绳子放下期间光标进出便签区域（晾衣绳窗口往上延到屏幕顶端），变了才发 */
export const onLinePointer = (cb: (inside: boolean) => void): Promise<UnlistenFn> =>
  listen<{ inside: boolean }>('line-pointer', (e) => cb(e.payload.inside), target());
/** 面板里拖着行在顶端松手：记录 id 与光标的屏幕 x */
export const onLineDrop = (cb: (id: number, x: number) => void): Promise<UnlistenFn> =>
  listen<{ id: number; x: number }>('line-drop', (e) => cb(e.payload.id, e.payload.x), target());
