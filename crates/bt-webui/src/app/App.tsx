//! Binder Trace WebUI 根组件。
//!
//! 只负责状态编排与布局：筛选条件交给后端执行，前端仅排序和渲染当前窗口。

import { useCallback, useMemo, useRef, useState } from 'react';
import { DetailPanel } from '../components/DetailPanel';
import { EventTable } from '../components/EventTable';
import { FilterBar } from '../components/FilterBar';
import { StatusBar } from '../components/StatusBar';
import { Toolbar } from '../components/Toolbar';
import { useI18n } from '../i18n';
import { useTableKeyboard } from '../hooks/useTableKeyboard';
import { useTraceStream } from '../hooks/useTraceStream';
import {
  correlate,
  EMPTY_FILTERS,
  hasActiveFilters,
  sortEvents,
  type SortColumn,
  type TraceFilters,
  type TraceSort,
} from '../domain';

// 默认按时间升序：新事件追加到列表底部（向下新增），配合 follow tail 滚到底部跟随。
const INITIAL_SORT: TraceSort = { column: 'time', direction: 'asc' };

export function App() {
  const { locale, messages, setLocale } = useI18n();

  const [filters, setFilters] = useState<TraceFilters>(EMPTY_FILTERS);
  const stream = useTraceStream(filters);
  const [sort, setSort] = useState<TraceSort>(INITIAL_SORT);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detailWidth, setDetailWidth] = useState(460);

  const tableScrollRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);

  const sortedEvents = useMemo(() => sortEvents(stream.events, sort), [stream.events, sort]);
  const selectedEvent = useMemo(
    () => stream.events.find((event) => event.id === selectedId) ?? null,
    [stream.events, selectedId],
  );
  const correlation = useMemo(
    () => (selectedEvent ? correlate(selectedEvent, stream.events) : []),
    [selectedEvent, stream.events],
  );

  const handleFilters = useCallback((patch: Partial<TraceFilters>) => {
    setFilters((previous) => ({ ...previous, ...patch }));
  }, []);

  const handleSort = useCallback((column: SortColumn) => {
    setSort((previous) =>
      previous.column === column
        ? { column, direction: previous.direction === 'asc' ? 'desc' : 'asc' }
        : { column, direction: 'desc' },
    );
  }, []);

  const handleSelect = useCallback(
    (id: string) => {
      setSelectedId(id);
      // 手动选中即停止跟随尾部，避免新事件刷新冲掉正在查看的上下文。
      stream.setFollowTail(false);
    },
    [stream],
  );

  const handleClearSelection = useCallback(() => setSelectedId(null), []);

  const handleNavigate = useCallback((id: string) => setSelectedId(id), []);

  const handleClearEvents = useCallback(() => {
    stream.clear();
    setSelectedId(null);
  }, [stream]);

  const handleExport = useCallback(() => {
    const payload = JSON.stringify(
      { device: stream.deviceContext, exported_at: new Date().toISOString(), events: sortedEvents.map((event) => event.raw) },
      null,
      2,
    );
    const blob = new Blob([payload], { type: 'application/json;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `binder-trace-${Date.now()}.json`;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
  }, [sortedEvents, stream.deviceContext]);

  useTableKeyboard({
    rows: sortedEvents,
    selectedId,
    onSelect: handleSelect,
    onClear: handleClearSelection,
    onEnter: () => detailRef.current?.focus(),
  });

  return (
    <div className="bt-app">
      <Toolbar
        title={messages.app.title}
        deviceContext={stream.error ?? stream.deviceContext}
        isRunning={stream.isRunning}
        followTail={stream.followTail}
        locale={locale}
        messages={messages}
        onToggleRunning={stream.toggleRunning}
        onToggleFollowTail={stream.toggleFollowTail}
        onClear={handleClearEvents}
        onExport={handleExport}
        onLocaleChange={setLocale}
      />

      <FilterBar
        filters={filters}
        hasActive={hasActiveFilters(filters)}
        interfaceOptions={stream.interfaceOptions}
        messages={messages}
        onChange={handleFilters}
        onClear={() => setFilters(EMPTY_FILTERS)}
      />

      <div className="bt-workspace">
        <EventTable
          events={sortedEvents}
          selectedId={selectedId}
          sort={sort}
          followTail={stream.followTail}
          canLoadOlder={stream.hasMoreBefore}
          canLoadNewer={stream.hasMoreAfter}
          isLoadingOlder={stream.isLoadingBefore}
          isLoadingNewer={stream.isLoadingAfter}
          messages={messages}
          scrollRef={tableScrollRef}
          onSort={handleSort}
          onSelect={handleSelect}
          onFollowLatest={stream.showLatest}
          onLoadOlder={stream.loadOlder}
          onLoadNewer={stream.loadNewer}
        />

        {selectedEvent ? (
          <DetailPanel
            event={selectedEvent}
            correlation={correlation}
            detailRef={detailRef}
            width={detailWidth}
            messages={messages}
            onClose={handleClearSelection}
            onNavigate={handleNavigate}
            onResize={setDetailWidth}
          />
        ) : null}
      </div>
      <StatusBar
        visibleCount={sortedEvents.length}
        matchedCount={stream.matchedCount}
        windowStartIndex={stream.windowStartIndex}
        windowEndIndex={stream.windowEndIndex}
        totalCount={stream.totalCount}
        droppedCount={stream.droppedCount}
        isRunning={stream.isRunning}
        followTail={stream.followTail}
        messages={messages}
      />
    </div>
  );
}
