import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export const PANEL_SHOWN = 'panel-shown';

export const onPanelShown = (cb: (p: { t0: number | null }) => void): Promise<UnlistenFn> =>
  listen<{ t0: number | null }>(PANEL_SHOWN, (e) => cb(e.payload));
