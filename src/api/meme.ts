// 表情包制作，对接 src-tauri/src/commands/meme.rs。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { MemeFormat } from '../settings-form';

/** 设置页表单里（未保存）的生图接口配置 */
export interface ImageCfg { format: MemeFormat; baseUrl: string; apiKey: string; model: string }

/** 测试生成：用当前配置生成一张小图，返回 PNG 字节；失败时抛出给用户看的原因（字符串） */
export const memeTestImage = (cfg: ImageCfg) => invoke<ArrayBuffer>('meme_test_image', { cfg });

/** 表情包库的个数与占用字节数 */
export const memeLibraryStats = () => invoke<[number, number]>('meme_library_stats');

/** 一张参考图：历史记录的 id，文件记录里是第几个文件（位图记录为 null）。页面里的草图用负数 id 占位 */
export type RefKey = { id: number; file: number | null };

/** 打开或追加到制作窗口（后端发给页面）：`refs` 是追加之后窗口里的全部参考图；`idea` 只在“想表达什么”为空时放进去；
 *  `scene` 是接着改的做法，`sceneId` 是它在库里的 id；`draft` 是空白打开新窗口时的草稿（画成卡片，点“接着做”才恢复）；
 *  `expired` 时试用已结束，不带参考图和文字，页面只显示过期提示；
 *  `taskId` 不为 null 时是回到 AI 任务的那次（`scene` 是任务里的做法，`sceneId` 为 null），`taskIds` 是那次已画好的结果
 *  （网格没画规整的是整张那一个；在画、排队、没画出来的为空）。平时为 null 和空数组 */
export interface MemeOpen {
  refs: RefKey[]; idea: string; scene: string | null; sceneId: number | null; sceneFinal: boolean; draft: string | null; cutout: boolean; expired: boolean;
  taskId: number | null; taskIds: number[];
  /** 回到任务时那次的种类和是不是网格没画规整（`taskIds` 是整张那一个）；不是回到任务时为 null、false */
  taskKind: TaskView['kind'] | null; taskIrregular: boolean;
  /** 粘贴目标：打开窗口时前台应用的名字（主按钮“贴到<它>”）；从托盘打开、取不到时为 null。macOS 上窗口重新拿到焦点时用 memeTarget 更新 */
  target: string | null;
  /** 这次窗口是从面板里打开的：标题前有“‹ 返回面板”，Esc 回面板 */
  fromPanel: boolean;
}

/** 面板的笑脸按钮、Mod+E：带着这些记录打开（空 = 空白打开）；全都做不了表情包时抛出 AppError */
export const memeOpen = (ids: number[]) => invoke<void>('meme_open', { ids });
/** 关闭圆点、不是从面板来时的 Esc：关窗口，键盘回到原来的应用 */
export const memeClose = () => invoke<void>('meme_close');
/** “‹ 返回面板”、从面板来时的 Esc：关窗口，面板不切分类、不改选中；`draft` 为这次关窗口留了草稿（面板提示一句） */
export const memeBack = (draft: boolean) => invoke<void>('meme_back', { draft });
/** 页面的参考图变了：后端据此判断再从面板带进来时还能加几张 */
export const memeSetRefs = (refs: RefKey[]) => invoke<void>('meme_set_refs', { refs });
/** 一张参考图的 PNG（动图取第一帧）；原记录没了时抛出 "gone" */
export const memeRefImage = (k: RefKey) => invoke<ArrayBuffer>('meme_ref_image', { id: k.id, file: k.file });
/** “从历史添加”：最近 8 张图片 */
export const memeRecentImages = () => invoke<RefKey[]>('meme_recent_images');
/** 窗口里按 ⌘V：只引用已经进了历史的那条，剪切板里是图片（或图片文件）时返回它的参考图（按文件拆开），用不上的为空数组。
 *  暂停记录时、这次没进历史时抛出原因 */
