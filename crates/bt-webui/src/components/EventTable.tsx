//! 高性能事件表格。
//!
//! 渲染层只消费后端按需加载后的 256 行窗口。当前直接 `map` 行，列宽通过
//! colgroup 固定；接入虚拟列表时，只需替换 tbody 渲染，[`EventRow`] 已经是纯组件。

import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Messages } from '../i18n';
import {
  formatBytes,
  type SortColumn,
  type TraceEvent,
  type TraceSort,
} from '../domain';
import { TailIcon } from './icons';
import { DirectionToken, FlagBadges, StatusToken, TruncatedBadge } from './tokens';

type ColId =
  | 'seq'
  | 'time'
  | 'direction'
  | 'process'
  | 'interface'
  | 'method'
  | 'size'
  | 'status'
  | 'flags';

interface ColumnDef {
  id: ColId;
  sort: SortColumn | null;
  width: number;
  minWidth: number;
  maxWidth: number;
  numeric?: boolean;
}

const COLUMNS: ColumnDef[] = [
  { id: 'seq', sort: 'seq', width: 56, minWidth: 48, maxWidth: 120, numeric: true },
  { id: 'time', sort: 'time', width: 104, minWidth: 88, maxWidth: 180 },
  { id: 'direction', sort: 'direction', width: 76, minWidth: 64, maxWidth: 140 },
  { id: 'process', sort: 'pid', width: 188, minWidth: 120, maxWidth: 380 },
  { id: 'interface', sort: 'interface', width: 280, minWidth: 160, maxWidth: 720 },
  { id: 'method', sort: 'method', width: 230, minWidth: 140, maxWidth: 640 },
  { id: 'size', sort: 'size', width: 84, minWidth: 70, maxWidth: 160, numeric: true },
  { id: 'status', sort: 'status', width: 78, minWidth: 64, maxWidth: 140 },
  { id: 'flags', sort: null, width: 130, minWidth: 90, maxWidth: 260 },
];

const COLUMN_WIDTHS_STORAGE_KEY = 'bt-webui.table.column-widths.v1';

type ColumnWidths = Record<ColId, number>;

function headerLabel(id: ColId, messages: Messages): string {
  switch (id) {
    case 'seq':
      return messages.table.sequence;
    case 'time':
      return messages.table.time;
    case 'direction':
      return messages.table.direction;
    case 'process':
      return messages.table.process;
    case 'interface':
      return messages.table.interface;
    case 'method':
      return messages.table.method;
    case 'size':
      return messages.table.dataSize;
    case 'status':
      return messages.table.state;
    case 'flags':
      return messages.table.flags;
    default:
      return '';
  }
}

interface EventTableProps {
  events: TraceEvent[];
  selectedId: string | null;
  sort: TraceSort;
  followTail: boolean;
  canLoadOlder: boolean;
  canLoadNewer: boolean;
  isLoadingOlder: boolean;
  isLoadingNewer: boolean;
  messages: Messages;
  scrollRef: React.RefObject<HTMLDivElement | null>;
  onSort(column: SortColumn): void;
  onSelect(id: string): void;
  onFollowLatest(): Promise<void> | void;
  onLoadOlder(): Promise<void>;
  onLoadNewer(): Promise<void>;
}

