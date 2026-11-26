// 和应用一致的下拉选择：原生 <select> 留着当数据源（取值、change 事件、保存设置都照旧），只是藏起来；
// 外面换成一个按钮，点开是和右键菜单同一套样子的弹出列表（样式在 ui.css）。弹层挂在 host（设置窗）里，下面放不下就往上弹。

/** 列表里按键后焦点落在第几项；不是移动键为 null */
export function moveIndex(i: number, key: string, n: number): number | null {
  if (n <= 0) return null;
  switch (key) {
    case 'ArrowDown': return Math.min(i + 1, n - 1);
    case 'ArrowUp': return Math.max(i - 1, 0);
    case 'Home': return 0;
    case 'End': return n - 1;
    default: return null;
  }
}

const CHEVRON = '<svg class="chev" viewBox="0 0 20 20" aria-hidden="true"><path d="m6 8 4 4 4-4"/></svg>';
const CHECK = '<svg class="ck" viewBox="0 0 20 20" aria-hidden="true"><path d="m5 10.5 3.2 3L15 7"/></svg>';

/** 换掉 sel 的外观。代码里直接给 sel.value 赋值、改选项时按钮上的字自己跟着变。
 *  room：窗口贴着内容大小的页面（划词卡片）用，弹层要伸出 host 底部时告诉它还差多少像素，收起时给 0。
 *  destroy：页面要整块重画、扔掉这个 select 时调，摘掉挂在 document 上的监听和弹层，展开着的话先按收起处理 */
export function popSelect(sel: HTMLSelectElement, host: HTMLElement, room?: (px: number) => void): { sync(): void; destroy(): void } {
  if (sel.dataset.pop) return { sync() {}, destroy() {} };
  sel.dataset.pop = '1';
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'sel-btn';
  btn.id = `${sel.id}Btn`;
  btn.setAttribute('aria-haspopup', 'listbox');
  btn.setAttribute('aria-expanded', 'false');
  const desc = sel.getAttribute('aria-describedby');
  if (desc) btn.setAttribute('aria-describedby', desc);
  // 原来指向 select 的 <label for> 改指向按钮，点标签、读屏都落到按钮上
  const label = document.querySelector<HTMLLabelElement>(`label[for="${sel.id}"]`);
  if (label) {
    label.htmlFor = btn.id;
    label.id ||= `${sel.id}Label`;
  }
  if (sel.dataset.act) btn.dataset.act = sel.dataset.act;
  sel.hidden = true;
  sel.tabIndex = -1;
  sel.after(btn);

  const menu = document.createElement('div');
  menu.className = 'sel-menu';
  menu.setAttribute('role', 'listbox');
  if (label) menu.setAttribute('aria-labelledby', label.id);
  menu.dataset.open = 'false';
  host.append(menu);

  const items = () => [...menu.querySelectorAll<HTMLButtonElement>('button')];
  function sync() {
    btn.innerHTML = `<span></span>${CHEVRON}`;
    btn.firstElementChild!.textContent = sel.selectedOptions[0]?.textContent ?? '';
    btn.disabled = sel.disabled;
  }
  function close(refocus: boolean) {
    if (menu.dataset.open !== 'true') return;
    menu.dataset.open = 'false';
    room?.(0);
    btn.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', outside, true);
    if (refocus) btn.focus();
  }
  function outside(e: PointerEvent) {
    const t = e.target as Node;
    if (!menu.contains(t) && !btn.contains(t)) close(false);
  }
  function choose(v: string) {
    if (sel.value !== v) {
      sel.value = v;
      sel.dispatchEvent(new Event('change', { bubbles: true }));
    }
    sync();
    close(true);
  }
  function open() {
    menu.replaceChildren(...[...sel.options].map((o, i) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.setAttribute('role', 'option');
      b.setAttribute('aria-selected', String(o.selected));
      b.style.setProperty('--i', String(i));
      b.innerHTML = `<span></span>${CHECK}`;
      b.firstElementChild!.textContent = o.textContent;
      b.addEventListener('click', () => choose(o.value));
      return b;
    }));
    // 先量尺寸再摆：和按钮右对齐，宽度不小于按钮；下面放不下就放到按钮上方
    menu.style.minWidth = `${btn.offsetWidth}px`;
    menu.dataset.open = 'true';
    const r = btn.getBoundingClientRect(), h = host.getBoundingClientRect();
    const mh = menu.offsetHeight, mw = menu.offsetWidth, gap = 4;
    const below = r.bottom - h.top + gap, above = r.top - h.top - mh - gap;
    const up = !room && below + mh > host.clientHeight - 8 && above >= 8;
    if (room) room(Math.max(0, below + mh + 8 - host.clientHeight));
    menu.style.top = `${up ? above : below}px`;
    menu.style.left = `${Math.max(8, r.right - h.left - mw)}px`;
    menu.style.transformOrigin = up ? 'bottom right' : 'top right';
    btn.setAttribute('aria-expanded', 'true');
    document.addEventListener('pointerdown', outside, true);
    (items()[sel.selectedIndex] ?? items()[0])?.focus({ preventScroll: true });
  }

  btn.addEventListener('click', () => (menu.dataset.open === 'true' ? close(true) : open()));
  btn.addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    open();
  });
  menu.addEventListener('keydown', (e) => {
    const list = items(), i = list.indexOf(document.activeElement as HTMLButtonElement);
    const next = moveIndex(i, e.key, list.length);
    if (next !== null) {
      e.preventDefault();
      list[next].focus();
    } else if (e.key === 'Escape') {
      // 只关列表，不让设置窗收到 Esc
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'Tab') close(false);
  });
  // 设置窗（<dialog>）关掉时列表跟着收起；别的 host 不发 close，不挂
  const onClose = () => close(false);
  if (host instanceof HTMLDialogElement) host.addEventListener('close', onClose);
  sel.addEventListener('change', sync);
  // 代码里给 value 赋值不发 change：在这个元素上包一层 setter，赋完值顺手刷新按钮；选项被换掉时也刷新
  const value = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')!;
  Object.defineProperty(sel, 'value', {
    configurable: true,
    get() { return value.get!.call(sel); },
    set(v: string) { value.set!.call(sel, v); sync(); },
  });
  const mo = new MutationObserver(sync);
  mo.observe(sel, { childList: true, subtree: true, attributes: true, attributeFilter: ['selected', 'disabled'] });
  sync();
  return {
    sync,
    destroy() {
      close(false);
      mo.disconnect();
      host.removeEventListener('close', onClose);
      menu.remove();
    },
  };
}
