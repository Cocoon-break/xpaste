// 动效逐项对照（M20）：读 styles.css / main.ts / keymap.ts 源文本，钉住每项的时长、曲线与键盘 / 减少动态效果路径。
// 期望值取自规格 M20 动效清单与原型 .spec 表；值不符时改实现，不改这里。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (p: string) => readFileSync(new URL(p, import.meta.url), 'utf8');
const css = read('./styles.css'), ts = read('./main.ts'), keymap = read('./keymap.ts'), html = read('../index.html');
const panelRs = read('../src-tauri/src/panel.rs');
const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
/** 选择器（精确，整条规则的开头）对应的所有声明块，拼在一起 */
const rule = (sel: string) => {
  const re = new RegExp(`(?:^|[\\n{}])\\s*${esc(sel)}\\s*\\{([^}]*)\\}`, 'g');
  const bodies = [...css.matchAll(re)].map((m) => m[1]);
  assert.ok(bodies.length, `找不到规则 ${sel}`);
  return bodies.join('\n');
};

test('M20 曲线 enter / move / spring 与规格一致（CSS 变量与 main.ts 常量）', () => {
  assert.match(css, /--ease-enter: cubic-bezier\(\.22, 1, \.36, 1\);/);
  assert.match(css, /--ease-move: cubic-bezier\(\.25, 1, \.5, 1\);/);
  assert.match(css, /--ease-spring: cubic-bezier\(\.34, 1\.56, \.64, 1\);/);
  assert.match(ts, /EASE_ENTER = 'cubic-bezier\(\.22,1,\.36,1\)', EASE_MOVE = 'cubic-bezier\(\.25,1,\.5,1\)', EASE_SPRING = 'cubic-bezier\(\.34,1\.56,\.64,1\)'/);
});

test('M20-1 首次打开面板 720ms 上浮缩放 → 光柱 barIn 620ms spring、220ms + i×18ms → 列表 28ms 间隔，只在启动后第一次（有意偏差：系统材质下只位移不缩放，v0.5.1）', () => {
  assert.match(rule('.window.boot'), /animation: panelIn 720ms var\(--ease-enter\) both/);
  assert.match(css, /@keyframes panelIn \{ from \{ opacity: 0; transform: scale\(\.94\) translateY\(24px\); \} \}/);
  // 系统材质：外层窗口圆角由系统画，内层缩放会错位
  assert.match(rule(':root:not([data-material="opaque"]) .window.boot'), /animation-name: panelInGlass/);
  assert.match(css, /@keyframes panelInGlass \{ from \{ opacity: 0; transform: translateY\(8px\); \} \}/);
  assert.match(rule('.window.boot .bar'), /animation: barIn 620ms var\(--ease-spring\) both; animation-delay: calc\(220ms \+ var\(--i\) \* 18ms\)/);
  assert.match(ts, /--i:\$\{staggerIndex\(i, n\)\}/);
  assert.match(rule('.item.stg .row'), /animation: rowIn 420ms var\(--ease-enter\) both; animation-delay: calc\(var\(--base, 0ms\) \+ var\(--i\) \* 28ms\)/);
  assert.match(ts, /if \(!booted\) \{[^}]*booted = true;\s*win\.classList\.add\('boot'\);/);
  assert.match(ts, /runSearch\('', first \? \{ stagger: true, base: 260 \} : \{\}\)/);
});

