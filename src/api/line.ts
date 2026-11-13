// 晾衣绳便签与 Pro 授权，对接 src-tauri/src/commands/line.rs 与 src-tauri/src/line.rs。
// 面板用挂、拖到顶端、Pro 状态与授权码；其余命令和 line-* 事件给晾衣绳页面用。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Kind, LineSide, LineStyle } from './types';

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
  /** 提醒时刻（Unix 秒），没有提醒为 null */
  remindAt: number | null;
  /** 左右两侧时挂在绳子上的高度：便签离第一张默认挂处往下多少逻辑像素；null 按默认依次往下排 */
  sideY: number | null;
}

/** `Note` 加上图片的 asset URL，可直接用作 `<img src>`。 */
export interface NoteDto extends Note {
  imageUrl: string | null;
  /** 只在挂上的那次结果里出现：记录正文超过 64 KB，只挂了前面一段 */
  truncated?: boolean;
}

/** Pro 状态：试用剩余天数（不足一天按一天）、授权邮箱与适用的大版本（订阅没有，为 null）；
 *  试用结束时 `reason` 是存着的授权用不了的原因（要联网续期、被授权服务拒绝、只适用于旧的大版本），没激活过为 null */
export type Pro =
  | { state: 'unavailable' }
  | { state: 'trial'; daysLeft: number }
  | { state: 'licensed'; email: string; major: number | null }
  | { state: 'expired'; reason: string | null };

export interface LineView {
  /** 这个构建带不带晾衣绳 */
  available: boolean;
  pro: Pro;
  notes: NoteDto[];
  /** 已挂着的记录 id：面板据此画刻线 */
  hung: number[];
  /** 便签快捷键的显示标签，没启用为空串 */
  hotkey: string;
  /** 生效的样式：试用结束后是默认样式 */
  style: LineStyle;
  /** 用户开关；与 available（构建带不带）是两回事 */
  on: boolean;
  side: LineSide;
  /** 一直显示：页面不自己收起，把能点的地方报给后端（lineHitRects） */
  pinned: boolean;
  /** 这次启动时便签文件读不全，已留底：页面提示一次 */
  recovered: boolean;
  /** 有购买页可去：设置里显示“购买 xPaste Pro” */
  canBuy: boolean;
}

/** 晾衣绳放下的方式：`peek` 只露绳子，`mouse` 鼠标放下，`flash` 快捷键挂上后亮一下，`kbd` 键盘打开，`drag` 拖着东西到顶端，
 *  `remind` 提醒到点（不拿键盘，随后为每张到期的便签发 line-remind） */
export type How = 'peek' | 'mouse' | 'flash' | 'kbd' | 'drag' | 'remind';

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
/** 左右两侧拖完：新顺序连同每张的高度（Note.sideY）一次写入 */
export const linePlace = (places: [number, number | null][]) => invoke<void>('line_place', { places });
/** 设（`at` 为 Unix 秒）或取消（null）提醒；过去的时刻马上到期 */
export const lineRemind = (id: number, at: number | null) => invoke<NoteDto>('line_remind', { id, at });
/** 返回摘下的便签与它原来的位置；撤销时把 id 交给 lineRestore */
export const lineRemove = (id: number) => invoke<[NoteDto, number]>('line_remove', { id });
/** 撤销摘下：后端放回它留的那份，`at` 不给放回原来的位置 */
export const lineRestore = (id: number, at?: number) => invoke<void>('line_restore', { id, at: at ?? null });
export const lineCopy = (id: number) => invoke<CopyNote>('line_copy', { id });
/** 链接用浏览器打开，文件在访达 / 资源管理器里显示 */
export const lineOpen = (id: number) => invoke<void>('line_open', { id });
/** 大卡片里播放视频：源视频的 asset URL；源文件已移动或删除时为 null */
export const linePreviewSrc = (id: number) => invoke<string | null>('line_preview_src', { id });
/** 面板里拖着的记录：晾衣绳页面画跟手的影子便签用。`thumbUrl` 为图片、视频缩略图的 asset URL */
export type DragItem = { id: number; kind: Kind; title: string; thumbUrl: string | null };
/** 面板里把行拖出顶边：坐标为屏幕逻辑像素。第一次放下绳子，之后把位置和记录转给晾衣绳页面 */
export const lineDragHover = (x: number, y: number, item: DragItem) => invoke<void>('line_drag_hover', { x, y, item });
/** 拖到绳子那一侧松手：坐标为屏幕逻辑像素，晾衣绳页面按 x（顶部）或 y（左右两侧）算出位置后挂上 */
export const lineDragDrop = (id: number, x: number, y: number) => invoke<void>('line_drag_drop', { id, x, y });
/** 设置里“位置”下面的程序坞提示；没有要提示的为空串 */
export const lineSideHint = (side: LineSide) => invoke<string>('line_side_hint', { side });
export const proStatus = () => invoke<Pro>('pro_status');
/** 联网激活；设备满员时错误的 `kind` 为 `deviceLimit`，带已绑定的设备 */
export const proActivate = (code: string) => invoke<Pro>('pro_activate', { code });
/** 设备满员时解绑列表里的一台，随后重新激活 */
export const proUnbind = (code: string, deviceId: number) => invoke<void>('pro_unbind', { code, deviceId });
/** 在这台设备上停用：授权服务解绑本机后清掉本机的授权；连不上时报错，本机的不动 */
export const proDeactivate = () => invoke<Pro>('pro_deactivate');
/** 用浏览器打开购买页 */
export const proBuy = () => invoke<void>('pro_buy');
/** 取走 `xpaste://activate?code=…` 带来的授权码（只给一次），没有为 null */
export const proTakeLink = () => invoke<string | null>('pro_take_link');

