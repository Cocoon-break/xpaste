import { attachConsole, info } from './api/log';
import { bootstrap, hidePanel, openAccessibilitySettings } from './api/commands';
import {
  onPanelShown,
  onRecordAdded,
  onRecordUpdated,
  onRecordsRemoved,
} from './api/events';
import { applyAdded, applyRemoved, applyUpdated, sorted, type Records } from './records';

const q = document.querySelector<HTMLInputElement>('#q')!;
const status = document.querySelector<HTMLElement>('#status')!;
const axSettings = document.querySelector<HTMLButtonElement>('#axSettings')!;
const list = document.querySelector<HTMLUListElement>('#list')!;

await attachConsole();
await onPanelShown(({ t0 }) => {
  void info(`reduced-motion=${matchMedia('(prefers-reduced-motion: reduce)').matches}`);
  q.focus();
  requestAnimationFrame(() => {
    if (t0 !== null) void info(`唤起耗时 ${Date.now() - t0} ms`);
  });
});
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !e.isComposing) void hidePanel();
});

const b = await bootstrap();
document.documentElement.dataset.material = b.platform.material;
status.textContent = `xPaste · ${b.platform.os} · 最多保存 ${b.settings.maxRecords} 条`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);

// 阶段 2 临时：记录列表，阶段 3 由原型界面替换。记录内容只经 textContent 写入
const records: Records = new Map(b.records.map((r) => [r.id, r]));
const render = () => {
  list.replaceChildren(
    ...sorted(records).map((r) => {
      const li = document.createElement('li');
      if (r.thumb) {
        const img = document.createElement('img');
        img.src = r.thumb;
        img.width = 32;
        img.alt = '';
        li.append(img);
      }
      li.append(`${r.title} · ${r.kind} · ${r.app?.name ?? '未知来源'} · 用过 ${r.uses} 次`);
      return li;
    }),
  );
};
render();
await onRecordAdded((s) => {
  applyAdded(records, s);
  render();
});
await onRecordUpdated((s) => {
  applyUpdated(records, s);
  render();
});
await onRecordsRemoved((ids) => {
  applyRemoved(records, ids);
  render();
});

axSettings.addEventListener('click', () => void openAccessibilitySettings());