test('M20-2 快捷键唤起 90ms enter / 托盘单击 320ms spring / 隐藏 160ms ease-out 淡出；macOS 系统材质下透明度由窗口 alpha 承担（进入 90ms、退出 160ms），内容只位移（有意偏差：系统材质下只位移不缩放，同 v0.5.1）', () => {
  // 鼠标唤起（托盘单击）240 / 320ms；键盘唤起 data-instant 90 / 120ms
  assert.match(rule('.window[data-shown="true"]'), /transition: opacity 240ms var\(--ease-enter\), transform 320ms var\(--ease-spring\)/);
  assert.match(rule('.window[data-instant][data-shown="true"]'), /transition-duration: 90ms, 120ms;/);
  // tray → 非 instant，其余来源（快捷键、再次启动、粘贴失败重新显示）→ instant
  assert.match(ts, /win\.toggleAttribute\('data-instant', trigger !== 'tray'\);/);
  assert.match(ts, /onPanelShown\(\(\{ t0, trigger \}\) => shown\(t0, trigger\)\)/);
  assert.match(rule('.window[data-shown="false"]'), /opacity: 0;[^}]*transition: opacity 140ms ease-out, transform 160ms ease-out/);
  assert.match(rule(':root:not([data-material="opaque"]) .window[data-shown="false"]'), /transform: translateY\(10px\);/);
  // 淡出播完再让后端隐藏窗口；macOS 系统材质下立即调，后端在同样 160ms 内淡出整个窗口 alpha（panel.rs fade_out_ms）
  assert.match(ts, /const hide = \(\) => \{[\s\S]*?win\.dataset\.shown = 'false';[\s\S]*?const animate = !reduced\.matches;[\s\S]*?hidePanel\(animate\)[\s\S]*?\}\), animate && !nativeFade\(\) \? 160 : 0\);/);
  assert.match(ts, /const nativeFade = \(\) => S\.platform\.os === 'macos' && S\.platform\.material === 'glass';/);
  // 原生淡出时内容不自己淡出，只位移，窗口 alpha 单独承担整体淡出（不匹配 Windows Mica：nativeFade 要求 macOS）
  assert.match(ts, /root\.toggleAttribute\('data-native-fade', nativeFade\(\)\);/);
  assert.match(rule(':root[data-native-fade] .window[data-shown="false"]'), /opacity: 1;/);
  // 系统材质的透明度在窗口 alpha 上：退出 1 → 0 160ms，进入 0 → 1 90ms（托盘 240ms），内容位移照 CSS 120 / 160ms
  assert.match(panelRs, /\(macos && material != "opaque" && animate\)\.then_some\(160\)/);
  assert.match(panelRs, /\.map\(\|_\| if trigger == Trigger::Tray \{ 240 \} else \{ 90 \}\)/);
  assert.match(panelRs, /let start = if fade_in\.is_some\(\) \{ 0\.0 \} else \{ 1\.0 \};/);
});