// 晾衣绳页面
export const lineReady = () => invoke<void>('line_ready');
/** 按内容报窗口尺寸（逻辑像素）：顶部是高，左右两侧是宽 */
export const lineResize = (extent: number) => invoke<void>('line_resize', { extent });
/** 要收起：后端发 line-hide，页面播完再调 lineHidden */
export const lineHide = () => invoke<void>('line_hide');
export const lineHidden = () => invoke<void>('line_hidden');
/** 一直显示时能点的地方：`[x, y, w, h]`，窗口内逻辑像素，其余地方点击穿透；变了才报 */
export const lineHitRects = (rects: number[][]) => invoke<void>('line_hit_rects', { rects });
/** 拿键盘（true）或交还（false） */
export const lineKeyboard = (on: boolean) => invoke<void>('line_keyboard', { on });
/** 试用结束的提示条上点“输入授权码”：唤起面板并打开设置的 Pro 标签 */
export const openProSettings = () => invoke<void>('open_panel_settings', { tab: 'pro' });

/** 从别的应用拖文件到晾衣绳窗口：`x`、`y` 为窗口内的逻辑坐标 */
export type FileDrag = { type: 'over'; x: number; y: number } | { type: 'drop'; x: number; y: number; paths: string[] } | { type: 'leave' };
export const onLineFileDrag = (cb: (d: FileDrag) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent(({ payload: p }) => {
    if (p.type === 'leave') return cb({ type: 'leave' });
    const x = p.position.x / devicePixelRatio, y = p.position.y / devicePixelRatio;
    cb(p.type === 'drop' ? { type: 'drop', x, y, paths: p.paths } : { type: 'over', x, y });
  });

const target = () => ({ target: getCurrentWindow().label });

/** 便签或 Pro 状态变了，载荷是完整视图（广播） */
export const onLineChanged = (cb: (v: LineView) => void): Promise<UnlistenFn> =>
  listen<LineView>('line-changed', (e) => cb(e.payload));
/** 收到 `xpaste://activate?code=…`：面板已唤起，用 proTakeLink 取授权码 */
export const onProLink = (cb: () => void): Promise<UnlistenFn> => listen('pro-link', () => cb());
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
/** `pass`：一直显示时窗口刚换成点击穿透，页面去掉悬停的样子 */
export const onLinePointer = (cb: (inside: boolean, pass: boolean) => void): Promise<UnlistenFn> =>
  listen<{ inside: boolean; pass?: boolean }>('line-pointer', (e) => cb(e.payload.inside, !!e.payload.pass), target());
/** 便签的提醒到了：每张到期的发一次，页面让它高亮 */
export const onLineRemind = (cb: (id: number) => void): Promise<UnlistenFn> =>
  listen<{ id: number }>('line-remind', (e) => cb(e.payload.id), target());
/** 面板里拖着行又拖回面板松手，绳子一直显示着不收：拿掉影子便签 */
export const onLineDragCancel = (cb: () => void): Promise<UnlistenFn> => listen('line-drag-cancel', () => cb(), target());
/** 面板里拖着行在绳子那一侧松手：记录 id 与光标的屏幕坐标 */
export const onLineDrop = (cb: (id: number, x: number, y: number) => void): Promise<UnlistenFn> =>
  listen<{ id: number; x: number; y: number }>('line-drop', (e) => cb(e.payload.id, e.payload.x, e.payload.y), target());
