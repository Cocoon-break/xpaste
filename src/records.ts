// 前端记录表：按后端事件增量更新；排序见 view.ts 的 visible。纯函数，便于单元测试。
import type { RecordSummary } from './api/types';

export type Records = Map<number, RecordSummary>;

export const applyAdded = (map: Records, s: RecordSummary): void => {
  map.set(s.id, s);
};

/** 不存在的 id 也插入（例如撤销删除、补全晚于删除事件）。 */
export const applyUpdated = applyAdded;

export const applyRemoved = (map: Records, ids: number[]): void => {
  for (const id of ids) map.delete(id);
};
