// 替换 @tauri-apps/api/core：按命令名在页面里应答。行为对齐 crates/xpaste-core 的要点
// （搜索片段只给识别文字命中、前后各 12 字；拼音命中带 mark），其余命令能让界面走通即可。
import type { AppError, Bootstrap, Settings } from '../../src/api/types';
import { INITIALS, PINYIN, RECORDS, TRANSLATIONS, mk, summary, type Demo } from './data';
import { tell } from './bridge';
import { emit } from './event';

const q = new URLSearchParams(location.search);

export const settings: Settings = {
  hotkey: 'CommandOrControl+Shift+V',
  retentionDays: null,
  maxRecords: 5000,
  ignoredApps: [],
  skipSensitive: true,
  moveToTop: true,
  launchAtLogin: false,
  updateCheck: false,
  appearance: q.get('theme') === 'light' ? 'light' : q.get('theme') === 'dark' ? 'dark' : 'system',
  panelSize: [960, 640],
  // ?style=compact：精简模式（M27），核对界面用
  panelStyle: q.get('style') === 'compact' ? 'compact' : 'full',
  compactSize: [400, 520],
  lastFilter: 'all',
  // ?spectrum=0：搜索场景里把频谱收起来，列表多露出几行
  spectrumCollapsed: q.get('spectrum') === '0',
  frequentMinUses: 3,
  ocrEnabled: true,
  ocrBackfill: true,
  searchFileText: true,
  shotHotkey: 'Alt+Shift+A',
  shotDir: '',
  shotSaveCopy: true,
  longAutoScroll: false,
  recHotkey: 'Alt+Shift+R',
  recDir: '',
  recSystemAudio: true,
  recMicrophone: false,
  recCopy: true,
  recQuality: 'high',
  recFps: 30,
  translateEngine: 'system',
  translateBaseUrl: '',
  translateApiKey: '',
  translateModel: '',
  translateLang: 'en',
  translateHotkey: 'Alt+Shift+T',
};

const byId = (id: number) => RECORDS.find((r) => r.id === id);
const removed: Demo[] = [];

const haystack = (r: Demo) =>
  [r.title, r.text ?? '', r.linkTitle ?? '', r.app?.name ?? '', ...r.files.map((f) => f.path), r.ocrText ?? '', r.fileText ?? '', r.snippetBody ?? ''].join('\n').toLowerCase();

const snippet = (text: string, needle: string) => {
  const chars = [...text];
  const at = text.toLowerCase().indexOf(needle);
  if (at < 0) return null;
  const first = [...text.slice(0, at)].length;
  const end = first + [...needle].length;
  const from = Math.max(0, first - 12);
  const to = Math.min(chars.length, end + 12);
  return (from > 0 ? '…' : '') + chars.slice(from, to).join('') + (to < chars.length ? '…' : '');
};

/** 纯字母的查询：恰好 2 个字母按首字母，3 个及以上按全拼前缀，命中词表里的词（M05 规则 11） */
const pinyinWords = (n: string) =>
  /^[a-z]+$/.test(n) ? PINYIN.filter(([w, full]) => (n.length === 2 ? INITIALS[w] === n : n.length >= 3 && full.startsWith(n))).map(([w]) => w) : [];

const search = (text: string) => {
  const n = text.trim().toLowerCase();
  if (!n) return [];
  const words = pinyinWords(n);
  const hits = [];
  for (const r of [...RECORDS].sort((a, b) => b.copiedAt - a.copiedAt)) {
    if (haystack(r).includes(n)) {
      hits.push({ id: r.id, snippet: r.ocrText && !r.title.toLowerCase().includes(n) ? snippet(r.ocrText, n) : null, mark: null });
      continue;
    }
    const w = words.find((x) => r.title.includes(x));
    if (w) hits.push({ id: r.id, snippet: null, mark: w });
  }
  return hits;
};

/** 访客在官网页面上复制了一段文字：当成一次新的复制记进演示面板（只在页面里，不读剪切板）。 */
export function addCopied(text: string) {
  const t = text.trim().slice(0, 2000);
  if (!t) return;
  const kind = /^https?:\/\/\S+$/.test(t) ? 'link' : /^[a-z][\w-]*( +-{1,2}[\w-]+| +[\w./~-]+)+$/.test(t) ? 'code' : 'text';
  const r = mk(kind, t, '这个页面', 0);
  RECORDS.push(r);
  void emit('record-added', summary(r));
}

const fail = (kind: AppError['kind'], message: string): never => {
  throw { kind, message } satisfies AppError;
};

