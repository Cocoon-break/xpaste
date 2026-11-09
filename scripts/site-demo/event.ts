// 替换 @tauri-apps/api/event：事件只在页面内分发。
type Cb = (e: { payload: unknown }) => void;
const subs = new Map<string, Set<Cb>>();
export type UnlistenFn = () => void;

export async function listen<T>(name: string, cb: (e: { payload: T }) => void, _opts?: unknown): Promise<UnlistenFn> {
  if (!subs.has(name)) subs.set(name, new Set());
  const set = subs.get(name)!;
  set.add(cb as Cb);
  return () => set.delete(cb as Cb);
}
export async function emit(name: string, payload?: unknown): Promise<void> {
  subs.get(name)?.forEach((cb) => cb({ payload }));
}