test('M20-3 选中项切换 130ms move，换类型色；按住方向键 0ms', () => {
  assert.match(ts, /function moveInd\(\{ instant = false, dur = 130, ease = EASE_MOVE \}/);
  assert.match(ts, /ind\.dataset\.type = r\.kind;/);
  assert.match(ts, /const d = instant \|\| !indShown \|\| reduced\.matches \? 0 : dur;\s*ind\.style\.transition = `transform \$\{d\}ms \$\{ease\}, opacity 160ms ease`;/);
  assert.match(keymap, /if \(vert\) return \{ t: 'move', d: vert, instant: e\.repeat \};/);
  assert.match(ts, /case 'move': return move\(a\.d, a\.instant\);/);
});

test('M20-4 预览切换 180ms enter，下方 4px + 轻微模糊浮入；按住跳过', () => {
  assert.match(ts, /fx\(pbody, \[\{ opacity: 0\.35, transform: 'translateY\(4px\)', filter: 'blur\(2px\)' \}, \{ opacity: 1, transform: 'none', filter: 'blur\(0\)' \}\], \{ duration: 180, easing: EASE_ENTER \}\)/);
  assert.match(ts, /renderPreview\(changed && fade && !instant\);/);
});

test('M20-5 频谱游标 360ms move', () => {
  assert.match(rule('.sp-cursor'), /transition: transform 360ms var\(--ease-move\)/);
  assert.match(ts, /spCursor\.style\.transitionDuration = animate && !reduced\.matches \? '' : '0ms';/);
});

test('M20-6 切换分类 380ms enter，指示条换色，列表逐行浮现；键盘 0ms', () => {
  assert.match(rule('.tabs-over'), /transition: clip-path 380ms var\(--ease-enter\)/);
  assert.match(rule('.tabs-over[data-instant]'), /transition: none/);
  // 颜色带按标签中心铺色标，指示条滑到哪就露出哪种类型色
  assert.match(ts, /tabsOver\.style\.background = `linear-gradient\(90deg, \$\{stops\.join\(', '\)\}\)`/);
  assert.match(ts, /renderList\(\{ stagger: !instant \}\);/);
  assert.match(ts, /case 'filter': return setFilter\(cycleFilter\(S\.filter, a\.d\), true\);/);
  assert.match(ts, /setFilter\(t\.dataset\.f as Filter, e\.detail === 0\)/);
});

test('M20-7 新记录 300ms enter 行高展开、下方行补位 → 520ms spring 落位 → 类型色闪光；新光柱升起 + ping', () => {
  assert.match(rule('.item'), /transition: grid-template-rows 300ms var\(--ease-enter\)/);
  assert.match(css, /@starting-style \{\s*\.item\.is-new \{ grid-template-rows: 0fr; \}\s*\.item\.is-new \.row \{ opacity: 0; transform: translateY\(-12px\) scale\(\.97\); \}\s*\}/);
  assert.match(rule('.item.is-new .row'), /transition: opacity 260ms var\(--ease-enter\) 60ms, transform 520ms var\(--ease-spring\) 60ms; animation: flash 1400ms ease-out 200ms/);
  // 旧 WebKit 没有 @starting-style 的兜底，同样 520ms spring
  assert.match(ts, /fx\(row, \[\{ transform: 'translateY\(-12px\) scale\(\.97\)' \}, \{ transform: 'none' \}\], \{ duration: 520, easing: EASE_SPRING, delay: 60, fill: 'backwards' \}\)/);
  assert.match(ts, /const ENTER_GLIDE = `transform 300ms \$\{EASE_ENTER\}`;/);
  assert.match(ts, /renderList\(\{ scroll: restored, glide: S\.shown \? ENTER_GLIDE : '' \}\);\s*renderSpectrum\(s\.id\);/);
  assert.match(rule('.bar.fresh'), /animation: barIn 700ms var\(--ease-spring\) both, ping 1200ms ease-out 200ms/);
});

test('M20-8 复制成功 760ms move 类型色光扫；复制按钮对勾弹入', () => {
  assert.match(rule('.row.beam::after'), /animation: beam 760ms var\(--ease-move\) var\(--beam-at, 0ms\) forwards/);
  assert.match(ts, /const BEAM_MS = 760;/);
  assert.match(rule('.swap > *'), /transition: opacity 180ms ease-out, transform 320ms var\(--ease-spring\), filter 180ms ease-out/);
  assert.match(css, /\.swap\[data-state="a"\] > \[data-s="b"\], \.swap\[data-state="b"\] > \[data-s="a"\] \{ opacity: 0; transform: scale\(\.3\) rotate\(-35deg\); filter: blur\(3px\); \}/);
  assert.match(ts, /const sw = copyBtn\.querySelector<HTMLElement>\('\.swap'\)!;\s*sw\.dataset\.state = 'b';/);
});

test('M20-9 删除 180ms ease-out 左滑模糊 + 100ms 后 240ms move 收起；撤销倒计时圈 5s', () => {
  assert.match(rule('.item.is-removing .row'), /opacity: 0; transform: translateX\(-32px\); filter: blur\(4px\); transition: opacity 180ms ease-out, transform 240ms var\(--ease-move\), filter 180ms ease-out/);
  assert.match(rule('.item.is-removing'), /grid-template-rows: 0fr; transition: grid-template-rows 240ms var\(--ease-move\) 100ms/);
  assert.match(ts, /paintRows\(\{ glide: `transform 240ms \$\{EASE_MOVE\} 100ms`, glideBusy: `transform 240ms \$\{EASE_MOVE\}` \}\)/);
  assert.match(read('./view.ts'), /export const UNDO_MS = 5000;/);
  assert.match(ts, /ring\.animate\(\[\{ strokeDashoffset: 0 \}, \{ strokeDashoffset: 44 \}\], \{ duration: UNDO_MS, fill: 'forwards' \}\)/);
});

test('M20-10 置顶（鼠标）300ms move FLIP，高亮框跟随；图钉旋转弹入；键盘直接到位', () => {
  assert.match(ts, /el\.animate\(\[\{ transform: `translateY\(\$\{dy\}px\) scale\(1\.02\)` \}, \{ transform: 'translateY\(0\) scale\(1\)' \}\], \{ duration: 300, easing: EASE_MOVE \}\)/);
  assert.match(ts, /ind\.style\.transition = `transform 300ms \$\{EASE_MOVE\}, opacity 160ms ease`;/);
  // 图钉用 .swap（scale(.3) rotate(-35deg) → 320ms spring），鼠标触发才动画
  assert.match(ts, /if \(id === S\.sel\) setPin\(pinned, !animate\);/);
  assert.match(ts, /const r0 = animate && !reduced\.matches \? rowEl\(id\)\?\.getBoundingClientRect\(\) : undefined;/);
  assert.match(ts, /case 'pin': return void \(sel !== null && togglePin\(sel, false\)\);/);
});

test('M20-11 右键菜单 260ms spring / 120ms ease-out，从指针所在角展开，菜单项间隔 24ms；键盘即时', () => {
  assert.match(rule('.menu[data-open="true"]'), /transition: opacity 180ms var\(--ease-enter\), transform 260ms var\(--ease-spring\), visibility 0s/);
  assert.match(rule('.menu'), /opacity: 0; transform: scale\(\.94\); visibility: hidden;\s*transition: opacity 120ms ease-out, transform 120ms ease-out, visibility 0s 120ms/);
  assert.match(rule('.menu[data-open="true"] button'), /transition-delay: calc\(var\(--i\) \* 24ms\), calc\(var\(--i\) \* 24ms\), 0ms/);
  assert.match(ts, /menu\.style\.transformOrigin = `\$\{ox\} \$\{oy\}`;/);
  assert.match(ts, /menu\.toggleAttribute\('data-instant', viaKb\);/);
  assert.match(rule('.menu[data-instant], .menu[data-instant] button'), /transition-duration: 0s; transition-delay: 0s;/);
});

test('M20-12 设置对话框 420ms spring / 150ms ease-out，分组间隔 45ms；开关滑块回弹', () => {
  assert.match(rule('dialog.is-shown'), /transition: opacity 220ms var\(--ease-enter\), transform 420ms var\(--ease-spring\)/);
  assert.match(rule('dialog'), /opacity: 0; transform: scale\(\.94\) translateY\(10px\);\s*transition: opacity 150ms ease-out, transform 150ms ease-out/);
  assert.match(rule('dialog.is-shown .dlg-body > *'), /animation: rowIn 460ms var\(--ease-enter\) both; animation-delay: calc\(60ms \+ var\(--i\) \* 45ms\)/);
  assert.match(rule('.switch input::after'), /transition: transform 360ms var\(--ease-spring\)/);
  assert.match(ts, /closeTimer = window\.setTimeout\(\(\) => dlg\.close\(\), reduced\.matches \? 0 : 150\);/);
});

test('M20-13 悬停 0 / 140ms ease-out；快捷按钮从右侧滑入；视频封面播放钮放大', () => {
  assert.match(rule('.row::before'), /opacity: 0; transition: opacity 140ms ease-out/);
  assert.match(rule('.row:hover::before'), /opacity: 1; transition-duration: 0ms/);
  assert.match(rule('.qa'), /transform: translate\(8px, -50%\); opacity: 0;[^}]*transition: opacity 120ms ease-out, transform 200ms var\(--ease-enter\)/);
  assert.match(rule('.row:hover .qa'), /opacity: 1; transform: translate\(0, -50%\)/);
  assert.match(rule('.poster .play'), /transition: transform 320ms var\(--ease-spring\)/);
  assert.match(rule('.poster:hover .play'), /transform: scale\(1\.1\)/);
});

