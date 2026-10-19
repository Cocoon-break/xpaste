import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { RecordSummary } from './types';

export const PANEL_SHOWN = 'panel-shown';

export const onPanelShown = (cb: (p: { t0: number | null }) => void): Promise<UnlistenFn> =>
  listen<{ t0: number | null }>(PANEL_SHOWN, (e) => cb(e.payload));

export const PANEL_HIDDEN = 'panel-hidden';

export const onPanelHidden = (cb: () => void): Promise<UnlistenFn> => listen(PANEL_HIDDEN, () => cb());

export const RECORD_ADDED = 'record-added';
export const RECORD_UPDATED = 'record-updated';
export const RECORDS_REMOVED = 'records-removed';
export const PAUSED_CHANGED = 'paused-changed';

export const onRecordAdded = (cb: (s: RecordSummary) => void): Promise<UnlistenFn> =>
  listen<RecordSummary>(RECORD_ADDED, (e) => cb(e.payload));

export const onRecordUpdated = (cb: (s: RecordSummary) => void): Promise<UnlistenFn> =>
  listen<RecordSummary>(RECORD_UPDATED, (e) => cb(e.payload));

export const onRecordsRemoved = (cb: (ids: number[]) => void): Promise<UnlistenFn> =>
  listen<number[]>(RECORDS_REMOVED, (e) => cb(e.payload));

export const onPausedChanged = (cb: (paused: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>(PAUSED_CHANGED, (e) => cb(e.payload));
