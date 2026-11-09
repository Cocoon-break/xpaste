// 官网演示面板的数据：喂给 src/ 的真实前端。时间都相对页面加载时刻，“2分钟前”这类相对时间才自然。
import type { Kind, RecordDetail, RecordSummary } from '../../src/api/types';

export type Demo = RecordDetail & { fileText?: string };
type Extra = Partial<Demo>;

const now = Date.now();
const MIN = 60e3;
let uid = 0;
export const nextId = () => ++uid;

export const mk = (kind: Kind, title: string, app: string | null, ago: number, extra: Extra = {}): Demo => ({
  id: nextId(),
  kind,
  title,
  app: app ? { name: app, icon: null } : null,
  copiedAt: now - ago * MIN,
  uses: 0,
  pinned: false,
  size: new TextEncoder().encode(extra.text ?? title).length,
  lang: null,
  thumb: null,
  durationSec: null,
  ocrState: 0,
  snippetBody: null,
  text: kind === 'text' || kind === 'code' || kind === 'link' ? title : null,
  truncated: false,
  linkTitle: null,
  files: [],
  image: null,
  video: null,
  ocrText: null,
  translation: null,
  translationLang: null,
  translationEngine: null,
  ...extra,
});

const img = (file: string, w: number, h: number, ocr: string | null, bytes: number): Extra => ({
  thumb: `demo/${file}`,
  image: { src: `demo/${file}`, w, h },
  ocrState: ocr ? 1 : 2,
  ocrText: ocr,
  size: bytes,
});
const file = (dir: string, name: string, size: number): Extra => ({
  files: [{ path: `/Users/demo/${dir}/${name}`, name, ext: name.split('.').pop() ?? '', size, exists: true }],
  size,
});

const MEETING_OCR = ['【产品评审会】xPaste v0.2 发布评审', '时间：10月22日（周三）14:00–15:00', '会议号：482 913 776', '入会密码：0716', '议题：图片文字搜索、频谱缩放、Windows 安装包'].join('\n');
const SLIDE_OCR = ['Q3 复盘', '新增用户 3.2 万，环比 +18%', '次日留存 46%', '下季度目标：Windows 正式版'].join('\n');
const NOTES_TEXT = '# 需求评审纪要\n\n- 搜索：三个字符以上走全文索引\n- 发布：先走灰度方案，内部 20% 用户\n- 风险：Windows OCR 依赖系统语言包\n';

// 列表最上面的主角记录
const HERO: Demo[] = [
  mk('code', 'const debounce = (fn, ms = 200) => {\n  let t;\n  return (...args) => {\n    clearTimeout(t);\n    t = setTimeout(() => fn(...args), ms);\n  };\n};', 'VS Code', 2, { lang: 'JavaScript', uses: 4 }),
  mk('image', '截图 2026-10-20 14.32.png', '截图', 5, img('meeting.webp', 1600, 1000, MEETING_OCR, 412_336)),
  mk('link', 'https://developer.mozilla.org/zh-CN/docs/Web/API/Clipboard_API', 'Chrome', 7, { linkTitle: 'Clipboard API - Web API | MDN', uses: 1 }),
  mk('file', '需求评审纪要.md', '访达', 9, { ...file('Documents', '需求评审纪要.md', 2_184), ocrState: 1, fileText: NOTES_TEXT }),
  mk('text', '周四下午 3 点在 2 号会议室评审 xPaste v0.2，记得带上交互稿和动效参数表。', '微信', 14),
  mk('text', '腾讯会议号 482 913 776', '微信', 20),
  mk('file', '会议室预订表.xlsx', '访达', 26, file('Documents', '会议室预订表.xlsx', 48_211)),
  mk('image', '截图 2026-10-20 13.58.png', '截图', 34, img('slide.webp', 1600, 1000, SLIDE_OCR, 386_120)),
  mk('text', '招商银行 尾号 6688 转账到账 2,400.00 元', '信息', 41),
  mk('video', '屏幕录制 2026-10-20 13.41.mp4', 'xPaste', 52, { ...file('Movies/xPaste', '屏幕录制 2026-10-20 13.41.mp4', 50_541_363), thumb: 'demo/video.webp', durationSec: 83, video: { src: null, w: 1920, h: 1080, fps: 60, codec: 'H.264' } }),
  mk('code', 'git log --oneline --graph -n 20', '终端', 70, { lang: 'Shell', uses: 9 }),
  mk('file', '工商银行回单_10月.pdf', '访达', 88, file('Downloads', '工商银行回单_10月.pdf', 182_400)),
  mk('file', '动效规范-v2.fig', '访达', 96, file('Design', '动效规范-v2.fig', 4_404_019)),
  mk('text', '下周一 10:00 产品同步会，议题：隐私过滤、Windows 安装包体积', '飞书', 130),
  mk('image', '设计稿-搜索态.png', 'Figma', 160, img('design.webp', 1600, 1000, null, 1_204_331)),
  mk('text', 'Wi-Fi：xpaste-studio-5G', '备忘录', 60 * 24 * 3, { uses: 7 }),
  mk('text', '此致\n设计组 · xPaste 团队\n工作日 10:00–19:00', '邮件', 60 * 24 * 6, { pinned: true, uses: 15 }),
  mk('text', '上海市徐汇区漕溪北路 88 号 12 楼', '备忘录', 60 * 24 * 12, { pinned: true, uses: 23 }),
];

