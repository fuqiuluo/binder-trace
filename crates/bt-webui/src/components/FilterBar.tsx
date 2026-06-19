//! 查询筛选区：全文搜索 + 方向 + 接口。变化即时生效。
//!
//! pid/tid/uid/method/code/flags/状态/设备 等字段不再单列下拉，统一并入
//! 左侧全文搜索框（见 `domain/select` 的 `eventHaystack`）。

import { memo, useCallback, useEffect, useRef, useState } from 'react';
import type { Messages } from '../i18n';
import type { TraceFilters } from '../domain';
import { SearchIcon, TrashIcon } from './icons';

interface FilterBarProps {
  filters: TraceFilters;
  hasActive: boolean;
  interfaceOptions: string[];
  messages: Messages;
  onChange(patch: Partial<TraceFilters>): void;
  onClear(): void;
}

export function FilterBar({
  filters,
  hasActive,
  interfaceOptions,
  messages,
  onChange,
  onClear,
}: FilterBarProps) {
  const f = messages.filters;

  // 稳定的提交回调：让 memo 化的搜索框不随事件流的高频重渲染而重渲染。
  const onQuery = useCallback((query: string) => onChange({ query }), [onChange]);

  return (
    <div className="bt-filterbar">
      <label className="bt-filter-field bt-filter-grow">
        <span className="bt-filter-label">{f.query}</span>
        <SearchField
          value={filters.query}
          placeholder={f.queryPlaceholder}
          label={f.query}
          onCommit={onQuery}
        />
      </label>

      <FilterSelect
        label={f.direction}
        ariaLabel={f.direction}
        value={filters.direction}
        allLabel={f.all}
        options={[
          { value: 'call', label: messages.directions.call },
          { value: 'reply', label: messages.directions.reply },
          { value: 'oneway', label: messages.directions.oneway },
        ]}
        onChange={(value) => onChange({ direction: value as TraceFilters['direction'] })}
      />

      <FilterSelect
        label={f.interface}
        ariaLabel={f.interface}
        value={filters.interface}
        allLabel={f.all}
        options={interfaceOptions.map((value) => ({ value, label: value }))}
        onChange={(value) => onChange({ interface: value })}
      />

      <button
        type="button"
        className="bt-btn-ghost"
        aria-label={f.clear}
        onClick={onClear}
        disabled={!hasActive}
      >
        <TrashIcon />
        <span>{f.clear}</span>
      </button>
    </div>
  );
}

interface SearchFieldProps {
  value: string;
  placeholder: string;
  label: string;
  onCommit(value: string): void;
}

/// 全文搜索框。
///
/// 用本地态镜像输入值，并隔离父层（事件流每 ~1.4s 一次的重渲染）。这能避免受控
/// `value` 在中文 / 日文等 IME 组合输入过程中被父层重渲染强行回写，导致「打不进字」。
/// 组合输入期间只更新本地态、不向上提交，待 `compositionend` 再提交，避免按半个拼音过滤。
const SearchField = memo(function SearchField({
  value,
  placeholder,
  label,
  onCommit,
}: SearchFieldProps) {
  const [text, setText] = useState(value);
  const composing = useRef(false);
  const committed = useRef(value);

  // 外部 value 变化（如「清除筛选」）且非自身提交时，同步回本地态。
  useEffect(() => {
    if (value !== committed.current) {
      committed.current = value;
      setText(value);
    }
  }, [value]);

  const emit = useCallback(
    (next: string) => {
      committed.current = next;
      onCommit(next);
    },
    [onCommit],
  );

  return (
    <span className="bt-search">
      <SearchIcon className="bt-search-icon" />
      <input
        className="bt-search-input"
        type="search"
        placeholder={placeholder}
        value={text}
        aria-label={label}
        onChange={(event) => {
          const next = event.target.value;
          setText(next);
          if (!composing.current) {
            emit(next);
          }
        }}
        onCompositionStart={() => {
          composing.current = true;
        }}
        onCompositionEnd={(event) => {
          composing.current = false;
          const next = event.currentTarget.value;
          setText(next);
          emit(next);
        }}
      />
    </span>
  );
});

interface FilterSelectProps {
  label: string;
  ariaLabel: string;
  value: string;
  allLabel: string;
  options: Array<{ value: string; label: string }>;
  onChange(value: string): void;
}

function FilterSelect({ label, ariaLabel, value, allLabel, options, onChange }: FilterSelectProps) {
  return (
    <label className="bt-filter-field">
      <span className="bt-filter-label">{label}</span>
      <select
        className="bt-select"
        aria-label={ariaLabel}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="all">{allLabel}</option>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}