export function EventTable({
  events,
  selectedId,
  sort,
  followTail,
  canLoadOlder,
  canLoadNewer,
  isLoadingOlder,
  isLoadingNewer,
  messages,
  scrollRef,
  onSort,
  onSelect,
  onFollowLatest,
  onLoadOlder,
  onLoadNewer,
}: EventTableProps) {
  // 是否贴底：决定「跳到最新」浮标是否显示。
  const [atBottom, setAtBottom] = useState(true);
  const [columnWidths, setColumnWidths] = useState<ColumnWidths>(loadColumnWidths);
  const olderLoadRef = useRef(false);
  const newerLoadRef = useRef(false);
  const tableWidth = useMemo(
    () => COLUMNS.reduce((sum, column) => sum + columnWidths[column.id], 0),
    [columnWidths],
  );

  const loadOlderFromTop = useCallback(async () => {
    const container = scrollRef.current;
    const anchorId = events[0]?.id;
    if (!container || !anchorId || olderLoadRef.current) {
      return;
    }

    olderLoadRef.current = true;
    try {
      await onLoadOlder();
      window.requestAnimationFrame(() => {
        const anchor = container.querySelector(`[data-event-id="${cssEscape(anchorId)}"]`);
        anchor?.scrollIntoView({ block: 'start' });
      });
    } finally {
      olderLoadRef.current = false;
    }
  }, [events, onLoadOlder, scrollRef]);

  const loadNewerFromBottom = useCallback(async () => {
    const container = scrollRef.current;
    const anchorId = events.at(-1)?.id;
    if (!container || !anchorId || newerLoadRef.current) {
      return;
    }

    newerLoadRef.current = true;
    try {
      await onLoadNewer();
      window.requestAnimationFrame(() => {
        const anchor = container.querySelector(`[data-event-id="${cssEscape(anchorId)}"]`);
        anchor?.scrollIntoView({ block: 'end' });
      });
    } finally {
      newerLoadRef.current = false;
    }
  }, [events, onLoadNewer, scrollRef]);

  const handleScroll = useCallback(() => {
    const container = scrollRef.current;
    if (!container) {
      return;
    }
    const distance = container.scrollHeight - container.scrollTop - container.clientHeight;
    setAtBottom(distance < 24);
    if (container.scrollTop < 32 && canLoadOlder && !isLoadingOlder) {
      void loadOlderFromTop();
    }
    if (distance < 32 && canLoadNewer && !isLoadingNewer) {
      void loadNewerFromBottom();
    }
  }, [
    canLoadNewer,
    canLoadOlder,
    isLoadingNewer,
    isLoadingOlder,
    loadNewerFromBottom,
    loadOlderFromTop,
    scrollRef,
  ]);

  const jumpToLatest = useCallback(() => {
    const container = scrollRef.current;
    Promise.resolve(onFollowLatest()).then(() => {
      window.requestAnimationFrame(() => {
        if (container) {
          container.scrollTop = container.scrollHeight;
        }
      });
    });
  }, [scrollRef, onFollowLatest]);

  const handleColumnResizeStart = useCallback((
    column: ColumnDef,
    event: React.PointerEvent<HTMLButtonElement>,
  ) => {
    event.preventDefault();
    event.stopPropagation();

    const startX = event.clientX;
    const startWidth = columnWidths[column.id];
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    document.body.classList.add('bt-column-resizing');

    const handlePointerMove = (moveEvent: PointerEvent) => {
      const nextWidth = clamp(
        startWidth + moveEvent.clientX - startX,
        column.minWidth,
        column.maxWidth,
      );
      setColumnWidths((previous) => ({
        ...previous,
        [column.id]: nextWidth,
      }));
    };

    const cleanup = () => {
      document.body.classList.remove('bt-column-resizing');
      window.removeEventListener('pointermove', handlePointerMove);
      window.removeEventListener('pointerup', cleanup);
      window.removeEventListener('pointercancel', cleanup);
      if (handle.hasPointerCapture(event.pointerId)) {
        handle.releasePointerCapture(event.pointerId);
      }
    };

    window.addEventListener('pointermove', handlePointerMove);
    window.addEventListener('pointerup', cleanup);
    window.addEventListener('pointercancel', cleanup);
  }, [columnWidths]);

  const handleColumnReset = useCallback((column: ColumnDef) => {
    setColumnWidths((previous) => ({
      ...previous,
      [column.id]: column.width,
    }));
  }, []);

  // follow tail：新事件到达时滚动到底部。
  useEffect(() => {
    const container = scrollRef.current;
    if (!container || !followTail) {
      return;
    }
    container.scrollTop = container.scrollHeight;
  }, [followTail, events.length, scrollRef]);

  // 选中行滚动到可视区。
  useEffect(() => {
    const container = scrollRef.current;
    if (!container || !selectedId) {
      return;
    }
    const row = container.querySelector(`[data-event-id="${cssEscape(selectedId)}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [selectedId, scrollRef]);

  useEffect(() => {
    persistColumnWidths(columnWidths);
  }, [columnWidths]);

  return (
    <div className="bt-table-pane">
      <div
        className="bt-table-scroll bt-scroll"
        ref={scrollRef}
        role="region"
        aria-label={messages.table.ariaLabel}
        tabIndex={0}
        onScroll={handleScroll}
      >
        <table className="bt-table" style={{ minWidth: tableWidth }}>
          <colgroup>
            {COLUMNS.map((column) => (
              <col key={column.id} style={{ width: columnWidths[column.id] }} />
            ))}
          </colgroup>
          <thead className="bt-thead">
            <tr>
              {COLUMNS.map((column) => {
                const label = headerLabel(column.id, messages);
                return (
                  <SortableHeader
                    key={column.id}
                    column={column}
                    label={label}
                    resizeLabel={messages.table.resizeColumn(label)}
                    sort={sort}
                    onSort={onSort}
                    onResizeStart={handleColumnResizeStart}
                    onResetWidth={handleColumnReset}
                  />
                );
              })}
            </tr>
          </thead>
          <tbody>
            {events.map((event) => (
              <EventRow
                key={event.id}
                event={event}
                isSelected={event.id === selectedId}
                messages={messages}
                onSelect={onSelect}
              />
            ))}
          </tbody>
        </table>
        {events.length === 0 ? <div className="bt-empty">{messages.table.noEvents}</div> : null}
      </div>
      {!atBottom && events.length > 0 ? (
        <button type="button" className="bt-jump-latest" onClick={jumpToLatest}>
          <TailIcon width={14} height={14} />
          {messages.table.jumpLatest}
        </button>
      ) : null}
    </div>
  );
}

interface SortableHeaderProps {
  column: ColumnDef;
  label: string;
  resizeLabel: string;
  sort: TraceSort;
  onSort(column: SortColumn): void;
  onResizeStart(column: ColumnDef, event: React.PointerEvent<HTMLButtonElement>): void;
  onResetWidth(column: ColumnDef): void;
}

function SortableHeader({
  column,
  label,
  resizeLabel,
  sort,
  onSort,
  onResizeStart,
  onResetWidth,
}: SortableHeaderProps) {
  const isActive = column.sort !== null && sort.column === column.sort;
  const nextDirection = isActive && sort.direction === 'desc' ? 'asc' : 'desc';
  const thClass = [
    column.numeric ? 'bt-th-num' : '',
    'bt-th--resizable',
  ].filter(Boolean).join(' ');

  if (column.sort === null) {
    return (
      <th className={thClass}>
        <span className="bt-th-content">
          <span>{label}</span>
        </span>
        <ColumnResizeHandle
          label={resizeLabel}
          column={column}
          onResizeStart={onResizeStart}
          onResetWidth={onResetWidth}
        />
      </th>
    );
  }

  return (
    <th className={thClass}>
      <button
        type="button"
        className="bt-th-button"
        aria-label={`${label} ${nextDirection === 'asc' ? '▲' : '▼'}`}
        onClick={() => onSort(column.sort as SortColumn)}
      >
        <span>{label}</span>
        {isActive ? (
          <span className="bt-th-marker" aria-hidden>
            {sort.direction === 'asc' ? '▲' : '▼'}
          </span>
        ) : null}
      </button>
      <ColumnResizeHandle
        label={resizeLabel}
        column={column}
        onResizeStart={onResizeStart}
        onResetWidth={onResetWidth}
      />
    </th>
  );
}

interface ColumnResizeHandleProps {
  label: string;
  column: ColumnDef;
  onResizeStart(column: ColumnDef, event: React.PointerEvent<HTMLButtonElement>): void;
  onResetWidth(column: ColumnDef): void;
}

function ColumnResizeHandle({
  label,
  column,
  onResizeStart,
  onResetWidth,
}: ColumnResizeHandleProps) {
  return (
    <button
      type="button"
      className="bt-column-resizer"
      aria-label={label}
      title={label}
      onPointerDown={(event) => onResizeStart(column, event)}
      onDoubleClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onResetWidth(column);
      }}
    />
  );
}

interface EventRowProps {
  event: TraceEvent;
  isSelected: boolean;
  messages: Messages;
  onSelect(id: string): void;
}

const EventRow = memo(function EventRow({ event, isSelected, messages, onSelect }: EventRowProps) {
  const rowClass = [
    'bt-tr',
    isSelected ? 'bt-tr-selected' : '',
    event.status === 'slow' ? 'bt-tr--slow' : '',
    event.status === 'error' ? 'bt-tr--error' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <tr
      className={rowClass}
      data-event-id={event.id}
      onClick={() => onSelect(event.id)}
    >
      <td className="bt-td-num bt-td-seq">{event.seq}</td>
      <td className="bt-td-time">{event.timestampLabel}</td>
      <td>
        <DirectionToken direction={event.direction} label={messages.directions[event.direction]} />
      </td>
      <td>
        <div className="bt-table-cell-block">
          <span className="bt-cell-primary" title={event.processLabel}>
            {event.processLabel}
          </span>
          <span className="bt-cell-meta">
            <span>
              {event.pid}:{event.tid} · {messages.table.uid} {event.uid}
            </span>
            <span className="bt-device-tag">{event.device}</span>
          </span>
        </div>
      </td>
      <td>
        <span className="bt-cell-secondary" title={event.interface}>
          {event.interface}
        </span>
      </td>
      <td>
        <div className="bt-table-cell-block">
          <span className="bt-cell-method" title={event.method}>
            {event.method}
          </span>
          <span className="bt-cell-meta">
            {messages.table.code} {event.code}
            {event.payloadTruncated ? <TruncatedBadgeInline label={messages.badges.truncated} /> : null}
          </span>
        </div>
      </td>
      <td className="bt-td-num bt-td-size">
        {formatBytes(event.dataSize)}
      </td>
      <td>
        <StatusToken status={event.status} label={messages.statuses[event.status]} />
      </td>
      <td>
        <FlagBadges flags={event.txFlags} />
      </td>
    </tr>
  );
});

function TruncatedBadgeInline({ label }: { label: string }) {
  return (
    <span style={{ marginLeft: 6, verticalAlign: 'middle' }}>
      <TruncatedBadge label={label} />
    </span>
  );
}

function cssEscape(value: string): string {
  return value.replace(/["\\]/g, '\\$&');
}

function defaultColumnWidths(): ColumnWidths {
  const widths = {} as ColumnWidths;
  for (const column of COLUMNS) {
    widths[column.id] = column.width;
  }
  return widths;
}

function loadColumnWidths(): ColumnWidths {
  const widths = defaultColumnWidths();
  try {
    const raw = window.localStorage.getItem(COLUMN_WIDTHS_STORAGE_KEY);
    if (!raw) {
      return widths;
    }

    const parsed = JSON.parse(raw) as Partial<Record<ColId, unknown>>;
    for (const column of COLUMNS) {
      const value = parsed[column.id];
      if (typeof value === 'number' && Number.isFinite(value)) {
        widths[column.id] = clamp(value, column.minWidth, column.maxWidth);
      }
    }
  } catch {
    return widths;
  }
  return widths;
}

function persistColumnWidths(widths: ColumnWidths) {
  try {
    window.localStorage.setItem(COLUMN_WIDTHS_STORAGE_KEY, JSON.stringify(widths));
  } catch {
    // localStorage 不可用时只影响偏好保存，拖拽本身仍然生效。
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}
