import { attachConsole, info } from './api/log';
import { bootstrap, hidePanel, openAccessibilitySettings, pasteText } from './api/commands';
import { onClipboardChanged, onPanelShown } from './api/events';

const q = document.querySelector<HTMLInputElement>('#q')!;
const status = document.querySelector<HTMLElement>('#status')!;
const axSettings = document.querySelector<HTMLButtonElement>('#axSettings')!;

await attachConsole();
await onPanelShown(({ t0 }) => {
  q.focus();
  requestAnimationFrame(() => {
    if (t0 !== null) void info(`唤起耗时 ${Date.now() - t0} ms`);
  });
});
await onClipboardChanged((p) => {
  void info(`剪切板变化 ${p.bytes} B sensitive=${p.sensitive}`);
});
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !e.isComposing) void hidePanel();
});

const b = await bootstrap();
document.documentElement.dataset.material = b.platform.material;
status.textContent = `xPaste · ${b.platform.os} · 最多保存 ${b.settings.maxRecords} 条`;
await info(`bootstrap 完成，平台 ${b.platform.os}，材质 ${b.platform.material}`);

const pasteKey = b.platform.os === 'macos' ? '⌘ + V' : 'Ctrl + V';
document.querySelector('#pasteTest')!.addEventListener('click', async () => {
  try {
    const r = await pasteText('xPaste 粘贴测试 ' + Date.now());
    axSettings.hidden = !(r.result === 'copiedOnly' && r.reason === 'needAccessibility');
    if (r.result === 'copiedOnly') {
      status.textContent = `已复制，在目标应用中按 ${pasteKey} 粘贴`;
    } else {
      status.textContent = r.result === 'pasted' ? '已粘贴' : '已复制';
    }
  } catch (err) {
    status.textContent = (err as { message?: string }).message ?? String(err);
  }
});
axSettings.addEventListener('click', () => void openAccessibilitySettings());
