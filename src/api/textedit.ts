// 改字，对接 src-tauri/src/commands/textedit.rs。类型的形状同 pro/web/textedit/state.ts。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';

export type Pt = [number, number];
/** 左上、右上、右下、左下，原图像素 */
export type Quad = [Pt, Pt, Pt, Pt];
/** 一段，单位是 `te_strip` 那条条带里的 x（TeRanges 的 w × h） */
export type Range = [number, number];
export type Way = 'local' | 'ai';
export type Align = 'left' | 'center' | 'right';

/** 一个候选，图用 teCand 取；没贴好的（placed 为 false）只留着看，不画 */
export interface Cand { file: string; x: number; y: number; by: Way; soft: boolean; note: string; placed: boolean }
/** 一处改动。align 是字数变了时怎么排，“再出一张”照它发；pick 是选中第几个候选，没选中为 NO_PICK（0xffffffff） */
export interface Edit { id: number; line: Quad; range: Range; old: string; new: string; align: Align; pick: number; cands: Cand[] }
/** 一张图；rev 整个进程只增，晚到的旧快照丢掉 */
export interface Job { id: string; rev: number; w: number; h: number; edits: Edit[] }
/** 一个 AI 任务；ahead 是前面还在排队的个数（表情包的也算）；startedAt 是开始的 Unix 毫秒。取消了的不在列表里 */
export interface Task {
  id: number; job: string; edit: number; status: 'waiting' | 'running' | 'done' | 'failed' | 'canceled';
  ahead: number; startedAt: number | null; reason: string; old: string; new: string;
}
/** 读出来的一行；chars 是每个字沿行方向的起止（0–1），取不到时为空 */
export interface Line { quad: Quad; text: string; chars: Range[] }
/** 读字的结果。blocked 是不给改的类别（这时 lines 为空）；locked 是签名的那几段；engine 为 false 是没有识别引擎；
 *  failed 是这张图读不出来，不许改 */
export interface Lines { lines: Line[]; blocked: string | null; locked: Quad[]; engine: boolean; failed: boolean }
/** 走法怎么算的；soft 是本机做得了、但可能有痕迹；本机做不了时没有 soft */
export interface Plan { way: 'local' | 'soft' | 'ai'; why: string; could: Way[]; soft?: boolean }
export type Source = 'shot' | 'clip' | 'panel' | 'file' | 'pin' | 'blank' | 'recent';
/** 一处改动；edit 是从哪一处打开的，后端用这一处替换它。字和范围都没变时，是给那一处再出一张 */
export interface Draft { edit: number | null; line: Quad; range: Range | null; old: string; new: string; align: Align }

/** 设置的“怎么改”，生图接口配好没有 */
export interface TeConfig { mode: Way; api: boolean }
/** 打开窗口或换了一张图。空白窗口、试用结束（“最近改过”除外）时 job 为 null；target 是粘贴目标的应用名 */
export interface TeOpen extends TeConfig { job: Job | null; from: Source; target: string | null; expired: boolean }
/** 改动范围：range、room 是 w × h 那条条带里的 x，m 是原图到条带的矩阵（行优先，除以第三个分量）。
 *  room 是 range 右边的空白到哪儿，可以超出条带；量不出时是条带右端 */
export interface TeRanges { range: Range; room: number; w: number; h: number; m: number[] }
/** 原尺寸的条带。quad 是它的四角，交回改过的条带时带上；left、right 是左右实际多取了多少像素 */
export interface TeStrip { w: number; h: number; m: number[]; quad: Quad; left: number; right: number; rgba: Uint8ClampedArray<ArrayBuffer> }
/** 提交了一处：task 是任务号（本机改的为 null），edit 是那一处的 id */
export interface TeRun { task: number | null; edit: number; job: Job }
/** “最近改过”的一张；done 是画上去的有几处 */
export interface TeRecent { id: string; w: number; h: number; created: number; done: number }
/** 哪一行：读出来的第几行，或者四角 */
export type LineRef = number | Quad;

