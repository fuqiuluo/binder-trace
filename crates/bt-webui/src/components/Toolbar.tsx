//! 顶部工具栏：品牌 / 设备状态、运行暂停、跟随尾部、清空、导出、语言。

import type { Locale, Messages } from '../i18n';
import { localeOptions } from '../i18n';
import {
  DownloadIcon,
  LogoGlyph,
  PauseIcon,
  PlayIcon,
  TailIcon,
  TrashIcon,
} from './icons';

interface ToolbarProps {
  title: string;
  deviceContext: string;
  isRunning: boolean;
  followTail: boolean;
  locale: Locale;
  messages: Messages;
  onToggleRunning(): void;
  onToggleFollowTail(): void;
  onClear(): void;
  onExport(): void;
  onLocaleChange(value: Locale): void;
}

export function Toolbar({
  title,
  deviceContext,
  isRunning,
  followTail,
  locale,
  messages,
  onToggleRunning,
  onToggleFollowTail,
  onClear,
  onExport,
  onLocaleChange,
}: ToolbarProps) {
  return (
    <header className="bt-toolbar">
      <div className="bt-brand">
        <div className="bt-brand-mark" aria-hidden>
          <LogoGlyph width={16} height={16} />
        </div>
        <div style={{ minWidth: 0 }}>
          <div className="bt-brand-title">{title}</div>
          <div className="bt-brand-sub">{deviceContext}</div>
        </div>
      </div>

      <span className="bt-toolbar-spacer" />

      <div className="bt-toolbar-group">
        <button
          type="button"
          className={`bt-btn-ghost bt-btn-ghost--icon${isRunning ? '' : ' bt-btn-ghost--active'}`}
          aria-label={isRunning ? messages.actions.pause : messages.actions.resume}
          title={isRunning ? messages.actions.pause : messages.actions.resume}
          onClick={onToggleRunning}
        >
          {isRunning ? <PauseIcon /> : <PlayIcon />}
        </button>
        <button
          type="button"
          className={`bt-btn-ghost bt-btn-ghost--icon${followTail ? ' bt-btn-ghost--active' : ''}`}
          aria-label={messages.actions.followTail}
          aria-pressed={followTail}
          title={messages.actions.followTail}
          onClick={onToggleFollowTail}
        >
          <TailIcon />
        </button>

        <span className="bt-divider" />

        <button
          type="button"
          className="bt-btn-ghost bt-btn-ghost--icon"
          aria-label={messages.actions.clear}
          title={messages.actions.clear}
          onClick={onClear}
        >
          <TrashIcon />
        </button>
        <button
          type="button"
          className="bt-btn-ghost bt-btn-ghost--icon"
          aria-label={messages.actions.export}
          title={messages.actions.export}
          onClick={onExport}
        >
          <DownloadIcon />
        </button>

        <span className="bt-divider" />

        <select
          className="bt-select"
          aria-label={messages.language.label}
          value={locale}
          onChange={(event) => onLocaleChange(event.target.value as Locale)}
        >
          {localeOptions.map((option) => (
            <option key={option.locale} value={option.locale}>
              {option.nativeLabel}
            </option>
          ))}
        </select>
      </div>
    </header>
  );
}
