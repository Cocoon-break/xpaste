// 官网上的划词翻译卡片：真实的 card.html，由父页面发来原文与译文，按真实接口的节奏分段出字。
import { emit } from './event';

const style = document.createElement('style');
style.textContent = 'html, body { background: transparent !important; }';
document.head.append(style);

const sleep = (ms: number) => new Promise((ok) => setTimeout(ok, ms));
let seq = 0;

async function play(source: string, text: string) {
  const mine = ++seq;
  void emit('trcard-show', { kind: 'text', seq: mine, source, engine: 'system', noAccess: false });
  await sleep(500);
  for (let i = 0; i < text.length; i += 3) {
    if (mine !== seq) return;
    void emit('translate-delta', { id: null, lang: 'zh-Hans', engine: 'system', text: text.slice(i, i + 3) });
    await sleep(55);
  }
  if (mine === seq) void emit('trcard-done', { seq: mine, ok: true, text, lang: 'zh-Hans', engine: 'system', error: null });
}

addEventListener('message', (e: MessageEvent) => {
  const m = e.data as { xp?: boolean; type?: string; value?: string; source?: string; text?: string } | null;
  if (!m || m.xp !== true || e.source !== parent) return;
  if (m.type === 'theme') {
    const root = document.documentElement;
    if (m.value === 'light' || m.value === 'dark') root.dataset.theme = m.value;
    else delete root.dataset.theme;
  } else if (m.type === 'card') void play(String(m.source ?? ''), String(m.text ?? ''));
});
addEventListener('load', () => parent !== window && parent.postMessage({ xp: true, type: 'ready' }, '*'));
