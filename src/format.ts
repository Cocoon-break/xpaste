// 文本格式化与转义：从原型搬运的纯函数，不碰 DOM。
import type { Kind } from './api/types';

const MAP: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
export const esc = (s: unknown): string => String(s).replace(/[&<>"']/g, (c) => MAP[c]);

/** 转义后把 `q`（按字面、忽略大小写）包进 `<mark>`。 */
export const hl = (str: string, q: string): string => {
  q = q.trim();
  if (!q) return esc(str);
  const re = new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'ig');
  return str
    .split(re)
    .map((p, i) => (i % 2 ? `<mark>${esc(p)}</mark>` : esc(p)))
    .join('');
};

const nf = new Intl.NumberFormat('zh-CN');
const nf1 = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 1 });
const rtf = new Intl.RelativeTimeFormat('zh-CN', { numeric: 'auto' });
const dtf = new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });

export const fmtInt = (n: number): string => nf.format(n);
export const fmtDateTime = (t: number): string => dtf.format(t);

const midnight = (t: number) => new Date(t).setHours(0, 0, 0, 0);

/** 与 view.ts 的 groupOf 一致：跨日按本地日历日差（昨天 / 前天 / N天前），同日按整小时。 */
export const rel = (t: number, now = Date.now()): string => {
  const age = now - t;
  if (age < 45_000) return '刚刚';
  if (age < 3_600_000) return rtf.format(-Math.max(1, Math.floor(age / 60_000)), 'minute');
  // 日差四舍五入以吸收夏令时的 23 / 25 小时
  const dayDiff = Math.round((midnight(now) - midnight(t)) / 86_400_000);
  if (dayDiff === 0) return rtf.format(-Math.max(1, Math.trunc(age / 3_600_000)), 'hour');
  return rtf.format(-dayDiff, 'day');
};

const pad = (n: number) => String(n).padStart(2, '0');
export const fmtDur = (sec: number): string => {
  const s = Math.round(sec || 0);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), ss = pad(s % 60);
  return h ? `${h}:${pad(m)}:${ss}` : `${pad(m)}:${ss}`;
};

export const fmtFps = (fps: number): string => `${Number(fps.toFixed(2))} fps`;

/** 原型顺序：编码、W × H、帧率；未知（0 / ''）的项省略。 */
export const videoChips = (v: { w: number; h: number; fps: number; codec: string }): string[] =>
  [v.codec, v.w && v.h ? `${v.w} × ${v.h}` : '', v.fps ? fmtFps(v.fps) : ''].filter(Boolean);

/** WebView 能内联播放的容器。 */
export const canInlinePlay = (ext: string): boolean => ['mp4', 'mov', 'm4v', 'webm'].includes(ext.toLowerCase());

export const fmtBytes = (n: number): string =>
  n < 1024 ? `${n} B` : n < 1048576 ? `${nf1.format(n / 1024)} KB` : `${nf1.format(n / 1048576)} MB`;

export const LABEL: Record<Kind, string> = {
  text: '文本', link: '链接', code: '代码', image: '图片', video: '视频', file: '文件',
};
export type Filter = 'all' | 'fav' | 'snip' | Kind;
export const FILTERS: [Filter, string][] = [['all', '全部'], ['fav', '常用'], ['snip', '片段'], ...(Object.entries(LABEL) as [Kind, string][])];
export const FL = Object.fromEntries(FILTERS) as Record<Filter, string>;

// “常用”不是内容类型，而是按使用次数聚合的视图；阈值 `min` 来自设置 frequentMinUses
export const isSnip = (r: { snippetBody: string | null }): boolean => r.snippetBody !== null;
// 片段不进“常用”（M25 规则 8）
export const isFav = (r: { uses: number; snippetBody: string | null }, min: number): boolean => !isSnip(r) && r.uses >= min;

export const truncate = (s: string, n: number): string => {
  const cs = [...s];
  return cs.length > n ? cs.slice(0, n).join('') + '…' : s;
};

export const host = (url: string): string => {
  try {
    return new URL(url).host;
  } catch {
    return '';
  }
};
export const isHttpUrl = (s: string): boolean => /^https?:\/\//.test(s);

export const TOK = /(\/\/.*|--\s.*)|('(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`)|\b(const|let|var|return|function|if|else|for|while|import|from|export|new|class|await|async|SELECT|FROM|WHERE|ORDER|BY|DESC|ASC|AND|OR|LIMIT|git|pnpm|npm|add)\b|\b(\d+(?:\.\d+)?)\b|(=>|>=|<=|[{}()[\];,=+*\/<>!&|?:])/g;

/** 单行代码的轻量高亮：注释 tc、字符串 ts、关键字 tk、数字 tn、标点 tp。 */
export const hlCode = (line: string): string => {
  let out = '', last = 0;
  for (const m of line.matchAll(TOK)) {
    const off = m.index ?? 0;
    const cls = m[1] ? 'tc' : m[2] ? 'ts' : m[3] ? 'tk' : m[4] ? 'tn' : 'tp';
    out += esc(line.slice(last, off)) + `<span class="${cls}">${esc(m[0])}</span>`;
    last = off + m[0].length;
  }
  return out + esc(line.slice(last));
};