/* —— 窗口。打不开时都抛出 AppError，message 是给用户看的 —— */
/** 面板的“改字”：这条记录的图，多个文件时是第一个图片文件 */
export const teOpen = (record: number) => invoke<void>('te_open', { record });
/** 窗口里 ⌘V、最近的图：先用 memePasteImage、memeRecentImages 拿到是哪张 */
export const teOpenRecord = (record: number, file: number | null) => invoke<void>('te_open_record', { record, file });
/** 拖进窗口的文件；多个的只用第一个图片，后端另发 te-toast “一次改一张” */
export const teOpenFiles = (paths: string[]) => invoke<void>('te_open_files', { paths });
export const teOpenRecent = (id: string) => invoke<void>('te_open_recent', { id });
/** 页面把事件都听上了：后端这才把（这之前留着的）打开事件发过来 */
export const teReady = () => invoke<void>('te_ready');
/** 打开设置的“改字” */
export const openEditSettings = () => invoke<void>('open_panel_settings', { tab: 'edit' });
export const teClose = () => invoke<void>('te_close');
export const teBack = () => invoke<void>('te_back');
/** 窗口重新拿到焦点：粘贴目标的名字 */
export const teTarget = () => invoke<string | null>('te_target');
export const teConfig = () => invoke<TeConfig>('te_config');

/* —— 图 —— */
/** 原图的 PNG */
export const teImage = (job: string) => invoke<ArrayBuffer>('te_image', { job });
/** 画上选中候选的 PNG */
export const teRender = (job: string) => invoke<ArrayBuffer>('te_render', { job });
/** 一张候选的 PNG；没贴好的是接口出的整张 */
export const teCand = (job: string, file: string) => invoke<ArrayBuffer>('te_cand', { job, file });
/** 一张图此刻的内容（te-tasks 来了先取它） */
export const teJob = (job: string) => invoke<Job>('te_job', { job });
/** 读字的结果，没读完就等 */
export const teLines = (job: string) => invoke<Lines>('te_lines', { job });
/** 第 from 到 to 个字（不含 to）的改动范围；n 是这一行几个字，chars 没有给空数组 */
export const teRanges = (job: string, line: LineRef, n: number, chars: Range[], from: number, to: number) =>
  invoke<TeRanges>('te_ranges', { job, line, n, chars, from, to });
/** 原尺寸的条带，左右按需要的像素多取 */
export async function teStrip(job: string, line: LineRef, extend?: { left: number; right: number }): Promise<TeStrip> {
  const buf = await invoke<ArrayBuffer>('te_strip', { job, line, extend: extend ?? null });
  const at = new Uint8Array(buf).indexOf(10);
  const head = JSON.parse(new TextDecoder().decode(buf.slice(0, at))) as Omit<TeStrip, 'rgba'>;
  return { ...head, rgba: new Uint8ClampedArray(buf.slice(at + 1)) };
}

/* —— 改。不给改的、签名、过期、字数、范围不对的都抛出 AppError —— */
/** 本机的删字、搬字行不行；本机重排行的不用问它 */
export const tePlan = (job: string, edit: Draft) => invoke<Plan>('te_plan', { job, edit });
/** 改一处：AI 写的入队、马上返回，本机的改好才返回。mask 为 false 是不带遮罩 */
export const teRun = (job: string, edit: Draft, way: Way, mask = true) => invoke<TeRun>('te_run', { job, edit, way, mask });
/** 本机重排好的一条：quad 和尺寸是 teStrip 给的那条的 */
export function teLocalPatch(job: string, edit: Draft, quad: Quad, rgba: Uint8ClampedArray): Promise<TeRun> {
  const head = new TextEncoder().encode(JSON.stringify({ job, edit, quad }) + '\n');
  const body = new Uint8Array(head.length + rgba.length);
  body.set(head);
  body.set(rgba, head.length);
  return invoke<TeRun>('te_local_patch', body);
}
/** 选中那一处的第 i 个候选 */
export const tePick = (job: string, edit: number, i: number) => invoke<Job>('te_pick', { job, edit, i });
/** 改回去：候选留着 */
export const teUndo = (job: string, edit: number) => invoke<Job>('te_undo', { job, edit });

