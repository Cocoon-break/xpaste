import { attachConsole, info } from './api/log';
import { bootstrap, hidePanel } from './api/commands';
import { onPanelShown } from './api/events';

const q = document.querySelector<HTMLInputElement>('#q')!;

await attachConsole();
await onPanelShown(({ t0 }) => {
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
document.querySelector<HTMLElement>('#status')!.textContent = `xPaste · ${b.platform.os} · 最多保存 ${b.settings.maxRecords} 条`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);
