// 设置 → Pro → 搅拌机里的配方列表：拖把手调顺序、显示开关、行内编辑、自带的恢复默认、自己加的删除，删除和恢复可在 4 s 内撤销。
// 列表是设置对话框的草稿：get() 读、set() 写，点“保存设置”才生效。
import type { Recipe } from './api/types.ts';

export const RECIPES_MAX = 30;
const UNDO_MS = 4000;

/** 打开的配方里排前三的是底座按键（key），其余在 ⋯ 里（more）；关着的不出现在搅拌机上（null） */
export const placeOf = (list: Recipe[], i: number): 'key' | 'more' | null => {
  if (!list[i]?.show) return null;
  return list.slice(0, i).filter((r) => r.show).length < 3 ? 'key' : 'more';
};

export const moveRecipe = (list: Recipe[], from: number, to: number): Recipe[] => {
  const l = [...list];
  l.splice(to, 0, ...l.splice(from, 1));
  return l;
};

/** 拖着的那一行（原来在 from）中心到了 center：越过了几个别的行的中点，就落在第几位 */
export const dropIndex = (mids: number[], from: number, center: number): number =>
  mids.filter((m, k) => k !== from && m < center).length;

/** 点“完成”时的校验；i 是正在改的那一行（新配方还没进列表时给 -1），和它自己同名不算重名 */
export const recipeError = (list: Recipe[], i: number, name: string, prompt: string): string | null => {
  const n = name.trim();
  if (!n) return '请给配方起个名字';
  if (list.some((r, k) => k !== i && r.name === n)) return '已经有同名的配方了';
  if (!prompt.trim()) return '提示词不能为空';
  return null;
};

/** 自带的配方名字或提示词和默认不同（才有“恢复默认”）；自己加的没有默认 */
export const isChanged = (r: Recipe, defaults: Recipe[]): boolean => {
  const d = r.builtin ? defaults.find((x) => x.builtin === r.builtin) : undefined;
  return !!d && (d.name !== r.name || d.prompt !== r.prompt);
};

const ic = (body: string, fill = false) =>
  `<svg class="ic" viewBox="0 0 20 20" ${fill ? 'fill="currentColor"' : 'fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"'} aria-hidden="true">${body}</svg>`;
const IC_GRIP = ic('<circle cx="7.5" cy="5.5" r="1.3"/><circle cx="12.5" cy="5.5" r="1.3"/><circle cx="7.5" cy="10" r="1.3"/><circle cx="12.5" cy="10" r="1.3"/><circle cx="7.5" cy="14.5" r="1.3"/><circle cx="12.5" cy="14.5" r="1.3"/>', true);
const IC_EDIT = ic('<path d="M4 16l1-4 8-8 3 3-8 8z"/><path d="M11.5 5.5l3 3"/>');
const IC_DEL = ic('<path d="M4.5 6h11M8 6V4.5h4V6M6 6l.7 9.5h6.6L14 6"/>');
const IC_UNDO = ic('<path d="M7.5 6 4 9.5 7.5 13"/><path d="M4.5 9.5H12a4 4 0 0 1 0 8h-1.5"/>');

// composing：这次按键归输入法（ime.ts 的同名函数；那个模块一加载就挂全局监听，单测里不能直接引）
type Opts = { get(): Recipe[]; set(l: Recipe[]): void; defaults: Recipe[]; composing?: (e: KeyboardEvent) => boolean };

