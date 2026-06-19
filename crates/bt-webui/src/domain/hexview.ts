//! Payload 十六进制视图格式化。
//!
//! 纯函数：把字节数组切成 16 字节一行的 dump，每行包含 offset、按字节分组的
//! hex、以及 ASCII 栏。渲染层只负责把这份数据画出来，不在这里做任何副作用。

export interface HexRow {
  /** 该行起始字节偏移。 */
  offset: number;
  /** 16 个 hex 字节，不足位用空串占位以保持对齐。 */
  bytes: string[];
  /** ASCII 栏，不可打印字符用 `.`。 */
  ascii: string;
}

export interface HexDump {
  rows: HexRow[];
  totalBytes: number;
}

const DEFAULT_BYTES_PER_ROW = 16;

function toHex(byte: number): string {
  return byte.toString(16).padStart(2, '0');
}

function toAscii(byte: number): string {
  return byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : '.';
}

export interface HexDumpOptions {
  bytesPerRow?: number;
  maxRows?: number;
}

/// 把字节数组格式化成经典三栏 hex dump。
export function formatHexDump(bytes: Uint8Array, options?: HexDumpOptions): HexDump {
  const bytesPerRow = options?.bytesPerRow ?? DEFAULT_BYTES_PER_ROW;
  const rows: HexRow[] = [];
  const limit = options?.maxRows
    ? Math.min(bytes.length, options.maxRows * bytesPerRow)
    : bytes.length;

  for (let start = 0; start < limit; start += bytesPerRow) {
    const end = Math.min(start + bytesPerRow, limit);
    const rowBytes: string[] = [];
    let ascii = '';
    for (let i = 0; i < bytesPerRow; i += 1) {
      if (start + i < end) {
        const byte = bytes[start + i];
        rowBytes.push(toHex(byte));
        ascii += toAscii(byte);
      } else {
        rowBytes.push('');
        ascii += ' ';
      }
    }
    rows.push({ offset: start, bytes: rowBytes, ascii });
  }

  return { rows, totalBytes: bytes.length };
}
