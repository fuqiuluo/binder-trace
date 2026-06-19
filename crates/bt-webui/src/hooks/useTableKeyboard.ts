//! 表格键盘导航：↑/↓（或 j/k）在可见行间移动选区，Enter 聚焦详情，Esc 清除。
//!
//! 用 ref 持有最新回调，监听器只绑定一次，避免每帧重绑。在输入框 / 下拉框 /
//! 文本域内输入时不拦截按键。

import { useEffect, useRef } from 'react';
import type { TraceEvent } from '../domain';

export interface TableKeyboardOptions {
  rows: readonly TraceEvent[];
  selectedId: string | null;
  onSelect(id: string): void;
  onClear(): void;
  /** 选中行后将焦点交给详情面板。 */
  onEnter?(): void;
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tag = target.tagName;
  return (
    tag === 'INPUT' ||
    tag === 'SELECT' ||
    tag === 'TEXTAREA' ||
    target.isContentEditable
  );
}

export function useTableKeyboard(options: TableKeyboardOptions): void {
  const latest = useRef(options);
  latest.current = options;

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (isEditableTarget(event.target)) {
        return;
      }
      const { rows, selectedId, onSelect, onClear, onEnter } = latest.current;
      const key = event.key;
      if (key === 'ArrowDown' || key === 'j') {
        event.preventDefault();
        moveSelection(rows, selectedId, onSelect, 1);
      } else if (key === 'ArrowUp' || key === 'k') {
        event.preventDefault();
        moveSelection(rows, selectedId, onSelect, -1);
      } else if (key === 'Escape') {
        onClear();
      } else if (key === 'Enter') {
        onEnter?.();
      }
    }

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);
}

function moveSelection(
  rows: readonly TraceEvent[],
  selectedId: string | null,
  onSelect: (id: string) => void,
  delta: number,
): void {
  if (rows.length === 0) {
    return;
  }
  const currentIndex = selectedId
    ? rows.findIndex((event) => event.id === selectedId)
    : -1;
  let nextIndex = currentIndex + delta;
  if (currentIndex === -1) {
    nextIndex = delta > 0 ? 0 : rows.length - 1;
  }
  nextIndex = Math.min(rows.length - 1, Math.max(0, nextIndex));
  const target = rows[nextIndex];
  if (target) {
    onSelect(target.id);
  }
}
