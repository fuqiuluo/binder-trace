//! 展示用的 normalized Binder 事件模型与纯派生 / 格式化函数。
//!
//! 这里只放「给定输入必然得到同一输出」的纯函数：方向解码、flag 解码、
//! payload 解析、时间 / 字节 / 延迟格式化、百分位计算。筛选、排序、统计、
//! 关联等组合逻辑放在 [`./select`]，payload 十六进制视图放在 [`./hexview`]。

import type { BinderDeviceName, RawTraceRecord, TraceKind, TraceObject } from './record';

/// 调用方向。由 `kind` 与事务 flag (`TF_ONE_WAY`) 派生，不在 raw record 中。
export type Direction = 'call' | 'reply' | 'oneway';

/// UI 派生状态，用于慢调用 / 异常高亮与筛选。不在 raw record 中。
export type EventStatus = 'ok' | 'slow' | 'error';

/// 真实数据中不直接存在、需由解码层补全的语义信息。
///
/// 由 WebUI 后端补全：`interface` / `method` 来自平台方法表
///（`bt-decoder::AndroidPlatformMethods`）与 payload interface token，
/// `debugId` / `replyToDebugId` 来自内核 Binder debug id，`latencyUs`
/// 由 call/reply 配对计算。service（友好服务名）无法从 trace 可靠推断，故不纳入模型。
export interface TraceEnrichment {
  interface: string;
  method: string;
  processLabel: string;
  debugId: number;
  replyToDebugId: number | null;
  latencyUs: number | null;
  status: EventStatus;
}

/// 一条规范化后的 Binder 事件，供表格 / 详情面板消费。
export interface TraceEvent {
  /** 稳定唯一 id，跨设备仍唯一。 */
  id: string;
  seq: number;
  timestampNs: number;
  timestampLabel: string;
  device: BinderDeviceName;
  object: TraceObject;
  kind: TraceKind;

  pid: number;
  tid: number;
  uid: number;
  processLabel: string;
  /** 事件级 flags（`data.flags`）。 */
  eventFlags: number;

  hasTransaction: boolean;
  code: number;
  /** 事务级 flags（`data.transaction.flags`）。 */
  txFlags: number;
  dataSize: number;
  offsetsSize: number;
  targetHandle: number;
  senderPid: number;
  senderEuid: number;
  payloadTruncated: boolean;
  payload: Uint8Array;

  interface: string;
  method: string;
  debugId: number;
  replyToDebugId: number | null;
  latencyUs: number | null;
  status: EventStatus;
  direction: Direction;

  raw: RawTraceRecord;
}

/// Binder 事务 flag 位，与内核 `enum transaction_flags` 一致。
export const TF = {
  ONE_WAY: 0x01,
  ROOT_OBJECT: 0x04,
  STATUS_CODE_PENDING: 0x08,
  ACCEPT_FDS: 0x10,
  CLEAR_BUF: 0x20,
} as const;

export interface DecodedFlag {
  label: string;
  bit: number;
}

const FLAG_TABLE: ReadonlyArray<readonly [number, string]> = [
  [TF.ONE_WAY, 'ONE_WAY'],
  [TF.ROOT_OBJECT, 'ROOT_OBJECT'],
  [TF.STATUS_CODE_PENDING, 'STATUS_CODE_PENDING'],
  [TF.ACCEPT_FDS, 'ACCEPT_FDS'],
  [TF.CLEAR_BUF, 'CLEAR_BUF'],
];

/// 把事务 flags 解码成可读标签列表；未知位不会丢失，可从原始值还原。
export function decodeTransactionFlags(flags: number): DecodedFlag[] {
  const result: DecodedFlag[] = [];
  for (const [bit, label] of FLAG_TABLE) {
    if ((flags & bit) === bit) {
      result.push({ bit, label });
    }
  }
  return result;
}

