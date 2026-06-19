//! 表格与详情面板共用的展示性 token：方向、状态、flags、截断徽标。

import { memo } from 'react';
import { decodeTransactionFlags, type Direction, type EventStatus } from '../domain';
import { ScissorsIcon } from './icons';

interface DirectionTokenProps {
  direction: Direction;
  label: string;
}

function DirectionTokenImpl({ direction, label }: DirectionTokenProps) {
  return <span className={`bt-token bt-token--${direction}`}>{label}</span>;
}

export const DirectionToken = memo(DirectionTokenImpl);

interface StatusTokenProps {
  status: EventStatus;
  label: string;
}

function StatusTokenImpl({ status, label }: StatusTokenProps) {
  return <span className={`bt-token bt-token--${status}`}>{label}</span>;
}

export const StatusToken = memo(StatusTokenImpl);

interface FlagBadgesProps {
  flags: number;
}

function FlagBadgesImpl({ flags }: FlagBadgesProps) {
  const decoded = decodeTransactionFlags(flags);
  if (decoded.length === 0) {
    return <span className="bt-cell-sub">0</span>;
  }
  return (
    <span>
      {decoded.map((flag) => (
        <span key={flag.bit} className="bt-flag">
          {flag.label}
        </span>
      ))}
    </span>
  );
}

export const FlagBadges = memo(FlagBadgesImpl);

interface TruncatedBadgeProps {
  label: string;
}

export function TruncatedBadge({ label }: TruncatedBadgeProps) {
  return (
    <span className="bt-trunc-badge" title={label}>
      <ScissorsIcon width={10} height={10} />
      {label}
    </span>
  );
}
