// 悬停提示：代替系统的 title 小黄条。鼠标停在带 title 的元素上 500 ms 后，在它下方（放不下就上方）显示应用样式的提示。
// 第一次悬停时把 title 挪到 data-tip，系统就不再弹自己的；元素没有别的可读名字时顺手补上 aria-label，读屏照旧。
// 代码之后又给它设了 title 也没关系，下次悬停再挪一次。

const DELAY = 500;

/** 提示框放在哪：优先放在元素正下方居中，下面放不下放上方；左右不出窗口 */
export function tipPos(el: { left: number; top: number; bottom: number; width: number }, tip: { w: number; h: number }, view: { w: number; h: number }, gap = 6): { x: number; y: number } {
  const x = Math.round(Math.min(Math.max(4, el.left + el.width / 2 - tip.w / 2), view.w - tip.w - 4));
  const below = el.bottom + gap;
  const y = below + tip.h <= view.h - 4 ? below : Math.max(4, el.top - gap - tip.h);
  return { x, y: Math.round(y) };
}

export function installTips(doc: Document = document) {
  const tip = doc.createElement('div');
  tip.className = 'ui-tip';
  tip.setAttribute('aria-hidden', 'true');
  // 挂在 <html> 下：有的页面（划词卡片）每次重画都清空 body
  doc.documentElement.append(tip);
  let timer = 0, cur: Element | null = null;

  function hide() {
    clearTimeout(timer);
    cur = null;
    tip.removeAttribute('data-show');
  }
  function show(el: HTMLElement) {
    const text = el.dataset.tip;
    if (!text || !el.isConnected) return;
    // 模态对话框在最顶层，挂在页面上的提示会被它盖住：元素在打开着的对话框里时，提示挂进对话框、按对话框的坐标摆
    const dlg = el.closest<HTMLDialogElement>('dialog[open]');
    const host = dlg ?? doc.documentElement;
    if (tip.parentNode !== host) host.append(tip);
    tip.style.position = dlg ? 'absolute' : '';
    tip.textContent = text;
    tip.style.left = '0px';
    tip.style.top = '0px';
    const r = el.getBoundingClientRect(), o = dlg ? dlg.getBoundingClientRect() : { left: 0, top: 0 };
    const view = dlg ? { w: dlg.clientWidth, h: dlg.clientHeight } : { w: innerWidth, h: innerHeight };
    const p = tipPos({ left: r.left - o.left, top: r.top - o.top, bottom: r.bottom - o.top, width: r.width }, { w: tip.offsetWidth, h: tip.offsetHeight }, view);
    tip.style.left = `${p.x}px`;
    tip.style.top = `${p.y}px`;
    tip.setAttribute('data-show', '');
  }
  doc.addEventListener('pointerover', (e) => {
    const el = (e.target as Element).closest<HTMLElement>('[title], [data-tip]');
    if (el === cur) return;
    hide();
    if (!el) return;
    const t = el.getAttribute('title');
    if (t !== null) {
      // 有 aria-label 或者自己就有字的元素不用补名字
      if (t && !el.hasAttribute('aria-label') && !el.hasAttribute('aria-labelledby') && !el.textContent?.trim()) el.setAttribute('aria-label', t);
      el.dataset.tip = t;
      el.removeAttribute('title');
    }
    if (!el.dataset.tip) return;
    cur = el;
    timer = window.setTimeout(() => show(el), DELAY);
  });
  doc.addEventListener('pointerout', (e) => {
    if (cur && !cur.contains(e.relatedTarget as Node | null)) hide();
  });
  for (const ev of ['pointerdown', 'keydown', 'wheel', 'blur'] as const) addEventListener(ev, hide, true);
}