test('M20-14 提示条 420ms spring 弹入 / 160ms ease-out 退出；连续触发只换字', () => {
  assert.match(rule('.toast[data-open="true"]'), /transition: opacity 220ms var\(--ease-enter\), transform 420ms var\(--ease-spring\), visibility 0s/);
  assert.match(rule('.toast'), /transform: translate\(-50%, 8px\) scale\(\.96\); visibility: hidden;\s*transition: opacity 160ms ease-out, transform 160ms ease-out, visibility 0s 160ms/);
  // 已打开时不重播弹入，只让文字浮入
  assert.match(ts, /if \(toastEl\.dataset\.open === 'true'\) fx\(toastMsg, /);
});

test('M20-15 切换主题：点选即时切换并立即保存；data-theme-switching 两帧内关闭全部过渡，各元素不各自渐变；圆形扩散已删除（阶段 9 F01）', () => {
  assert.match(css, /\[data-theme-switching\] \*, \[data-theme-switching\] \*::before, \[data-theme-switching\] \*::after \{ transition: none !important; \}/);
  assert.match(ts, /function setTheme\(mode: Appearance\) \{\s*root\.setAttribute\('data-theme-switching', ''\);[\s\S]*?requestAnimationFrame\(\(\) => requestAnimationFrame\(\(\) => root\.removeAttribute\('data-theme-switching'\)\)\);/);
  // 不防抖、不预览：先改本地设置再保存；串行保存，每次发当时最新的设置；失败回到最近已确认落盘的外观
  assert.match(ts, /async function applyAppearance\(mode: Appearance\) \{[\s\S]*?S\.settings = \{ \.\.\.S\.settings, appearance: mode \};\s*setTheme\(mode\);\s*const save = \(themeSave = themeSave\.catch\(\(\) => \{\}\)\.then\(\(\) => \{\s*const s = S\.settings;\s*return saveSettings\(s\)\.then\(\(\) => void \(savedAppearance = s\.appearance\)\);[\s\S]*?await save;[\s\S]*?S\.settings = \{ \.\.\.S\.settings, appearance: savedAppearance \};\s*setTheme\(S\.settings\.appearance\);/);
  assert.match(ts, /S\.settings = b\.settings;\nsavedAppearance = b\.settings\.appearance;/);
  // 按住方向键只切一次
  assert.match(ts, /themeSeg\.addEventListener\('keydown', [\s\S]*?e\.stopPropagation\(\);\s*\/\/ [^\n]*\n\s*if \(e\.repeat\) return;/);
  assert.match(ts, /if \(b\) void applyAppearance\(b\.dataset\.themeOpt as Appearance\);/);
  assert.match(html, /<span><b id="themeLabel">外观<\/b><small>点选即生效<\/small><\/span>/);
  // 圆形扩散、预览与关闭还原都已删除
  assert.doesNotMatch(ts, /startViewTransition|view-transition|circle\(|themePending|previewWindowTheme|previewAppearance/);
  assert.doesNotMatch(css, /view-transition/);
});

test('M20-16 描边跟随指针：.glow 读指针坐标变量，悬停时显现', () => {
  assert.match(html, /<div class="glow" id="glow" aria-hidden="true"><\/div>/);
  assert.match(rule('.glow'), /radial-gradient\(260px circle at var\(--mx, 50%\) var\(--my, -200px\)/);
  assert.match(rule('.window:hover .glow'), /opacity: 1/);
  assert.match(ts, /win\.addEventListener\('pointermove'[\s\S]*?requestAnimationFrame\([\s\S]*?glow\.style\.setProperty\('--mx', [\s\S]*?glow\.style\.setProperty\('--my', /);
});

test('M20-频谱开关 chevron 140ms enter：收起时旋转 180°，减少动态效果下即时；收起 / 展开本身不过渡高度', () => {
  assert.match(html, /<button class="btn icon ghost" type="button" id="spToggle" aria-expanded="true" aria-controls="spTrack" aria-label="收起复制频谱"><span data-icon="chevron"><\/span><\/button>/);
  assert.match(rule('#spToggle .ic'), /transition: transform 140ms var\(--ease-enter\)/);
  assert.match(rule('.spectrum[data-collapsed="true"] #spToggle .ic'), /transform: rotate\(180deg\)/);
  assert.match(rule('.spectrum[data-collapsed="true"] .sp-track, .spectrum[data-collapsed="true"] .sp-axis'), /display: none/);
  // 减少动态效果：由全局规则把 transition-duration 归零（原则 3），这里确认没有用 !important 绕开
  assert.doesNotMatch(rule('#spToggle .ic'), /!important/);
});

test('原则 2 键盘触发走 instant：方向键按住、分类、置顶、菜单、主题', () => {
  assert.match(keymap, /instant: e\.repeat/);
  assert.match(ts, /case 'filter': return setFilter\(cycleFilter\(S\.filter, a\.d\), true\);/);
  assert.match(ts, /case 'pin': return void \(sel !== null && togglePin\(sel, false\)\);/);
  assert.match(ts, /if \(r && sel !== null\) openMenu\(sel, r\.left \+ 48, r\.bottom, true\);/);
  assert.match(ts, /pinBtn\.addEventListener\('click', \(e\) => S\.sel !== null && togglePin\(S\.sel, e\.detail !== 0\)\)/);
});

test('原则 3 减少动态效果：CSS 全局规则 + fx() 与直接 WAAPI 调用都检查 matchMedia（倒计时圈除外：计时信息，不是过渡）', () => {
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*\*, \*::before, \*::after, ::backdrop \{ transition-duration: 0s !important; transition-delay: 0s !important; animation: none !important; \}/);
  assert.match(ts, /const reduced = matchMedia\('\(prefers-reduced-motion: reduce\)'\);/);
  assert.match(ts, /const fx = \(el: Element \| null, kf: Keyframe\[\], opts: KeyframeAnimationOptions\) => \{\s*if \(el && !reduced\.matches\) el\.animate\(kf, opts\);/);
  // 绕过 fx 的 .animate() 只有这几处：fx 自身、倒计时圈、置顶 FLIP（r0 已排除 reduced）
  const direct = [...ts.matchAll(/(\w+)\.animate\(/g)].map((m) => m[1]);
  assert.deepEqual(direct, ['el', 'ring', 'el']);
  // 计时器驱动的收尾在 reduced 下也即时
  for (const ms of [150, 360]) assert.match(ts, new RegExp(`reduced\\.matches \\? 0 : ${ms}\\)`));
  // 隐藏的 160ms：reduced 时 animate 为 false，计时为 0，后端也不做原生淡出
  assert.match(ts, /const animate = !reduced\.matches;[\s\S]*?animate && !nativeFade\(\) \? 160 : 0\);/);
});

test('原则 4 只动 transform / opacity / filter 与颜色类属性（白名单），不过渡布局尺寸（行高 grid-template-rows 只限 .item / .item.is-removing）', () => {
  const OK = new Set(['none', 'opacity', 'transform', 'filter', 'clip-path', 'color', 'background-color', 'border-color', 'box-shadow', 'visibility', 'stroke-dashoffset']);
  const ROW_H = new Set(['.item', '.item.is-removing']);
  // 逗号分段时跳过括号里的逗号（cubic-bezier(.22, 1, …)），每段第一个词是属性
  const props = (t: string) => t.split(/,(?![^(]*\))/).map((x) => x.trim().split(/\s+/)[0]);
  const bad: string[] = [];
  let n = 0;
  for (const [, raw, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const sel = raw.replace(/\/\*[\s\S]*?\*\//g, '');
    for (const [, t] of body.matchAll(/(?:^|[;\s])transition(?:-property)?:\s*([^;}]+)/g)) {
      n++;
      for (const p of props(t)) if (!OK.has(p) && !(p === 'grid-template-rows' && ROW_H.has(sel.trim()))) bad.push(`${sel.trim()} → ${p}`);
    }
  }
  assert.ok(n > 40, `只解析到 ${n} 条 transition`);
  assert.deepEqual(bad, []);
  // 关键帧：flash 的 background 只是颜色（color-mix），不改尺寸
  const KF_OK = new Set(['opacity', 'transform', 'filter', 'box-shadow', 'background']);
  const kfs = [...css.matchAll(/@keyframes (\w+) \{([\s\S]*?\})\s*\}/g)];
  assert.equal(kfs.length, css.match(/@keyframes /g)!.length);
  assert.deepEqual(kfs.flatMap((m) => [...m[2].matchAll(/([\w-]+)\s*:/g)].map((d) => d[1]).filter((p) => !KF_OK.has(p)).map((p) => `${m[1]} → ${p}`)), []);
  // main.ts 里内联的过渡与 WAAPI 关键帧
  const inline = [...ts.matchAll(/\.style\.transition = `([^`]*)`/g)].map((m) => m[1]);
  assert.ok(inline.length);
  assert.deepEqual(inline.flatMap(props).filter((p) => !OK.has(p)), []);
  const waapiKeys = [...ts.matchAll(/(?:fx\(\w+(?:\.querySelector\([^)]*\))?, |\.animate\()\[([^\]]*)\]/g)].flatMap((m) => [...m[1].matchAll(/(\w+):/g)].map((k) => k[1]));
  assert.ok(waapiKeys.length > 10);
  assert.deepEqual([...new Set(waapiKeys)].filter((k) => !['transform', 'opacity', 'filter', 'strokeDashoffset'].includes(k)), []);
});

test('阶段 10 图中文字：区块、角标、片段、识别设置不加过渡或动画；details 开合无动效', () => {
  // 去掉注释再按规则读：选择器含 .ocr- / .snip 的块里不能有 transition / animation 声明
  const rules = [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(([, sel]) => /\.ocr-[a-z-]*|\.snip\b/.test(sel));
  assert.ok(rules.length >= 8, `只找到 ${rules.length} 条图中文字规则`);
  assert.deepEqual(rules.filter(([, , body]) => /(?:^|[;\s])(transition|animation)(?:-[a-z-]+)?\s*:/.test(body)).map(([, sel]) => sel.trim()), []);
  // details 开合：没有 ::details-content / summary 标记的过渡或动画规则
  assert.doesNotMatch(css, /(?:::details-content|summary::[a-z-]+)\s*\{[^}]*(?:transition|animation)/);
});
