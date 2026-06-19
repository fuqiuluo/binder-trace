//! 选中事件的详情面板：摘要 / Payload(hex) / Raw JSON / 关联调用。
//!
//! 顶部 header 固定，下方 tabs 切换内容。payload hex viewer 按 offset + hex + ascii
//! 三栏展示；关联 tab 基于 debugId/replyToDebugId 展示 call/reply。

import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import hljs from 'highlight.js/lib/core';
import jsonLang from 'highlight.js/lib/languages/json';
import type { Messages } from '../i18n';
import {
  bytesToHex,
  decodeTransactionFlags,
  formatBytes,
  formatFlagsHex,
  formatHexDump,
  type CorrelatedEvent,
  type TraceEvent,
} from '../domain';
import { CloseIcon } from './icons';
import { CopyButton } from './CopyButton';
import { DirectionToken, StatusToken } from './tokens';

// 只注册 JSON 语言，避免引入 highlight.js 全量语言包（保持 bundle 精简）。
hljs.registerLanguage('json', jsonLang);

type DetailTab = 'summary' | 'payload' | 'raw' | 'correlation';
type MountedTabs = Record<DetailTab, boolean>;

function mountedTabsFor(tab: DetailTab): MountedTabs {
  return {
    summary: tab === 'summary',
    payload: tab === 'payload',
    raw: tab === 'raw',
    correlation: tab === 'correlation',
  };
}

interface DetailPanelProps {
  event: TraceEvent;
  correlation: CorrelatedEvent[];
  detailRef: React.RefObject<HTMLDivElement | null>;
  width: number;
  messages: Messages;
  onClose(): void;
  onNavigate(id: string): void;
  onResize(width: number): void;
}