async function translate(id: number) {
  const r = byId(id);
  const hit = r && TRANSLATIONS.find(([head]) => r.title.startsWith(head));
  if (!r || !hit) return fail('translate', '演示页面只给几条文字记录准备了译文，试试“周四下午 3 点…”那一条');
  const text = hit[1];
  // 像真实接口一样分段出字
  for (let i = 0; i < text.length; i += 6) {
    await new Promise((ok) => setTimeout(ok, 45));
    void emit('translate-delta', { id, lang: 'en', engine: 'system', text: text.slice(i, i + 6) });
  }
  Object.assign(r, { translation: text, translationLang: 'en', translationEngine: 'system' });
  return { text, lang: 'en', engine: 'system' };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const handlers: Record<string, (a: any) => unknown> = {
  bootstrap: (): Bootstrap => ({
    settings,
    platform: { os: q.get('os') === 'windows' ? 'windows' : 'macos', material: 'glass', macosMajor: 26 },
    records: RECORDS.map(summary),
    firstRun: false,
    paused: false,
  }),
  // 真实应用里后端等这一声才把面板显示出来
  frontend_ready: () => void setTimeout(() => { void emit('panel-shown', { t0: null, trigger: 'hotkey' }); tell({ type: 'ready' }); }, 60),
  get_record: ({ id }) => {
    const r = byId(id);
    if (!r) return null;
    const { fileText: _f, ...d } = r;
    return d;
  },
  search: ({ q: text }) => search(String(text)),
  copy: ({ id }) => {
    const r = byId(id);
    if (r) {
      r.uses += 1;
      void emit('record-updated', summary(r));
      tell({ type: 'pasted', title: r.title.split('\n')[0].slice(0, 24) });
    }
    return { result: 'pasted' };
  },
  set_pinned: ({ id, on }) => {
    const r = byId(id);
    if (r) { r.pinned = on; void emit('record-updated', summary(r)); }
  },
  delete: ({ ids }: { ids: number[] }) => {
    for (const id of ids) {
      const i = RECORDS.findIndex((r) => r.id === id);
      if (i >= 0) removed.push(...RECORDS.splice(i, 1));
    }
    void emit('records-removed', ids);
  },
  undo_delete: ({ ids }: { ids: number[] }) => {
    let n = 0;
    for (const id of ids) {
      const i = removed.findIndex((r) => r.id === id);
      if (i < 0) continue;
      const [r] = removed.splice(i, 1);
      RECORDS.push(r);
      void emit('record-added', summary(r));
      n += 1;
    }
    return n;
  },
  create_snippet: ({ name, text }) => {
    const r = mk('text', name, null, 0, { text, snippetBody: text });
    RECORDS.push(r);
    void emit('record-added', summary(r));
    return r.id;
  },
  save_snippet: ({ id, name }) => {
    const src = byId(id);
    const body = src?.text ?? src?.title ?? '';
    const r = mk(src?.kind ?? 'text', name, null, 0, { text: body, snippetBody: body });
    RECORDS.push(r);
    void emit('record-added', summary(r));
    return r.id;
  },
  update_snippet: ({ id, name, text }) => {
    const r = byId(id);
    if (!r) return;
    r.title = name;
    if (text !== null) { r.text = text; r.snippetBody = text; }
    void emit('record-updated', summary(r));
  },
  clear_history: () => {
    const ids = RECORDS.filter((r) => !r.pinned && r.snippetBody === null).map((r) => r.id);
    handlers.delete({ ids });
    return ids.length;
  },
  count_unpinned: () => RECORDS.filter((r) => !r.pinned && r.snippetBody === null).length,
  prune_preview: () => 0,
  save_settings: ({ settings: s }: { settings: Settings }) => {
    Object.assign(settings, s);
    void emit('settings-changed', settings);
  },
  set_paused: ({ on }) => void emit('paused-changed', on),
  accessibility_granted: () => true,
  screen_capture_granted: () => true,
  ocr_progress: () => ({ done: 3, total: 3, supported: true }),
  installed_apps: () => [],
  hotkey_status: () => ({ panel: false, shot: false, rec: false, translate: false }),
  translate_engines: () => ({ system: true, openai: false, model: '', current: 'system' }),
  translate_status: () => ({ supported: true, pack: 'installed' }),
  translate_record: ({ id }) => translate(id),
  translate_test: () => 'Hello',
  export_logs: () => fail('internal', '演示页面不导出日志'),
  // 划词卡片（card.html）：窗口高度交给父页面去调
  trcard_resize: ({ h }) => void tell({ type: 'card-size', h }),
  trcard_translate: ({ seq }) => ({ seq: Number(seq) + 1, engine: 'system' }),
};

export async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const h = handlers[cmd];
  return (h ? await h(args) : undefined) as T;
}