/// 由事件类型与事务 flags 推断调用方向。
export function decodeDirection(kind: TraceKind, txFlags: number): Direction {
  if (kind === 'reply') {
    return 'reply';
  }
  if (kind === 'transaction') {
    return (txFlags & TF.ONE_WAY) === TF.ONE_WAY ? 'oneway' : 'call';
  }
  // ioctl / 诊断事件不参与事务表格，落到 call 仅作类型兜底。
  return 'call';
}

/// 把 raw record 与解码层补全的语义信息合并成可展示事件。纯函数。
export function buildTraceEvent(
  raw: RawTraceRecord,
  enrichment: TraceEnrichment,
): TraceEvent {
  const tx = raw.data.transaction;
  const payload = tx ? hexToBytes(tx.payload_hex) : new Uint8Array(0);

  return {
    id: `${raw.device_id}:${raw.seq}`,
    seq: raw.seq,
    timestampNs: raw.timestamp_ns,
    timestampLabel: formatTimestampNs(raw.timestamp_ns),
    device: raw.data.binder_device,
    object: raw.object,
    kind: raw.data.kind,
    pid: raw.data.process.pid,
    tid: raw.data.process.tid,
    uid: raw.data.process.uid,
    processLabel: enrichment.processLabel,
    eventFlags: raw.data.flags,
    hasTransaction: tx !== null,
    code: tx?.code ?? 0,
    txFlags: tx?.flags ?? 0,
    dataSize: tx?.data_size ?? 0,
    offsetsSize: tx?.offsets_size ?? 0,
    targetHandle: tx?.target_handle ?? 0,
    senderPid: tx?.sender_pid ?? 0,
    senderEuid: tx?.sender_euid ?? 0,
    payloadTruncated: tx?.payload_truncated ?? false,
    payload,
    interface: enrichment.interface,
    method: enrichment.method,
    debugId: enrichment.debugId,
    replyToDebugId: enrichment.replyToDebugId,
    latencyUs: enrichment.latencyUs,
    status: enrichment.status,
    direction: decodeDirection(raw.data.kind, tx?.flags ?? 0),
    raw,
  };
}

const NS_PER_MS = 1_000_000;

/// 把纳秒级 epoch 时间戳格式化成 `HH:MM:SS.mmm`（本地时区）。
export function formatTimestampNs(ns: number): string {
  const date = new Date(Math.floor(ns / NS_PER_MS));
  const hh = String(date.getHours()).padStart(2, '0');
  const mm = String(date.getMinutes()).padStart(2, '0');
  const ss = String(date.getSeconds()).padStart(2, '0');
  const ms = String(date.getMilliseconds()).padStart(3, '0');
  return `${hh}:${mm}:${ss}.${ms}`;
}

/// 把字节数格式化成人类可读的单位。
export function formatBytes(bytes: number): string {
  if (bytes <= 0) {
    return '0 B';
  }
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;
}

/// 把整数格式化成带千分位的字符串。
export function formatNumber(value: number): string {
  return value.toLocaleString('en-US');
}

/// 把 flags 数值格式化成 8 位 hex。
export function formatFlagsHex(flags: number): string {
  return `0x${(flags >>> 0).toString(16).padStart(8, '0')}`;
}

/// 把 hex 字符串解析成字节数组，长度为奇数时左侧补 0。
export function hexToBytes(hex: string): Uint8Array {
  const normalized = hex.length % 2 === 0 ? hex : `0${hex}`;
  const out = new Uint8Array(Math.floor(normalized.length / 2));
  for (let i = 0; i < out.length; i += 1) {
    const value = Number.parseInt(normalized.slice(i * 2, i * 2 + 2), 16);
    out[i] = Number.isNaN(value) ? 0 : value;
  }
  return out;
}

/// 把字节数组编码成小写 hex 字符串。
export function bytesToHex(bytes: Uint8Array): string {
  let out = '';
  for (let i = 0; i < bytes.length; i += 1) {
    out += bytes[i].toString(16).padStart(2, '0');
  }
  return out;
}
