//! 生产事件流：从 `binder-trace webui` 提供的本地 API 拉取真实内核事件。
//!
//! 选区状态不在这里维护——它属于 App 层。本 hook 只负责事件来源、轮询暂停、
//! follow tail 状态、后端筛选窗口与服务端累计计数。

import { useCallback, useEffect, useRef, useState } from 'react';
import { buildTraceEvent, type TraceEnrichment, type TraceEvent } from '../domain';
import type { RawTraceRecord, TraceFilters } from '../domain';

const POLL_INTERVAL_MS = 600;
const DEFAULT_DEVICE_CONTEXT = 'production · binder transaction stream';

interface ApiTraceEvent {
  raw: RawTraceRecord;
  enrichment: TraceEnrichment;
}

interface EventsResponse {
  events: ApiTraceEvent[];
  total_count: number;
  retained_count: number;
  matched_count: number;
  dropped_count: number;
  backend_evicted_count: number;
  oldest_seq: number | null;
  newest_seq: number | null;
  page_start_index: number | null;
  page_end_index: number | null;
  has_more_before: boolean;
  has_more_after: boolean;
  interfaces: string[];
  source_state: 'connecting' | 'connected' | 'disabled' | 'error';
  error: string | null;
  device_context: string;
}

export interface TraceStream {
  events: TraceEvent[];
  totalCount: number;
  retainedCount: number;
  matchedCount: number;
  droppedCount: number;
  backendEvictedCount: number;
  windowStartIndex: number | null;
  windowEndIndex: number | null;
  hasMoreBefore: boolean;
  hasMoreAfter: boolean;
  isLoadingBefore: boolean;
  isLoadingAfter: boolean;
  interfaceOptions: string[];
  isRunning: boolean;
  followTail: boolean;
  deviceContext: string;
  sourceState: EventsResponse['source_state'];
  error: string | null;
  setRunning(value: boolean): void;
  toggleRunning(): void;
  setFollowTail(value: boolean): void;
  toggleFollowTail(): void;
  showLatest(): Promise<void>;
  loadOlder(): Promise<void>;
  loadNewer(): Promise<void>;
  clear(): void;
}

