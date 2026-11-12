// 全局快捷键：Tauri 快捷键字符串 ↔ 结构 ↔ 显示标签，以及录制时按键事件 → 字符串。纯函数。
// 标签规则与 src-tauri/src/hotkey.rs 的 label 一致。
import type { PlatformInfo } from './api/types';

/** mod = Tauri 的 CommandOrControl（macOS ⌘、Windows Ctrl）；ctrl = 字面的 Control 键（macOS ⌃，Windows 上与 Ctrl 相同） */
export type Hotkey = { mod: boolean; ctrl: boolean; alt: boolean; shift: boolean; key: string };
/** 录制时用到的按键事件字段（ctrl / meta 分开：macOS 上 ⌃ 与 ⌘ 是两个键） */
export type ModEv = { key: string; code: string; ctrl: boolean; meta: boolean; alt: boolean; shift: boolean };

const MOD = ['commandorcontrol', 'cmdorctrl', 'command', 'cmd', 'super'];
const CTRL = ['control', 'ctrl'];

/** 忽略大小写与空格；未知修饰词丢弃，最后一个非修饰 token 当键名。 */
export function parseHotkey(s: string): Hotkey {
  const h: Hotkey = { mod: false, ctrl: false, alt: false, shift: false, key: '' };
  for (const raw of s.split('+')) {
    const t = raw.trim().toLowerCase();
    if (MOD.includes(t)) h.mod = true;
    else if (CTRL.includes(t)) h.ctrl = true;
    else if (t === 'alt') h.alt = true;
    else if (t === 'shift') h.shift = true;
    else if (t) h.key = t.toUpperCase();
  }
  return h;
}

/** 固定顺序 CommandOrControl+Control+Alt+Shift+KEY，缺的修饰键省略。 */
export function hotkeyString(h: Hotkey): string {
  return [h.mod && 'CommandOrControl', h.ctrl && 'Control', h.alt && 'Alt', h.shift && 'Shift', h.key].filter(Boolean).join('+');
}

export function hotkeyLabel(s: string, os: PlatformInfo['os']): string {
  const h = parseHotkey(s), mac = os === 'macos';
  // Windows 上 Control 与 CommandOrControl 是同一个键，只显示一次
  return [h.mod && (mac ? '⌘' : 'Ctrl'), h.ctrl && (mac ? '⌃' : !h.mod && 'Ctrl'), h.alt && (mac ? '⌥' : 'Alt'), h.shift && 'Shift', KEY_LABEL[h.key] ?? h.key]
    .filter(Boolean)
    .join(' + ');
}

const KEY_LABEL: Record<string, string> = { SPACE: 'Space', UP: '↑', DOWN: '↓', LEFT: '←', RIGHT: '→' };
// 符号键存字符本身（全局快捷键插件认得 `、-、= 等），空格与方向键存键名
const CODE_KEY: Record<string, string> = {
  Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'",
  Comma: ',', Period: '.', Slash: '/', Space: 'Space', ArrowUp: 'Up', ArrowDown: 'Down', ArrowLeft: 'Left', ArrowRight: 'Right',
};

/** 用物理键位 code 而不是 key：Option 组合的死键 / 输入法字符（如 ç）不会混进来。 */
export function keyFromEvent(e: { key: string; code: string }): string | null {
  const m = /^(?:Key([A-Z])|Digit([0-9])|(F(?:[1-9]|1[0-9]|20)))$/.exec(e.code);
  return m ? (m[1] ?? m[2] ?? m[3]) : CODE_KEY[e.code] ?? null;
}

/** 必须带 ⌘ / Ctrl、⌃ 或 Alt，且主键是字母、数字、符号、空格、方向键或 F1–F20，否则不算一个可用的全局快捷键。
 *  macOS：⌘ → CommandOrControl，⌃ → Control（两个键，不能混）；Windows：Ctrl → CommandOrControl，Win 键不参与。 */
export function hotkeyFromEvent(e: ModEv, os: PlatformInfo['os']): string | null {
  const key = keyFromEvent(e);
  const mac = os === 'macos';
  const mod = mac ? e.meta : e.ctrl, ctrl = mac && e.ctrl;
  return (mod || ctrl || e.alt) && key ? hotkeyString({ mod, ctrl, alt: e.alt, shift: e.shift, key }) : null;
}

/** hotkeyFromEvent 不接受时的提示：缺修饰键，或带了修饰键但主键不能用（如 Enter、Esc）。 */
export function hotkeyHint(e: ModEv, os: PlatformInfo['os']): string {
  const mac = os === 'macos';
  if (mac ? e.meta || e.ctrl || e.alt : e.ctrl || e.alt) return '这个键不能用作快捷键，请换成字母、数字、符号或功能键';
  return mac ? '至少需要包含 ⌘、⌃ 或 ⌥' : '至少需要包含 Ctrl 或 Alt';
}
