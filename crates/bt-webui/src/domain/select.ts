//! 事件组合逻辑：筛选、排序、统计、关联。全部纯函数，独立于 React。
//!
//! 渲染层只消费这些函数的结果；未来接入虚拟列表时，只要在 `filterEvents →
//! sortEvents` 的产物上做窗口化切片即可，不需要改动这里。

import type { Direction, TraceEvent } from './trace';
import { decodeTransactionFlags } from './trace';

export type DirectionFilter = 'all' | Direction;
export type ScalarFilter = 'all' | string;

/// 表格筛选条件。`'all'` 表示该维度未限制；其余标量过滤已并入全文搜索。
export interface TraceFilters {
  query: string;
  direction: DirectionFilter;
  interface: ScalarFilter;
}

export const EMPTY_FILTERS: TraceFilters = {
  query: '',
  direction: 'all',
  interface: 'all',
};

export type SortColumn =
  | 'seq'
  | 'time'
  | 'direction'
  | 'pid'
  | 'interface'
  | 'method'
  | 'code'
  | 'size'
  | 'status'
  | 'device';

export type SortDirection = 'asc' | 'desc';

export interface TraceSort {
  column: SortColumn;
  direction: SortDirection;
}

function eventHaystack(event: TraceEvent): string {
  const flags = decodeTransactionFlags(event.txFlags)
    .map((flag) => flag.label)
    .join(' ');
  return [
    event.seq,
    event.debugId,
    event.pid,
    event.tid,
    event.uid,
    event.processLabel,
    event.interface,
    event.method,
    event.code,
    event.device,
    event.direction,
    event.status,
    flags,
  ]
    .join(' ')
    .toLowerCase();
}

/// 判断当前筛选条件是否与空条件有差异，用于显示「清除筛选」。
export function hasActiveFilters(filters: TraceFilters): boolean {
  return (
    filters.query.trim() !== '' ||
    filters.direction !== 'all' ||
    filters.interface !== 'all'
  );
}

/// 按筛选条件过滤事件。
export function filterEvents(
  events: readonly TraceEvent[],
  filters: TraceFilters,
): TraceEvent[] {
  const needle = filters.query.trim().toLowerCase();
  return events.filter((event) => {
    if (needle !== '' && !eventHaystack(event).includes(needle)) {
      return false;
    }
    if (filters.direction !== 'all' && event.direction !== filters.direction) {
      return false;
    }
    if (filters.interface !== 'all' && event.interface !== filters.interface) {
      return false;
    }
    return true;
  });
}

type SortValue = number | string;

function sortAccessor(event: TraceEvent, column: SortColumn): SortValue {
  switch (column) {
    case 'seq':
      return event.seq;
    case 'time':
      return event.timestampNs;
    case 'direction':
      return event.direction;
    case 'pid':
      return event.pid;
    case 'interface':
      return event.interface.toLowerCase();
    case 'method':
      return event.method.toLowerCase();
    case 'code':
      return event.code;
    case 'size':
      return event.dataSize;
    case 'status':
      return event.status;
    case 'device':
      return event.device;
    default:
      return event.seq;
  }
}

/// 按列与方向稳定排序。
export function sortEvents(
  events: readonly TraceEvent[],
  sort: TraceSort,
): TraceEvent[] {
  const sign = sort.direction === 'asc' ? 1 : -1;
  return [...events].sort((left, right) => {
    const a = sortAccessor(left, sort.column);
    const b = sortAccessor(right, sort.column);
    if (a < b) {
      return -1 * sign;
    }
    if (a > b) {
      return 1 * sign;
    }
    return 0;
  });
}

export type CorrelationRelation = 'self' | 'call' | 'reply';

export interface CorrelatedEvent {
  event: TraceEvent;
  relation: CorrelationRelation;
}

/// 找到与所选事件关联的 call/reply。
///
/// 关联键：以 `debugId` 为根。所选事件若是 reply（`replyToDebugId` 指向其 call），
/// 则以该 call 的 debugId 为根；否则以自身 debugId 为根。再收集所有
/// `debugId === root` 或 `replyToDebugId === root` 的事件。
export function correlate(
  selected: TraceEvent,
  events: readonly TraceEvent[],
): CorrelatedEvent[] {
  const root = selected.replyToDebugId ?? selected.debugId;
  const matches = events.filter(
    (event) => event.debugId === root || event.replyToDebugId === root,
  );
  return matches
    .map((event) => ({
      event,
      relation: relationOf(event, selected),
    }))
    .sort((left, right) => left.event.timestampNs - right.event.timestampNs);
}

function relationOf(event: TraceEvent, selected: TraceEvent): CorrelationRelation {
  if (event.id === selected.id) {
    return 'self';
  }
  return event.direction === 'reply' ? 'reply' : 'call';
}

/// 收集去重并按出现频率排序的字段取值，供筛选下拉框使用。
export function uniqueValues(
  events: readonly TraceEvent[],
  pick: (event: TraceEvent) => string,
): string[] {
  const counts = new Map<string, number>();
  for (const event of events) {
    counts.set(pick(event), (counts.get(pick(event)) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .map((entry) => entry[0]);
}
