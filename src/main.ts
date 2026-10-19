import './styles.css';
import { attachConsole, info } from './api/log';
import { bootstrap, hidePanel } from './api/commands';
import {
  onPanelHidden,
  onPanelShown,
  onPausedChanged,
  onRecordAdded,
  onRecordsRemoved,
  onRecordUpdated,
} from './api/events';
import type { RecordSummary } from './api/types';

const I = {
  text: '<path d="M4 5.5h12M4 10h12M4 14.5h7"/>',
  link: '<path d="M8.5 11.5a3 3 0 0 0 4.2 0l2.6-2.6a3 3 0 0 0-4.2-4.2l-.9.9"/><path d="M11.5 8.5a3 3 0 0 0-4.2 0l-2.6 2.6a3 3 0 0 0 4.2 4.2l.9-.9"/>',
  code: '<path d="M7 6 3 10l4 4M13 6l4 4-4 4"/>',
  color: '<path d="M10 3.5c3 3.4 5 6 5 8.3a5 5 0 0 1-10 0c0-2.3 2-4.9 5-8.3z"/>',
  image: '<rect x="3" y="4" width="14" height="12" rx="2"/><circle cx="7.5" cy="8.5" r="1.5"/><path d="m4 15 4.5-4.5L12 14l2-2 3 3"/>',
  video: '<rect x="2.5" y="5" width="11" height="10" rx="2"/><path d="m13.5 9 4-2.5v7l-4-2.5"/>',
  file: '<path d="M5 3h6.5L15 6.5V17H5z"/><path d="M11 3v4h4"/>',
  star: '<path d="m10 3 2.1 4.4 4.8.6-3.5 3.3.9 4.7L10 13.8 5.7 16l.9-4.7L3.1 8l4.8-.6z"/>',
  play: '<path d="M7 5v10l8-5z" fill="currentColor"/>',
  search: '<circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3"/>',
  sliders: '<path d="M4 6h8M15.5 6h.5M4 14h.5M8 14h8"/><circle cx="13.5" cy="6" r="1.8"/><circle cx="6.5" cy="14" r="1.8"/>',
  pin: '<path d="M7.5 3h5l-.8 5 2.8 3H5.5l2.8-3z"/><path d="M10 11v6"/>',
  pinFill: '<path d="M7.5 3h5l-.8 5 2.8 3H5.5l2.8-3z" fill="currentColor"/><path d="M10 11v6"/>',
  copy: '<rect x="7" y="7" width="9.5" height="9.5" rx="2"/><path d="M13 7V5a1.5 1.5 0 0 0-1.5-1.5h-6A1.5 1.5 0 0 0 4 5v6.5A1.5 1.5 0 0 0 5.5 13H7"/>',
  check: '<path d="m4.5 10.5 3.5 3.5 7.5-8"/>',
  trash: '<path d="M4 6h12M8 6V4h4v2M6 6l.7 10h6.6L14 6"/>',
  more: '<circle cx="5" cy="10" r="1.3" fill="currentColor" stroke="none"/><circle cx="10" cy="10" r="1.3" fill="currentColor" stroke="none"/><circle cx="15" cy="10" r="1.3" fill="currentColor" stroke="none"/>',
  min: '<path d="M5 10h10"/>',
  x: '<path d="m5.5 5.5 9 9M14.5 5.5l-9 9"/>',
  open: '<path d="M11 4h5v5M16 4l-7 7M14 11.5V16H4V6h4.5"/>',
  plus: '<path d="M10 4v12M4 10h12"/>',
  spark: '<path d="M10 3v3M10 14v3M3 10h3M14 10h3M5.5 5.5l2 2M12.5 12.5l2 2M14.5 5.5l-2 2M7.5 12.5l-2 2"/>',
  inbox: '<path d="M3 11h4l1 2h4l1-2h4"/><path d="M5 5h10l2 6v4H3v-4z"/>',
  sun: '<circle cx="10" cy="10" r="3.2"/><path d="M10 2.5v1.8M10 15.7v1.8M2.5 10h1.8M15.7 10h1.8M4.7 4.7 6 6M14 14l1.3 1.3M15.3 4.7 14 6M6 14l-1.3 1.3"/>',
  moon: '<path d="M15.5 12.2A6 6 0 0 1 7.8 4.5a6 6 0 1 0 7.7 7.7z"/>',
  monitor: '<rect x="3" y="4" width="14" height="9.5" rx="1.5"/><path d="M7.5 16.5h5M10 13.5v3"/>',
};
const svg = (n: keyof typeof I, cls = '') =>
  `<svg class="ic ${cls}" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${I[n]}</svg>`;
document.querySelectorAll<HTMLElement>('[data-icon]').forEach((el) => {
  el.innerHTML = svg(el.dataset.icon as keyof typeof I);
  el.style.display = 'inline-grid';
});

const $ = <T extends HTMLElement = HTMLElement>(s: string) => document.querySelector<T>(s)!;
const root = document.documentElement;
const win = $('#win');
const q = $<HTMLInputElement>('#q');
const menu = $('#menu');

// 记录与暂停事件的处理函数，列表、预览等由后续任务填充
const handlers = {
  added: (_s: RecordSummary) => {},
  updated: (_s: RecordSummary) => {},
  removed: (_ids: number[]) => {},
  paused: (_p: boolean) => {},
};

let booted = false;
const shown = (t0: number | null) => {
  win.dataset.shown = 'true';
  if (!booted) {
    // 首次唤起才播放 720ms 编排；之后的快捷键唤起只做 90ms 淡入
    booted = true;
    win.classList.add('boot');
    setTimeout(() => win.classList.remove('boot'), 1400);
  }
  q.value = '';
  q.focus();
  if (import.meta.env.DEV && t0 !== null) {
    requestAnimationFrame(() => void info(`唤起耗时 ${Date.now() - t0} ms`));
  }
};
// 一次隐藏可能收到两次 panel-hidden，这里只写状态，重复调用无副作用
const hidden = () => {
  win.dataset.shown = 'false';
  menu.dataset.open = 'false';
  (document.activeElement as HTMLElement | null)?.blur();
};

await attachConsole();
// 先订阅再 bootstrap，避免两者之间的事件丢失
await onPanelShown(({ t0 }) => shown(t0));
await onPanelHidden(hidden);
await onRecordAdded((s) => handlers.added(s));
await onRecordUpdated((s) => handlers.updated(s));
await onRecordsRemoved((ids) => handlers.removed(ids));
await onPausedChanged((p) => handlers.paused(p));

const b = await bootstrap();
if (b.settings.appearance === 'system') delete root.dataset.theme;
else root.dataset.theme = b.settings.appearance;
root.dataset.material = b.platform.material;
const MOD = b.platform.os === 'macos' ? '⌘' : 'Ctrl';
document.querySelectorAll('.mod').forEach((k) => (k.textContent = MOD));
$('.modp').textContent = `${MOD} P`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);

// 临时：Esc 隐藏，键盘逻辑由后续任务接管
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !e.isComposing) void hidePanel();
});