/* —— 完成 —— */
/** 完成动作。每种来源做得了哪几个由后端查：截图 copy、repin、saveShotDir；刚复制的图、面板 paste、copy、saveShotDir；
 *  文件 saveBeside、copy、saveAs；贴图 replacePin、copy、saveShotDir；空白窗口、最近改过 copy、saveShotDir */
export type FinishAct = 'paste' | 'copy' | 'saveBeside' | 'saveAs' | 'saveShotDir' | 'repin' | 'replacePin';
/** text 是要弹的提示。then：stay 不动，close 提示完关窗口，saveAs 接着走另存为（存一份的文件夹写不进去） */
export interface FinishOut { text: string | null; then: 'stay' | 'close' | 'saveAs' }
/** 完成，只用已改好的。贴过去的由后端关窗口、提示，返回 stay 且没有 text；saveAs 由后端开系统对话框，取消的也是。
 *  没成的抛出 AppError，窗口不关，改好的还在 */
export const teFinish = (job: string, act: FinishAct) => invoke<FinishOut>('te_finish', { job, act });

/* —— 任务 —— */
export const teTasks = () => invoke<Task[]>('te_tasks');
/** 取消：在改的请求不中止，结果回来存成候选、不选中 */
export const teCancel = (task: number) => invoke<void>('te_cancel', { task });
/** 没改出来的重试，发 1 次请求；mask 为 false 是“不带遮罩再试” */
export const teRetry = (task: number, mask = true) => invoke<void>('te_retry', { task, mask });
/** 去掉改好的、没改出来的 */
export const teDrop = (task: number) => invoke<void>('te_drop', { task });

/* —— 最近改过 —— */
/** 最近 12 张，新的在前 */
export const teRecent = () => invoke<TeRecent[]>('te_recent');
/** 缩略图的 PNG */
export const teThumb = (id: string) => invoke<ArrayBuffer>('te_thumb', { id });
/** 删一张，5 秒内可用 teRecentRestore 撤销 */
export const teRecentRemove = (id: string) => invoke<void>('te_recent_remove', { id });
export const teRecentRestore = (id: string) => invoke<void>('te_recent_restore', { id });
/** 几张、清空能腾出多少字节（可能 0 张但占地方） */
export const teRecentStats = () => invoke<[number, number]>('te_recent_stats');
export const teRecentClear = () => invoke<void>('te_recent_clear');

/* —— 事件 —— */
export const onTeOpen = (cb: (p: TeOpen) => void): Promise<UnlistenFn> => listen<TeOpen>('te-open', (e) => cb(e.payload));
export const onTeClosed = (cb: () => void): Promise<UnlistenFn> => listen('te-closed', () => cb());
/** 任务或图变了，载荷是整份任务列表；先取 teJob 再派发 */
export const onTeTasks = (cb: (tasks: Task[]) => void): Promise<UnlistenFn> => listen<Task[]>('te-tasks', (e) => cb(e.payload));
export const onTeToast = (cb: (text: string) => void): Promise<UnlistenFn> => listen<string>('te-toast', (e) => cb(e.payload));
/** 改字关着时从系统的“打开方式”进来（后端已唤起面板），只发给面板 */
export const onTeOff = (cb: () => void): Promise<UnlistenFn> => listen('te-off', () => cb());
/** 有文件拖进窗口 */
export const onTeDrop = (cb: (d: { type: 'over' } | { type: 'leave' } | { type: 'drop'; paths: string[] }) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent(({ payload: p }) => cb(p.type === 'drop' ? { type: 'drop', paths: p.paths } : p.type === 'leave' ? { type: 'leave' } : { type: 'over' }));
