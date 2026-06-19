//! 一键复制按钮：复制文本到剪贴板，短暂给出「已复制」反馈。

import { useCallback, useState } from 'react';
import { CopyIcon } from './icons';

interface CopyButtonProps {
  value: string;
  ariaLabel: string;
  copiedLabel: string;
}

export function CopyButton({ value, ariaLabel, copiedLabel }: CopyButtonProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    const write = navigator.clipboard?.writeText(value);
    if (write && typeof write.then === 'function') {
      write.then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }).catch(() => undefined);
    }
  }, [value]);

  return (
    <button
      type="button"
      className="bt-copy"
      aria-label={ariaLabel}
      title={copied ? copiedLabel : ariaLabel}
      onClick={handleCopy}
    >
      <CopyIcon width={12} height={12} />
    </button>
  );
}