export function useTraceStream(filters: TraceFilters, windowLimit: number): TraceStream {
  const [events, setEvents] = useState<TraceEvent[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [retainedCount, setRetainedCount] = useState(0);
  const [matchedCount, setMatchedCount] = useState(0);
  const [droppedCount, setDroppedCount] = useState(0);
  const [backendEvictedCount, setBackendEvictedCount] = useState(0);
  const [windowStartIndex, setWindowStartIndex] = useState<number | null>(null);
  const [windowEndIndex, setWindowEndIndex] = useState<number | null>(null);
  const [hasMoreBefore, setHasMoreBefore] = useState(false);
  const [hasMoreAfter, setHasMoreAfter] = useState(false);
  const [isLoadingBefore, setLoadingBefore] = useState(false);
  const [isLoadingAfter, setLoadingAfter] = useState(false);
  const [interfaceOptions, setInterfaceOptions] = useState<string[]>([]);
  const [deviceContext, setDeviceContext] = useState(DEFAULT_DEVICE_CONTEXT);
  const [sourceState, setSourceState] = useState<EventsResponse['source_state']>('connecting');
  const [error, setError] = useState<string | null>(null);
  const [isRunning, setRunning] = useState(true);
  const [followTail, setFollowTail] = useState(true);

  const abortRef = useRef<AbortController | null>(null);
  const eventCacheRef = useRef(new Map<string, { api: ApiTraceEvent; event: TraceEvent }>());
  const eventsRef = useRef<TraceEvent[]>([]);
  const windowRangeRef = useRef<WindowRange>({ start: null, end: null });
  const requestGenerationRef = useRef(0);

  const applyResponse = useCallback((data: EventsResponse, mode: PageMode) => {
    setTotalCount(data.total_count);
    setRetainedCount(data.retained_count);
    setMatchedCount(data.matched_count);
    setDroppedCount(data.dropped_count);
    setBackendEvictedCount(data.backend_evicted_count);
    setInterfaceOptions(data.interfaces);
    setDeviceContext(data.device_context || DEFAULT_DEVICE_CONTEXT);
    setSourceState(data.source_state);
    setError(data.error);
    setHasMoreBefore(data.has_more_before);
    setHasMoreAfter(data.has_more_after);

    const pageEvents = materializeEvents(eventCacheRef.current, data.events);
    const nextEvents = mergeWindow(eventsRef.current, pageEvents, mode, windowLimit);
    const nextRange = nextWindowRange(data, mode, nextEvents.length);
    eventsRef.current = nextEvents;
    windowRangeRef.current = nextRange;
    pruneCache(eventCacheRef.current, nextEvents);
    setEvents(nextEvents);
    setWindowStartIndex(nextRange.start);
    setWindowEndIndex(nextRange.end);
  }, [windowLimit]);

  const fetchPage = useCallback(
    async (request: PageRequest, generation: number, signal?: AbortSignal) => {
      const response = await fetch(eventsUrl(filters, request), {
        cache: 'no-store',
        signal,
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const data = (await response.json()) as EventsResponse;
      if (generation !== requestGenerationRef.current) {
        return;
      }
      applyResponse(data, request.mode);
    },
    [applyResponse, filters],
  );

  const refreshLatest = useCallback(async () => {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    const generation = requestGenerationRef.current;

    try {
      await fetchPage({ mode: 'latest', limit: windowLimit }, generation, controller.signal);
    } catch (caught) {
      if (controller.signal.aborted) {
        return;
      }
      setSourceState('error');
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }, [fetchPage, windowLimit]);

  useEffect(() => {
    requestGenerationRef.current += 1;
    eventCacheRef.current.clear();
    eventsRef.current = [];
    windowRangeRef.current = { start: null, end: null };
    setEvents([]);
    setWindowStartIndex(null);
    setWindowEndIndex(null);
    setHasMoreBefore(false);
    setHasMoreAfter(false);
    setFollowTail(true);
    void refreshLatest();
  }, [filters, refreshLatest, windowLimit]);

  useEffect(() => {
    if (!isRunning || !followTail) {
      return;
    }
    void refreshLatest();
    const handle = window.setInterval(() => void refreshLatest(), POLL_INTERVAL_MS);
    return () => {
      window.clearInterval(handle);
      abortRef.current?.abort();
    };
  }, [followTail, isRunning, refreshLatest]);

  const clear = useCallback(async () => {
    await fetch('/api/events/clear', { method: 'POST' }).catch(() => undefined);
    setEvents([]);
    eventsRef.current = [];
    windowRangeRef.current = { start: null, end: null };
    setTotalCount(0);
    setRetainedCount(0);
    setMatchedCount(0);
    setDroppedCount(0);
    setBackendEvictedCount(0);
    setWindowStartIndex(null);
    setWindowEndIndex(null);
    setHasMoreBefore(false);
    setHasMoreAfter(false);
    eventCacheRef.current.clear();
  }, []);

  const toggleRunning = useCallback(() => setRunning((value) => !value), []);
  const toggleFollowTail = useCallback(() => setFollowTail((value) => !value), []);
  const showLatest = useCallback(async () => {
    setFollowTail(true);
    await refreshLatest();
  }, [refreshLatest]);
  const loadOlder = useCallback(async () => {
    const firstSeq = eventsRef.current[0]?.seq;
    if (firstSeq === undefined || !hasMoreBefore || isLoadingBefore) {
      return;
    }

    setFollowTail(false);
    setLoadingBefore(true);
    try {
      await fetchPage(
        { mode: 'older', limit: pageLoadLimit(windowLimit), beforeSeq: firstSeq },
        requestGenerationRef.current,
      );
    } catch (caught) {
      setSourceState('error');
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoadingBefore(false);
    }
  }, [fetchPage, hasMoreBefore, isLoadingBefore, windowLimit]);

  const loadNewer = useCallback(async () => {
    const lastSeq = eventsRef.current.at(-1)?.seq;
    if (lastSeq === undefined || !hasMoreAfter || isLoadingAfter) {
      return;
    }

    setLoadingAfter(true);
    try {
      await fetchPage(
        { mode: 'newer', limit: pageLoadLimit(windowLimit), afterSeq: lastSeq },
        requestGenerationRef.current,
      );
    } catch (caught) {
      setSourceState('error');
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoadingAfter(false);
    }
  }, [fetchPage, hasMoreAfter, isLoadingAfter, windowLimit]);

  return {
    events,
    totalCount,
    retainedCount,
    matchedCount,
    droppedCount,
    backendEvictedCount,
    windowStartIndex,
    windowEndIndex,
    hasMoreBefore,
    hasMoreAfter,
    isLoadingBefore,
    isLoadingAfter,
    interfaceOptions,
    isRunning,
    followTail,
    deviceContext,
    sourceState,
    error,
    setRunning,
    toggleRunning,
    setFollowTail,
    toggleFollowTail,
    showLatest,
    loadOlder,
    loadNewer,
    clear,
  };
}

type PageMode = 'latest' | 'older' | 'newer';

interface PageRequest {
  mode: PageMode;
  limit: number;
  beforeSeq?: number;
  afterSeq?: number;
}

interface WindowRange {
  start: number | null;
  end: number | null;
}

function eventsUrl(filters: TraceFilters, request: PageRequest): string {
  const params = new URLSearchParams();
  params.set('limit', String(request.limit));
  if (filters.query.trim() !== '') {
    params.set('query', filters.query);
  }
  if (filters.direction !== 'all') {
    params.set('direction', filters.direction);
  }
  if (filters.interface !== 'all') {
    params.set('interface', filters.interface);
  }
  if (request.beforeSeq !== undefined) {
    params.set('before_seq', String(request.beforeSeq));
  }
  if (request.afterSeq !== undefined) {
    params.set('after_seq', String(request.afterSeq));
  }
  return `/api/events?${params.toString()}`;
}

function materializeEvents(
  previousCache: Map<string, { api: ApiTraceEvent; event: TraceEvent }>,
  apiEvents: ApiTraceEvent[],
): TraceEvent[] {
  return apiEvents.map((api) => {
    const id = `${api.raw.device_id}:${api.raw.seq}`;
    const cached = previousCache.get(id);
    const event = cached && sameApiEvent(cached.api, api)
      ? cached.event
      : buildTraceEvent(api.raw, api.enrichment);
    previousCache.set(id, { api, event });
    return event;
  });
}

function mergeWindow(
  previous: TraceEvent[],
  pageEvents: TraceEvent[],
  mode: PageMode,
  windowLimit: number,
): TraceEvent[] {
  if (mode === 'latest') {
    return trimWindow(dedupeEvents(pageEvents), 'end', windowLimit);
  }
  if (pageEvents.length === 0) {
    return previous;
  }
  const merged = mode === 'older'
    ? dedupeEvents([...pageEvents, ...previous])
    : dedupeEvents([...previous, ...pageEvents]);
  return trimWindow(merged, mode === 'older' ? 'start' : 'end', windowLimit);
}

function nextWindowRange(
  response: EventsResponse,
  mode: PageMode,
  visibleCount: number,
): WindowRange {
  if (visibleCount === 0) {
    return { start: null, end: null };
  }

  if (mode === 'newer' && response.page_end_index !== null) {
    return {
      start: response.page_end_index - visibleCount + 1,
      end: response.page_end_index,
    };
  }

  if (response.page_start_index !== null) {
    return {
      start: response.page_start_index,
      end: response.page_start_index + visibleCount - 1,
    };
  }

  return { start: null, end: null };
}

function dedupeEvents(events: TraceEvent[]): TraceEvent[] {
  const seen = new Set<string>();
  const unique: TraceEvent[] = [];
  for (const event of events) {
    if (seen.has(event.id)) {
      continue;
    }
    seen.add(event.id);
    unique.push(event);
  }
  return unique;
}

function trimWindow(events: TraceEvent[], keep: 'start' | 'end', windowLimit: number): TraceEvent[] {
  if (events.length <= windowLimit) {
    return events;
  }
  return keep === 'start'
    ? events.slice(0, windowLimit)
    : events.slice(events.length - windowLimit);
}

function pageLoadLimit(windowLimit: number): number {
  return Math.min(windowLimit, Math.max(128, Math.floor(windowLimit / 2)));
}

function pruneCache(
  cache: Map<string, { api: ApiTraceEvent; event: TraceEvent }>,
  visibleEvents: TraceEvent[],
) {
  const visibleIds = new Set(visibleEvents.map((event) => event.id));
  for (const id of cache.keys()) {
    if (!visibleIds.has(id)) {
      cache.delete(id);
    }
  }
}

function sameApiEvent(left: ApiTraceEvent, right: ApiTraceEvent): boolean {
  return (
    left.raw.timestamp_ns === right.raw.timestamp_ns &&
    left.raw.data.transaction?.payload_hex === right.raw.data.transaction?.payload_hex &&
    left.enrichment.interface === right.enrichment.interface &&
    left.enrichment.method === right.enrichment.method &&
    left.enrichment.processLabel === right.enrichment.processLabel &&
    left.enrichment.debugId === right.enrichment.debugId &&
    left.enrichment.replyToDebugId === right.enrichment.replyToDebugId &&
    left.enrichment.latencyUs === right.enrichment.latencyUs &&
    left.enrichment.status === right.enrichment.status
  );
}
