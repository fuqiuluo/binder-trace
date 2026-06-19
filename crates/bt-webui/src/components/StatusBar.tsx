//! 底部状态栏：运行 / 跟随 / 当前位置 + 捕获计数。

import type { Messages } from '../i18n';
import { formatNumber } from '../domain';

interface StatusBarProps {
  visibleCount: number;
  displayLimit: number;
  displayLimitOptions: readonly number[];
  matchedCount: number;
  windowStartIndex: number | null;
  windowEndIndex: number | null;
  totalCount: number;
  droppedCount: number;
  isRunning: boolean;
  followTail: boolean;
  messages: Messages;
  onDisplayLimitChange(value: number): void;
}

export function StatusBar({
  visibleCount,
  displayLimit,
  displayLimitOptions,
  matchedCount,
  windowStartIndex,
  windowEndIndex,
  totalCount,
  droppedCount,
  isRunning,
  followTail,
  messages,
  onDisplayLimitChange,
}: StatusBarProps) {
  const s = messages.streamStats;
  const st = messages.stream;

  return (
    <footer className="bt-statusbar" role="contentinfo">
      <span className={`bt-stat${isRunning ? ' bt-stat--ok' : ' bt-stat--warn'}`}>
        <span className={`bt-status-dot${isRunning ? '' : ' bt-status-dot--paused'}`} />
        <span className="bt-stat-value">
          {isRunning ? st.running : st.paused}
        </span>
      </span>

      {followTail ? (
        <>
          <span className="bt-stat-sep" />
          <span className="bt-stat">{st.following}</span>
        </>
      ) : null}

      <span className="bt-stat-sep" />
      <Stat
        label={s.position}
        value={formatPosition(windowStartIndex, windowEndIndex, matchedCount)}
      />
      <Stat label={s.visible} value={formatNumber(visibleCount)} />
      <Stat label={s.total} value={formatNumber(totalCount)} />
      <Stat
        label={s.dropped}
        value={formatNumber(droppedCount)}
        tone={droppedCount > 0 ? 'danger' : undefined}
      />
      <span className="bt-statusbar-spacer" />
      <label className="bt-window-size">
        <span className="bt-stat-label">{s.displayLimit}</span>
        <select
          className="bt-select bt-window-size-select"
          value={displayLimit}
          aria-label={s.displayLimit}
          onChange={(event) => onDisplayLimitChange(Number(event.target.value))}
        >
          {displayLimitOptions.map((option) => (
            <option key={option} value={option}>
              {formatNumber(option)}
            </option>
          ))}
        </select>
      </label>
    </footer>
  );
}

function formatPosition(
  start: number | null,
  end: number | null,
  matchedCount: number,
): string {
  if (start === null || end === null) {
    return `0 / ${formatNumber(matchedCount)}`;
  }

  return `${formatNumber(start)}-${formatNumber(end)} / ${formatNumber(matchedCount)}`;
}

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: 'ok' | 'warn' | 'danger';
}) {
  const toneClass = tone ? ` bt-stat--${tone}` : '';
  return (
    <span className={`bt-stat${toneClass}`}>
      <span className="bt-stat-label">{label}</span>
      <span className="bt-stat-value">{value}</span>
    </span>
  );
}