export const memePasteImage = () => invoke<RefKey[]>('meme_paste_image');
/** 拖进窗口的文件：只收真实存在的图片文件，拼成一条记录进历史，按文件拆成参考图。暂停记录时抛出原因 */
export const memeAddFiles = (paths: string[]) => invoke<RefKey[]>('meme_add_files', { paths });
/** 拖文件进窗口松开时：文件的路径 */
export const onMemeFileDrop = (cb: (paths: string[]) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent(({ payload: p }) => { if (p.type === 'drop' && p.paths.length) cb(p.paths); });

/** 一种内置风格：键名、显示名、类别 */
export interface MemeStyle { key: string; name: string; cat: string }
/** 上次的选择：风格、文字样式、文字位置、“更多调整”是否展开 */
export interface MemePrefs { style: string; ts: 'stroke' | 'color' | 'bubble'; pos: 'top' | 'mid' | 'bottom'; more: boolean }
/** 风格库：内置风格，设置里挑过的（键名）和自己写的（描述），以及上次的选择 */
export interface MemeStyles { styles: MemeStyle[]; used: string[]; custom: string[]; prefs: MemePrefs }
export const memeStyles = () => invoke<MemeStyles>('meme_styles');
/** 记住这次的选择：后端校验后写回设置 */
export const memePrefsSet = (p: MemePrefs) => invoke<void>('meme_prefs_set', { ...p });
/** 没有参考图时，按文字挑内置角色的表情键名（BASE_EMOS 之一） */
export const memeBaseEmotion = (text: string) => invoke<string>('meme_base_emotion', { text });
/** 挑过的、自己写的风格变了：后端去重、限个数后写回设置 */
export const memeStylesTouch = (used: string[], custom: string[]) => invoke<void>('meme_styles_touch', { used, custom });

/** 出文案：5 条候选（翻译里配置的 AI 接口）。`sketch` 是第一张为草图时它的文件名，`avoid` 是“换一批”前给过的；失败时抛出原因 */
export const memeCaptions = (idea: string, refs: RefKey[], sketch: string | null, avoid: string[]) =>
  invoke<string[]>('meme_captions', { idea, refs, sketch, avoid });

/** 生成请求：one 一张、set 一套 16 张、set_part 分 4 次里的第 part 次（0–3）、set_parts 分 4 次重画（只用于 AI 任务）、act 以库里 base 那张为底的 AI 动作 */
export interface GenReq {
  kind: 'one' | 'set' | 'set_part' | 'set_parts' | 'act'; part: number | null; act: string | null;
  /** 内置风格的键名，自定义时为空串 */
  style: string; custom: string | null;
  idea: string; caption: string;
  /** 草图（负数 id）由页面滤掉，另经 `sketch` 带文件名 */
  refs: RefKey[]; sketch: string | null; base: number | null;
}
/** 一个 AI 任务（后端排队，不跟窗口走）。`startedMs` 是开始画的 Unix 毫秒，秒数页面自己走；`ahead` 是前面还在排队的个数；
 *  画好的结果在 `ids`，网格没画规整时 `ids` 为空、整张图是 `irregular`；`title`、`scene` 是发起时传的，原样带回 */
export interface TaskView {
  id: number; kind: 'one' | 'set' | 'act'; state: 'queue' | 'run' | 'done' | 'fail';
  title: string; style: string; startedMs: number | null; ahead: number;
  ids: number[]; irregular: number | null; error: string; scene: string;
}
/** 发起一个 AI 任务，返回任务号。`scene` 是做法 JSON，`title` 是列表里显示的那句话。
 *  “分 4 次重画”传 `kind: 'set_parts'`（`part` 为 null），一个任务里顺序画 4 部分拼成 16 张；不认识的 kind 抛出原因 */
export const memeTaskStart = (req: GenReq, scene: string, title: string) => invoke<number>('meme_task_start', { req, scene, title });
/** 取消：排队的直接拿掉；在画的结果回来照样进库，不再显示 */
export const memeTaskCancel = (id: number) => invoke<void>('meme_task_cancel', { id });
/** 没画出来的重试（回到队尾） */
export const memeTaskRetry = (id: number) => invoke<void>('meme_task_retry', { id });
/** 去掉画好的、没画出来的 */
export const memeTaskDrop = (id: number) => invoke<void>('meme_task_drop', { id });
/** 回到那次：按任务里的做法打开制作窗口（发 meme-open，带 taskId、taskIds）；画好又画规整的随之拿掉 */
export const memeTaskOpen = (id: number) => invoke<void>('meme_task_open', { id });
/** 制作窗口重新拿到焦点：粘贴目标的名字（macOS 换成此刻最前面的应用，Windows 是打开时记下的那个）；没有目标时为 null */
export const memeTarget = () => invoke<string | null>('meme_target');
/** 整份任务列表，新的在前 */
export const memeTasks = () => invoke<TaskView[]>('meme_tasks');
/** 任务列表变了（发给制作窗口和面板），载荷是整份列表，新的在前 */
export const onMemeTasks = (cb: (tasks: TaskView[]) => void): Promise<UnlistenFn> =>
  listen<TaskView[]>('meme-tasks', (e) => cb(e.payload));
/** 生图接口配好了没有；这个服务商上次的一套没画规整（数量旁边提示） */
export const memeGenInfo = () => invoke<{ ready: boolean; shaky: boolean }>('meme_gen_info');
/** 抠图贴纸：抠出主体加白边的 PNG；没抠出主体时抛出原因 */
export const memeCutout = (png: Uint8Array) => invoke<ArrayBuffer>('meme_cutout', png);
/** 存草图，返回文件名 */
export const memeSaveSketch = (png: Uint8Array) => invoke<string>('meme_save_sketch', png);
/** 库里一个表情包的第 n 帧 PNG（AI 动作的 GIF）；读不到时抛出 "gone" */
export const memeFrame = (id: number, n: number) => invoke<ArrayBuffer>('meme_frame', { id, n });

/** 库里这几个表情包的名字（一套 16 张时是各格的情绪名）；读不到的为空串 */
export const memeNames = (ids: number[]) => invoke<string[]>('meme_names', { ids });

/** 导出的一级：帧间隔、颜色数、边长 */
export type MemeStep = { delayMs: number; colors: number; side: number };
/** 编码一级（一帧为 PNG，多帧为 GIF），后端只留最近一次；返回它的序号和字节数 */
export function memeEncode(frames: Uint8Array[], step: MemeStep) {
  const head = new TextEncoder().encode(JSON.stringify({ ...step, count: frames.length }) + '\n');
  const body = new Uint8Array(head.length + frames.reduce((n, f) => n + f.length, 0));
  body.set(head);
  let at = head.length;
  for (const f of frames) { body.set(f, at); at += f.length; }
  return invoke<{ token: number; bytes: number }>('meme_encode', body);
}
/** 参与搜索的几段（名字另传） */
export interface SearchParts { idea: string; caption: string; emotions: string[]; styleName: string }
/** 做完之后去哪：paste 放进剪切板、关窗口贴回原来的应用；copy 放进剪切板、关窗口；lib 关窗口、面板选中它 */
export type MemeThen = 'paste' | 'copy' | 'lib';
/** 做完的结果：库里的 id；`clipError` 是已进库、但没能放进剪切板时的提示（这时窗口不关）；
 *  `pasted` 为 false 时不用另外提示（后端已唤起面板，按面板粘贴的降级提示走） */
export interface MemeStoreOut { id: number; clipError: string | null; pasted: boolean }
/** 存进表情包库（做好的），再按 `then` 做完就走（放剪切板不进历史）；失败时抛出原因 */
export const memeStore = (token: number, name: string, scene: string, searchParts: SearchParts, then: MemeThen) =>
  invoke<MemeStoreOut>('meme_store', { token, name, scene, searchParts, then });
/** 和上次存的一样时：不再进库，库里这一个按 `then` 做完就走；不在了时抛出 "gone" */
export const memeCopy = (id: number, then: MemeThen) => invoke<MemeStoreOut>('meme_copy', { id, then });
/** 接着改：按库里这个表情包的做法打开制作窗口 */
export const memeOpenScene = (id: number) => invoke<void>('meme_open_scene', { id });
/** 留草稿（替换旧的），null 时丢掉 */
export const memeDraftSet = (scene: string | null) => invoke<void>('meme_draft_set', { scene });
/** 做法里一张草图的 PNG；读不到时抛出 "gone" */
export const memeSketchImage = (name: string) => invoke<ArrayBuffer>('meme_sketch_image', { name });

/** 打开设置：meme 是 Pro → 表情包制作（生图接口），translate 是“翻译”（出文案用的 AI 接口） */
export const openMemeSettings = (tab: 'meme' | 'translate') => invoke<void>('open_panel_settings', { tab });

export const onMemeOpen = (cb: (p: MemeOpen) => void): Promise<UnlistenFn> =>
  listen<MemeOpen>('meme-open', (e) => cb(e.payload));
export const onMemeToast = (cb: (text: string) => void): Promise<UnlistenFn> =>
  listen<string>('meme-toast', (e) => cb(e.payload));
/** 制作窗口里存进了表情包（后端已唤起面板）：面板切到“表情包”分类选中这个 id */
export const onMemeReveal = (cb: (id: number) => void): Promise<UnlistenFn> =>
  listen<number>('meme-reveal', (e) => cb(e.payload));
/** 制作窗口关掉、唤起了面板（后端发给面板）：返回面板时 `draft` 为留了草稿；贴过去没成时 `reason` 是退成只复制的原因 */
export interface MemeBack { draft: boolean; reason: 'needAccessibility' | 'elevated' | 'pasteFailed' | null }
export const onMemeBack = (cb: (p: MemeBack) => void): Promise<UnlistenFn> =>
  listen<MemeBack>('meme-back', (e) => cb(e.payload));
/** 窗口关掉了（Esc、关闭按钮、设置里关掉开关）：页面清空 */
export const onMemeClosed = (cb: () => void): Promise<UnlistenFn> => listen('meme-closed', () => cb());
