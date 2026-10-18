import { attachConsole, info } from './api/log';
import { bootstrap } from './api/commands';

await attachConsole();
const b = await bootstrap();
document.querySelector<HTMLElement>('#app')!.textContent = `xPaste · ${b.platform.os} · 最多保存 ${b.settings.maxRecords} 条`;
await info(`bootstrap 完成，平台 ${b.platform.os}`);