/** 在 host 里画配方列表、“＋ 新配方”和撤销行。defaults 可以先给空数组，取到后往里填再 render() */
export function mountRecipes(host: HTMLElement, o: Opts): { render(): void; edit(name: string): void; reset(): void } {
  host.innerHTML = '<div class="rl" role="list" aria-label="配方"></div><div class="rl-foot"><button class="btn sm" type="button" data-a="add">＋ 新配方</button><span class="rl-log" role="status" aria-live="polite"></span></div>';
  const rl = host.querySelector<HTMLElement>('.rl')!, addBtn = host.querySelector<HTMLButtonElement>('[data-a="add"]')!, logEl = host.querySelector<HTMLElement>('.rl-log')!;
  // 展开编辑的那一行：列表里的某一个（按对象认，换了顺序也跟着），或者还没进列表的新配方
  let editing: Recipe | null = null, fresh: Recipe | null = null, logTimer = 0;

  const rowsOf = () => [...rl.querySelectorAll<HTMLElement>('.rr')];
  const commit = (l: Recipe[]) => {
    o.set(l);
    render();
  };
  function log(msg: string, undo?: () => void) {
    clearTimeout(logTimer);
    logEl.replaceChildren(msg);
    if (undo) {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = '撤销';
      b.addEventListener('click', () => {
        logEl.replaceChildren();
        undo();
      });
      logEl.append(b);
    }
    logTimer = window.setTimeout(() => logEl.replaceChildren(), UNDO_MS);
  }

  function rowEl(r: Recipe, i: number): HTMLElement {
    const row = document.createElement('div');
    row.className = `rr${r.show ? '' : ' off'}`;
    row.setAttribute('role', 'listitem');
    const at = i < 0 ? null : placeOf(o.get(), i);
    const side = r.builtin ? (isChanged(r, o.defaults) ? `<button type="button" class="ib" data-a="restore" aria-label="恢复默认" data-tip="恢复默认">${IC_UNDO}</button>` : '<span class="ib ph"></span>')
      : `<button type="button" class="ib" data-a="del" aria-label="删除" data-tip="删除">${IC_DEL}</button>`;
    row.innerHTML = `<button type="button" class="grip" data-a="grip" aria-label="拖动调整顺序，也可以按上下方向键"${i < 0 ? ' disabled' : ''}>${IC_GRIP}</button>
      <div class="nm"><b></b>${at ? `<span class="tag${at === 'key' ? ' key' : ''}">${at === 'key' ? '底座按键' : '⋯ 里'}</span>` : ''}${r.builtin ? '' : '<span class="tag">自己加的</span>'}<small></small></div>
      <label class="switch"><input type="checkbox" data-a="show" aria-label="显示在搅拌机上"${r.show ? ' checked' : ''}${i < 0 ? ' disabled' : ''}></label>
      <button type="button" class="ib" data-a="edit" aria-label="编辑" data-tip="编辑" aria-expanded="${r === editing}">${IC_EDIT}</button>${side}`;
    row.querySelector('b')!.textContent = r.name || '新配方';
    row.querySelector('small')!.textContent = r.prompt || '还没写提示词';
    if (r === editing) row.append(editorEl(r));
    return row;
  }
  function editorEl(r: Recipe): HTMLElement {
    const ed = document.createElement('div');
    ed.className = 'ed';
    ed.innerHTML = `<input maxlength="8" autocomplete="off" spellcheck="false" placeholder="名字，最多 8 个字（底座按键上显示）" aria-label="配方名字">
      <textarea maxlength="2000" rows="3" placeholder="提示词：告诉 AI 怎么处理杯子里的原料" aria-label="提示词"></textarea>
      <div class="acts"><span class="err" role="alert"></span><span class="sp"></span><button class="btn sm" type="button" data-a="cancel">取消</button><button class="btn sm primary" type="button" data-a="ok">完成</button></div>`;
    ed.querySelector('input')!.value = r.name;
    ed.querySelector('textarea')!.value = r.prompt;
    return ed;
  }

  // 重画整张列表；开着的编辑框里还没点“完成”的字、焦点所在的控件都照原样放回
  function render(focus?: { i: number; a: string }) {
    const ni = rl.querySelector<HTMLInputElement>('.ed input'), pi = rl.querySelector<HTMLTextAreaElement>('.ed textarea');
    const kept = ni && pi ? { name: ni.value, prompt: pi.value, err: rl.querySelector('.ed .err')!.textContent } : null;
    const act = document.activeElement as HTMLElement | null;
    if (!focus && act && rl.contains(act)) {
      const row = act.closest<HTMLElement>('.rr');
      focus = { i: rowsOf().indexOf(row!), a: act.dataset.a ?? (act.tagName === 'TEXTAREA' ? 'prompt' : act.tagName === 'INPUT' ? 'name' : '') };
    }
    const l = o.get();
    if (editing && !l.includes(editing) && editing !== fresh) editing = null;
    rl.replaceChildren(...l.map((r, i) => rowEl(r, i)), ...(fresh ? [rowEl(fresh, -1)] : []));
    if (kept && editing) {
      rl.querySelector<HTMLInputElement>('.ed input')!.value = kept.name;
      rl.querySelector<HTMLTextAreaElement>('.ed textarea')!.value = kept.prompt;
      rl.querySelector('.ed .err')!.textContent = kept.err;
    }
    addBtn.disabled = l.length + (fresh ? 1 : 0) >= RECIPES_MAX;
    if (focus) {
      const row = rowsOf()[focus.i];
      const sel = focus.a === 'name' ? '.ed input' : focus.a === 'prompt' ? '.ed textarea' : `[data-a="${focus.a}"]`;
      (row?.querySelector<HTMLElement>(sel) ?? row?.querySelector<HTMLElement>('[data-a="edit"]'))?.focus({ preventScroll: true });
    }
  }

  // 同一时间只展开一行；换到别的行时新配方那一行去掉
  function open(r: Recipe | null) {
    if (fresh && r !== fresh) fresh = null;
    editing = r;
    render();
    if (!r) return;
    const row = rowsOf().find((x) => x.querySelector('.ed'))!;
    row.scrollIntoView({ block: 'nearest' });
    row.querySelector<HTMLElement>(r.name ? '.ed textarea' : '.ed input')!.focus();
  }
  function finish(row: HTMLElement) {
    const l = o.get(), i = editing === fresh ? -1 : l.indexOf(editing!);
    const ni = row.querySelector<HTMLInputElement>('.ed input')!, pi = row.querySelector<HTMLTextAreaElement>('.ed textarea')!;
    const err = recipeError(l, i, ni.value, pi.value);
    if (err) {
      row.querySelector('.ed .err')!.textContent = err;
      (err === '提示词不能为空' ? pi : ni).focus();
      return;
    }
    const r = { ...editing!, name: ni.value.trim(), prompt: pi.value.trim() };
    const at = i < 0 ? l.length : i;
    editing = fresh = null;
    commit(i < 0 ? [...l, r] : l.map((x, k) => (k === i ? r : x)));
    rowsOf()[at]?.querySelector<HTMLElement>('[data-a="edit"]')?.focus();
  }
  function cancel() {
    const at = editing === fresh ? -1 : o.get().indexOf(editing!);
    editing = fresh = null;
    render();
    (at < 0 ? addBtn : rowsOf()[at]?.querySelector<HTMLElement>('[data-a="edit"]'))?.focus();
  }

  host.addEventListener('click', (e) => {
    const t = (e.target as Element).closest<HTMLElement>('[data-a]');
    if (!t || t.dataset.a === 'show' || t.dataset.a === 'grip') return;
    if (t.dataset.a === 'add') {
      fresh = { name: '', prompt: '', show: true, builtin: null };
      return open(fresh);
    }
    const row = t.closest<HTMLElement>('.rr')!, i = rowsOf().indexOf(row), l = o.get(), r = l[i] ?? fresh!;
    switch (t.dataset.a) {
      case 'edit': return open(editing === r ? null : r);
      case 'ok': return finish(row);
      case 'cancel': return cancel();
      case 'del': {
        if (editing === r) editing = null;
        commit(l.filter((_, k) => k !== i));
        // 撤销时放回原来的位置；这期间列表变短了就放到末尾
        return log(`已删除“${r.name}”`, () => {
          const now = o.get();
          if (now.length < RECIPES_MAX && !now.some((x) => x.name === r.name)) commit([...now.slice(0, i), r, ...now.slice(i)]);
        });
      }
      case 'restore': {
        const d = o.defaults.find((x) => x.builtin === r.builtin)!;
        if (l.some((x, k) => k !== i && x.name === d.name)) return log('已经有同名的配方了');
        if (editing === r) editing = null;
        const back = { ...r, name: d.name, prompt: d.prompt };
        commit(l.map((x) => (x === r ? back : x)));
        rowsOf()[i]?.querySelector<HTMLElement>('[data-a="edit"]')?.focus();
        return log(`“${d.name}”恢复成默认`, () => {
          const now = o.get(), k = now.indexOf(back);
          if (k >= 0 && !now.some((x, j) => j !== k && x.name === r.name)) commit(now.map((x) => (x === back ? { ...back, name: r.name, prompt: r.prompt } : x)));
        });
      }
    }
  });
  host.addEventListener('change', (e) => {
    const t = e.target as HTMLInputElement;
    if (t.dataset.a !== 'show') return;
    const i = rowsOf().indexOf(t.closest<HTMLElement>('.rr')!), l = o.get(), r = { ...l[i], show: t.checked };
    if (editing === l[i]) editing = r;
    commit(l.map((x, k) => (k === i ? r : x)));
  });
  // 编辑框里 Enter（名字框）是“完成”、Esc 是“取消”，都不交给设置对话框（不提交设置、不回概览）；
  // 把手上按上下方向键挪一格
  host.addEventListener('keydown', (e) => {
    const t = e.target as HTMLElement;
    if (o.composing?.(e) ?? e.isComposing) return;
    if (t.closest('.ed')) {
      if (e.key === 'Escape' || (e.key === 'Enter' && t.tagName === 'INPUT')) {
        e.preventDefault();
        e.stopPropagation();
        if (e.key === 'Escape') cancel();
        else finish(t.closest<HTMLElement>('.rr')!);
      }
      return;
    }
    if (t.dataset.a === 'grip' && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      e.preventDefault();
      const l = o.get(), i = rowsOf().indexOf(t.closest<HTMLElement>('.rr')!), to = i + (e.key === 'ArrowUp' ? -1 : 1);
      if (to < 0 || to >= l.length) return;
      o.set(moveRecipe(l, i, to));
      render({ i: to, a: 'grip' });
    }
  });

  // 拖把手换顺序：拖的时候别的行用 transform 让位（挪 DOM 会让把手丢掉指针捕获），松手后再改顺序
  let drag: { y0: number; rows: HTMLElement[]; row: HTMLElement; i: number; t: number; mids: number[]; h: number } | null = null;
  rl.addEventListener('pointerdown', (e) => {
    const g = (e.target as Element).closest<HTMLButtonElement>('[data-a="grip"]');
    if (!g || g.disabled || e.button !== 0) return;
    e.preventDefault();
    const rows = rowsOf().slice(0, o.get().length), row = g.closest<HTMLElement>('.rr')!, i = rows.indexOf(row);
    drag = { y0: e.clientY, rows, row, i, t: i, mids: rows.map((r) => r.offsetTop + r.offsetHeight / 2), h: row.offsetHeight };
    row.classList.add('lift');
    g.setPointerCapture(e.pointerId);
  });
  rl.addEventListener('pointermove', (e) => {
    if (!drag) return;
    const d = drag, dy = e.clientY - d.y0;
    d.t = dropIndex(d.mids, d.i, d.mids[d.i] + dy);
    d.row.style.transform = `translateY(${dy}px)`;
    d.rows.forEach((r, k) => {
      if (k === d.i) return;
      const sh = d.i < d.t && k > d.i && k <= d.t ? -d.h : d.t < d.i && k >= d.t && k < d.i ? d.h : 0;
      r.style.transform = sh ? `translateY(${sh}px)` : '';
    });
  });
  const end = () => {
    if (!drag) return;
    const { i, t } = drag;
    drag = null;
    if (i !== t) o.set(moveRecipe(o.get(), i, t));
    render({ i: t, a: 'grip' });
  };
  rl.addEventListener('pointerup', end);
  rl.addEventListener('pointercancel', end);

  return {
    render: () => {
      // 设置重新打开（草稿换了一份）：编辑中的那一行已不在列表里，收起
      if (editing && editing !== fresh && !o.get().includes(editing)) editing = null;
      render();
    },
    /** 重新打开设置：收起编辑、去掉没完成的新配方和撤销行 */
    reset() {
      editing = fresh = null;
      clearTimeout(logTimer);
      logEl.replaceChildren();
      render();
    },
    edit(name: string) {
      const r = o.get().find((x) => x.name === name);
      if (r) open(r);
    },
  };
}