export function DetailPanel({
  event,
  correlation,
  detailRef,
  width,
  messages,
  onClose,
  onNavigate,
  onResize,
}: DetailPanelProps) {
  const [tab, setTab] = useState<DetailTab>('summary');
  const [panelWidth, setPanelWidth] = useState(width);
  const [mountedState, setMountedState] = useState(() => ({
    eventId: event.id,
    tabs: mountedTabsFor('summary'),
  }));
  const frameRef = useRef<number | null>(null);
  const pendingWidthRef = useRef(width);
  const m = messages.detail;
  const im = messages.inspector;
  const mountedTabs = mountedState.eventId === event.id ? mountedState.tabs : mountedTabsFor(tab);

  const selectTab = useCallback(
    (nextTab: DetailTab) => {
      setTab(nextTab);
      setMountedState((current) => {
        const baseTabs = current.eventId === event.id ? current.tabs : mountedTabsFor(tab);
        return {
          eventId: event.id,
          tabs: {
            ...baseTabs,
            [nextTab]: true,
          },
        };
      });
    },
    [event.id, tab],
  );

  useEffect(() => {
    setPanelWidth(width);
    pendingWidthRef.current = width;
  }, [width]);

  useEffect(() => {
    return () => {
      if (frameRef.current !== null) {
        window.cancelAnimationFrame(frameRef.current);
      }
    };
  }, []);

  // 拖拽左边缘改变面板宽度。面板在右侧，向左拖动（clientX 减小）即增大宽度。
  const startResize = (downEvent: React.PointerEvent<HTMLDivElement>) => {
    downEvent.preventDefault();
    const startX = downEvent.clientX;
    const startWidth = panelWidth;
    const pointerId = downEvent.pointerId;
    downEvent.currentTarget.setPointerCapture(pointerId);

    const commitFrame = () => {
      frameRef.current = null;
      setPanelWidth(pendingWidthRef.current);
    };

    const handleMove = (moveEvent: PointerEvent) => {
      const max = Math.min(960, window.innerWidth - 420);
      const next = startWidth + (startX - moveEvent.clientX);
      pendingWidthRef.current = Math.max(340, Math.min(max, next));
      if (frameRef.current === null) {
        frameRef.current = window.requestAnimationFrame(commitFrame);
      }
    };
    const stop = () => {
      window.removeEventListener('pointermove', handleMove);
      window.removeEventListener('pointerup', stop);
      window.removeEventListener('pointercancel', stop);
      document.body.classList.remove('bt-col-resizing');
      if (frameRef.current !== null) {
        window.cancelAnimationFrame(frameRef.current);
        frameRef.current = null;
      }
      setPanelWidth(pendingWidthRef.current);
      onResize(pendingWidthRef.current);
    };
    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', stop);
    window.addEventListener('pointercancel', stop);
    document.body.classList.add('bt-col-resizing');
  };

  return (
    <aside className="bt-detail-pane" aria-label={im.title} style={{ width: panelWidth }}>
      <div
        className="bt-detail-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label={im.resize}
        onPointerDown={startResize}
      />
      <div className="bt-detail-header">
        <div className="bt-detail-head-main">
          <div className="bt-detail-title-row">
            <span className="bt-detail-title" title={`${event.interface}#${event.method}`}>
              {event.method}
            </span>
            <DirectionToken direction={event.direction} label={messages.directions[event.direction]} />
            <StatusToken status={event.status} label={messages.statuses[event.status]} />
          </div>
          <div className="bt-detail-route">
            <span className="bt-detail-route-node" title={event.processLabel}>
              {event.processLabel}
            </span>
            <span className="bt-detail-route-arrow" aria-hidden>
              →
            </span>
            <span className="bt-detail-route-iface" title={event.interface}>
              {event.interface}
            </span>
          </div>
        </div>
        <button type="button" className="bt-icon-btn" aria-label={im.close} onClick={onClose}>
          <CloseIcon />
        </button>
      </div>

      <nav className="bt-detail-tabs" aria-label={im.tabs}>
        <TabButton active={tab === 'summary'} onClick={() => selectTab('summary')}>
          {m.summary}
        </TabButton>
        {event.hasTransaction ? (
          <TabButton active={tab === 'payload'} onClick={() => selectTab('payload')}>
            {m.payload}
          </TabButton>
        ) : null}
        <TabButton active={tab === 'raw'} onClick={() => selectTab('raw')}>
          {m.raw}
        </TabButton>
        <TabButton active={tab === 'correlation'} onClick={() => selectTab('correlation')}>
          {m.correlation}
        </TabButton>
      </nav>

      <div className="bt-detail-body" ref={detailRef} tabIndex={-1}>
        {mountedTabs.summary ? (
          <div className="bt-detail-panel" hidden={tab !== 'summary'}>
            <div className="bt-tab-scroll bt-scroll">
              <SummaryTab event={event} messages={messages} />
            </div>
          </div>
        ) : null}
        {mountedTabs.payload && event.hasTransaction ? (
          <div className="bt-detail-panel" hidden={tab !== 'payload'}>
            <PayloadTab event={event} messages={messages} />
          </div>
        ) : null}
        {mountedTabs.raw ? (
          <div className="bt-detail-panel" hidden={tab !== 'raw'}>
            <RawTab event={event} messages={messages} />
          </div>
        ) : null}
        {mountedTabs.correlation ? (
          <div className="bt-detail-panel" hidden={tab !== 'correlation'}>
            <div className="bt-tab-scroll bt-scroll">
              <CorrelationTab
                correlation={correlation}
                messages={messages}
                onNavigate={onNavigate}
              />
            </div>
          </div>
        ) : null}
      </div>
    </aside>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick(): void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      className={`bt-tab${active ? ' bt-tab--active' : ''}`}
      aria-selected={active}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

const SummaryTab = memo(function SummaryTab({ event, messages }: { event: TraceEvent; messages: Messages }) {
  const im = messages.inspector;
  const sm = messages.detail;
  const flagsHex = formatFlagsHex(event.txFlags);

  const flagLabels = useMemo(
    () => decodeTransactionFlags(event.txFlags).map((f) => f.label).join(' | ') || 'none',
    [event.txFlags],
  );

  return (
    <div>
      <section className="bt-section">
        <div className="bt-section-head">
          <h3 className="bt-section-title">{sm.summary}</h3>
        </div>
        <div className="bt-summary-grid">
          <SummaryCell label={im.direction}>
            <DirectionToken direction={event.direction} label={messages.directions[event.direction]} />
          </SummaryCell>
          <SummaryCell label={im.status}>
            <StatusToken status={event.status} label={messages.statuses[event.status]} />
          </SummaryCell>
          <SummaryCell label={im.sequence} value={String(event.seq)} />
          <SummaryCell label={im.time} value={event.timestampLabel} />
        </div>
      </section>

      <section className="bt-section">
        <div className="bt-section-head">
          <h3 className="bt-section-title">{sm.parsed}</h3>
        </div>
        <div className="bt-kv-card">
          <KV label={im.interface} value={event.interface} copyValue={event.interface} messages={messages} />
          <KV label={im.method} value={event.method} copyValue={event.method} messages={messages} />
        </div>
      </section>

      <section className="bt-section">
        <div className="bt-section-head">
          <h3 className="bt-section-title">{sm.caller}</h3>
        </div>
        <div className="bt-kv-card">
          <KV label={im.processLabel} value={event.processLabel} messages={messages} />
          <KV label={im.pid} value={String(event.pid)} copyValue={String(event.pid)} messages={messages} />
          <KV label={im.tid} value={String(event.tid)} copyValue={String(event.tid)} messages={messages} />
          <KV label={im.uid} value={String(event.uid)} copyValue={String(event.uid)} messages={messages} />
          <KV label={im.senderPid} value={String(event.senderPid)} messages={messages} />
          <KV label={im.senderEuid} value={String(event.senderEuid)} messages={messages} />
        </div>
      </section>

      {event.hasTransaction ? (
        <section className="bt-section">
          <div className="bt-section-head">
            <h3 className="bt-section-title">{sm.transaction}</h3>
          </div>
          <div className="bt-kv-card">
            <KV label={im.device} value={event.device} messages={messages} />
            <KV label={im.targetHandle} value={String(event.targetHandle)} messages={messages} />
            <KV label={im.code} value={String(event.code)} copyValue={String(event.code)} messages={messages} />
            <KV label={im.flags} value={`${flagsHex} (${flagLabels})`} copyValue={flagsHex} messages={messages} />
            <KV label={im.dataSize} value={formatBytes(event.dataSize)} messages={messages} />
            <KV label={im.offsetsSize} value={formatBytes(event.offsetsSize)} messages={messages} />
            <KV label={im.payloadTruncated} value={event.payloadTruncated ? sm.yes : sm.no} messages={messages} />
            <KV label={im.capturedBytes} value={`${formatBytes(event.payload.length)}`} messages={messages} />
            <KV label={im.latency} value={event.latencyUs === null ? '—' : `${event.latencyUs} µs`} messages={messages} />
          </div>
        </section>
      ) : null}
    </div>
  );
}, sameEventAndMessages);

const PayloadTab = memo(function PayloadTab({ event, messages }: { event: TraceEvent; messages: Messages }) {
  const im = messages.inspector;
  const payloadHex = useMemo(() => bytesToHex(event.payload), [event.payload]);
  const bodyRef = useRef<HTMLDivElement>(null);
  // 每行字节数随面板宽度自适应：拖宽面板就显示更多字节，而不是留空。8 的倍数便于分组对齐。
  const [bytesPerRow, setBytesPerRow] = useState(16);

  useEffect(() => {
    const el = bodyRef.current;
    if (!el) {
      return;
    }
    const measure = () => {
      // 11.5px 等宽字体单字符约 6.95px；每字节 ≈ hex(2) + 分隔(1) + ascii(1) = 4 字符。
      const fixed = 130; // offset 列 + 三栏内边距 + 分隔线，留余量避免溢出
      const raw = (el.clientWidth - fixed) / (6.95 * 4);
      const nextBytesPerRow = Math.max(8, Math.min(64, Math.floor(raw / 8) * 8));
      setBytesPerRow((current) => (current === nextBytesPerRow ? current : nextBytesPerRow));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [event.id]);

  const dump = useMemo(
    () => formatHexDump(event.payload, { bytesPerRow }),
    [bytesPerRow, event.payload],
  );

  return (
    <div className="bt-tab-fill">
      <div className="bt-hex-toolbar">
        <span>
          {formatBytes(event.payload.length)}
          {event.payloadTruncated
            ? ` · ${messages.detail.capturedOf(event.payload.length, event.dataSize)}`
            : ''}
        </span>
        <CopyButton value={payloadHex} ariaLabel={im.copyPayload} copiedLabel={messages.detail.copied} />
      </div>
      {event.payload.length === 0 ? (
        <div className="bt-correlation-empty">{messages.detail.emptyPayload}</div>
      ) : (
        <div className="bt-hex-body bt-scroll" ref={bodyRef}>
          {dump.rows.map((row) => (
            <div className="bt-hex-row" key={row.offset}>
              <span className="bt-hex-offset">{row.offset.toString(16).padStart(8, '0')}</span>
              <span className="bt-hex-bytes">{formatBytesRow(row.bytes)}</span>
              <span className="bt-hex-ascii">{row.ascii}</span>
            </div>
          ))}
          <div className="bt-hex-end">
            {messages.detail.payloadEnd} · {formatBytes(event.payload.length)}
          </div>
        </div>
      )}
    </div>
  );
}, sameEventAndMessages);

const RawTab = memo(function RawTab({ event, messages }: { event: TraceEvent; messages: Messages }) {
  const [wrap, setWrap] = useState(false);
  const m = messages.detail;
  const text = useMemo(() => JSON.stringify(event.raw, null, 2), [event.raw]);
  // highlight.js 输出已转义 HTML，配色由 CSS 里映射的 .hljs-* 类决定。
  const html = useMemo(() => hljs.highlight(text, { language: 'json' }).value, [text]);

  return (
    <div className="bt-tab-fill">
      <div className="bt-code-toolbar">
        <span className="bt-code-hint" title={m.rawHint}>
          {m.rawHint}
        </span>
        <span className="bt-code-actions">
          <button
            type="button"
            className={`bt-chip-btn${wrap ? ' bt-chip-btn--active' : ''}`}
            aria-pressed={wrap}
            onClick={() => setWrap((value) => !value)}
          >
            {m.wrap}
          </button>
          <CopyButton value={text} ariaLabel={messages.inspector.copyRaw} copiedLabel={m.copied} />
        </span>
      </div>
      <pre
        className={`bt-code bt-scroll${wrap ? ' bt-code--wrap' : ''}`}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </div>
  );
}, sameEventAndMessages);

const CorrelationTab = memo(function CorrelationTab({
  correlation,
  messages,
  onNavigate,
}: {
  correlation: CorrelatedEvent[];
  messages: Messages;
  onNavigate(id: string): void;
}) {
  const im = messages.inspector;
  if (correlation.length <= 1) {
    return <div className="bt-correlation-empty">{messages.detail.noCorrelation}</div>;
  }
  return (
    <div>
      {correlation.map((entry) => (
        <button
          key={entry.event.id}
          type="button"
          className={`bt-correlation-row${entry.relation === 'self' ? ' bt-correlation-row--self' : ''}`}
          onClick={() => onNavigate(entry.event.id)}
          aria-current={entry.relation === 'self'}
        >
          <span className="bt-correlation-rel">
            {entry.relation === 'self'
              ? messages.detail.self
              : entry.relation === 'reply'
                ? im.reply
                : im.send}
          </span>
          <span className="bt-correlation-method" title={entry.event.method}>
            {entry.event.method}
          </span>
          <span className="bt-correlation-seq">#{entry.event.seq}</span>
          <span className="bt-correlation-seq">
            {entry.event.latencyUs === null ? '—' : `${entry.event.latencyUs}µs`}
          </span>
        </button>
      ))}
    </div>
  );
}, sameCorrelationTabProps);

function sameEventAndMessages(
  previous: { event: TraceEvent; messages: Messages },
  next: { event: TraceEvent; messages: Messages },
): boolean {
  return previous.event === next.event && previous.messages === next.messages;
}

function sameCorrelationTabProps(
  previous: {
    correlation: CorrelatedEvent[];
    messages: Messages;
    onNavigate(id: string): void;
  },
  next: {
    correlation: CorrelatedEvent[];
    messages: Messages;
    onNavigate(id: string): void;
  },
): boolean {
  return (
    previous.correlation === next.correlation &&
    previous.messages === next.messages &&
    previous.onNavigate === next.onNavigate
  );
}

function SummaryCell({ label, value, children }: { label: string; value?: string; children?: React.ReactNode }) {
  return (
    <div className="bt-summary-cell">
      <div className="bt-summary-cell-label">{label}</div>
      <div className="bt-summary-cell-value">{value ?? children}</div>
    </div>
  );
}

function KV({
  label,
  value,
  copyValue,
  messages,
}: {
  label: string;
  value: string;
  copyValue?: string;
  messages: Messages;
}) {
  return (
    <div className="bt-kv">
      <span className="bt-kv-key">{label}</span>
      <span className="bt-kv-val" title={value}>{value}</span>
      {copyValue ? (
        <CopyButton value={copyValue} ariaLabel={`${messages.detail.copy} ${label}`} copiedLabel={messages.detail.copied} />
      ) : null}
    </div>
  );
}

// 每 8 字节加一个分组空隙；缺失字节（末行不足）补两个空格，保持各行等宽对齐。
function formatBytesRow(bytes: string[]): string {
  let out = '';
  for (let i = 0; i < bytes.length; i += 1) {
    if (i > 0) {
      out += i % 8 === 0 ? '  ' : ' ';
    }
    out += bytes[i] || '  ';
  }
  return out;
}