// 频谱用的背景记录：24 小时里散布，15–55 分钟前有一簇
const FILLER_TITLES: [Kind, string, string][] = [
  ['text', '好的，我晚点发你', '微信'],
  ['link', 'https://github.com/tauri-apps/tauri', 'Chrome'],
  ['code', 'pnpm tauri build --target aarch64-apple-darwin', '终端'],
  ['text', '报销单已提交，审批人：李经理', '飞书'],
  ['code', 'SELECT id, title FROM records WHERE uses >= 3;', 'DataGrip'],
  ['text', '会议纪要见附件，有问题随时找我', '邮件'],
  ['link', 'https://easings.net/zh-cn', 'Safari'],
  ['text', '顺丰快递 SF1428 3307 6612', '淘宝'],
  ['code', 'cargo test -p xpaste-core', '终端'],
  ['text', '收到，周五前给初稿', '微信'],
];
const rand = (i: number) => {
  const x = Math.sin(i * 12.9898) * 43758.5453;
  return x - Math.floor(x);
};
const FILLER: Demo[] = [];
for (let i = 0; i < 64; i++) {
  const [kind, title, app] = FILLER_TITLES[i % FILLER_TITLES.length];
  const ago = i < 14 ? 15 + rand(i) * 40 : 180 + Math.pow(rand(i + 99), 0.8) * (60 * 23.5 - 180);
  const r = mk(kind, title, app, ago);
  r.size = Math.round(20 + Math.pow(rand(i + 7), 3) * 60_000);
  FILLER.push(r);
}

const SNIPS: Demo[] = [
  mk('text', '收货地址', null, 60 * 24 * 20, { text: '上海市徐汇区漕溪北路 88 号 12 楼', snippetBody: '上海市徐汇区漕溪北路 88 号 12 楼', uses: 6 }),
  mk('text', '请假回复', null, 60 * 24 * 25, { text: '收到，已同步给项目组，祝好。', snippetBody: '收到，已同步给项目组，祝好。', uses: 2 }),
];

export const RECORDS: Demo[] = [...HERO, ...FILLER, ...SNIPS];

export const summary = (r: Demo): RecordSummary => {
  const { text: _t, truncated: _tr, linkTitle: _l, files: _f, image: _i, video: _v, ocrText: _o, fileText: _ft, translation: _a, translationLang: _b, translationEngine: _c, ...s } = r;
  return s;
};

/** 拼音演示用的词表：真实应用按整句索引，这里只照顾演示数据里出现的词（M05 规则 11）。 */
export const PINYIN: [string, string][] = [
  ['会议', 'huiyi'], ['银行', 'yinhang'], ['评审', 'pingshen'], ['纪要', 'jiyao'], ['截图', 'jietu'],
  ['地址', 'dizhi'], ['快递', 'kuaidi'], ['报销', 'baoxiao'], ['预订', 'yuding'], ['回单', 'huidan'],
];
/** 首字母：会议 → hy（每个字取拼音首字母；词表里都是两个字、各一个音节） */
export const INITIALS: Record<string, string> = { 会议: 'hy', 银行: 'yh', 评审: 'ps', 纪要: 'jy', 截图: 'jt', 地址: 'dz', 快递: 'kd', 报销: 'bx', 预订: 'yd', 回单: 'hd' };

/** 面板里按翻译时用的现成译文（按记录标题开头匹配） */
export const TRANSLATIONS: [string, string][] = [
  ['周四下午 3 点', "We're reviewing xPaste v0.2 in Meeting Room 2 at 3 PM on Thursday. Remember to bring the interaction drafts and the motion spec sheet."],
  ['下周一 10:00', 'Product sync next Monday at 10:00. Agenda: privacy filtering and the size of the Windows installer.'],
  ['上海市徐汇区', '12/F, 88 Caoxi North Road, Xuhui District, Shanghai'],
  ['此致', 'Best regards,\nDesign Team · xPaste\nWeekdays 10:00–19:00'],
  ['腾讯会议号', 'Tencent Meeting ID: 482 913 776'],
];
