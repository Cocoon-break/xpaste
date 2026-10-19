import { attachConsole, info } from './api/log';
import { bootstrap, hidePanel, openAccessibilitySettings, pasteText } from './api/commands';
import { onClipboardChanged, onPanelShown } from './api/events';
import { copyStatus, needsAccessibility } from './copy-status';

const q = document.querySelector<HTMLInputElement>('#q')!;
const status = document.querySelector<HTMLElement>('#status')!;
const axSettings = document.querySelector<HTMLButtonElement>('#axSettings')!;
// 阶段 1 临时：测量用，阶段 3 移除
let hiddenEvents = 0;

await attachConsole();
await onPanelShown(({ t0 }) => {
  void info(`隐藏期间收到 ${hiddenEvents} 个事件；reduced-motion=${matchMedia('(prefers-reduced-motion: reduce)').matches}`);
  hiddenEvents = 0;
  q.focus();
  requestAnimationFrame(() => {
    if (t0 !== null) void info(`唤起耗时 ${Date.now() - t0} ms`);
  });
});
await onClipboardChanged((p) => {
  hiddenEvents++;
  void info(`剪切板变化 ${p.bytes} B sensitive=${p.sensitive}`);
});
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !e.isComposing) void hidePanel();
});

const b = await bootstrap();
document.documentElement.dataset.material = b.platform.material;
status.textContent = `xPaste · ${b.platform.os} · 最多保存 ${b.settings.maxRecords} 条`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);

// 阶段 1 临时：测量用，阶段 3 移除
document.querySelector('#pasteTest')!.addEventListener('click', async () => {
  try {
    const r = await pasteText('xPaste 粘贴测试 ' + Date.now());
    axSettings.hidden = !needsAccessibility(r);
    status.textContent = copyStatus(r, b.platform.os);
  } catch (err) {
    status.textContent = (err as { message?: string }).message ?? String(err);
  }
});
axSettings.addEventListener('click', () => void openAccessibilitySettings());
