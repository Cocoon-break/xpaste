// 搜索节流：首个输入立即查；查询进行中、或距上次发起不足 gap 时只记下最新输入，到点或返回后补查一次。
// 连续输入最多每 gap 发一次查询，且最后一定按最新输入查过（大文本库里一次查询可能要几百毫秒，不能每键都发）。
export const SEARCH_GAP_MS = 80;

type Timers = {
  set: (fn: () => void, ms: number) => unknown;
  clear: (h: unknown) => void;
};
const realTimers: Timers = { set: (fn, ms) => setTimeout(fn, ms), clear: (h) => clearTimeout(h as number) };

export function searchGate(
  run: (v: string) => Promise<unknown>,
  gap = SEARCH_GAP_MS,
  now: () => number = () => performance.now(),
  timers: Timers = realTimers,
) {
  let timer: unknown;
  let lastAt = -Infinity;
  let busy = false;
  let pending: string | null = null;
  const stop = () => {
    if (timer !== undefined) timers.clear(timer);
    timer = undefined;
  };
  const fire = (v: string) => {
    lastAt = now();
    busy = true;
    run(v).finally(() => {
      busy = false;
      if (pending !== null) flush();
    });
  };
  // 有待查输入且没有查询在跑就立刻发；在跑则等它返回时补发
  const flush = () => {
    stop();
    if (pending === null || busy) return;
    const v = pending;
    pending = null;
    fire(v);
  };
  return {
    push(v: string) {
      pending = v;
      stop();
      if (busy) return;
      const wait = lastAt + gap - now();
      if (wait <= 0) flush();
      else timer = timers.set(flush, wait);
    },
    flush,
    // 程序路径（清空、唤起）自己查了，丢掉用户输入的待查项
    cancel() {
      stop();
      pending = null;
    },
  };
}
