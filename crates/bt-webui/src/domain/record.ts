//! JSONL 导出记录的原始类型定义。
//!
//! 这些类型严格对应 `bt-storage` 的 JSONL 行式导出格式（见
//! `crates/bt-storage/src/jsonl/record.rs`），只描述后端真实会写出的字段。
//! 前端展示用的派生 / 解码字段（interface、method、方向、延迟等）
//! 不属于 raw record，而是由 [`../trace`] 中的 normalized 模型承载。
//!
//! 设计原则：raw record 是事实来源，normalized 视图可丢失但不可违背它。
//! 真实数据接入时，把这里的类型直接喂给解析层即可。

/// Binder 事件类型，与 `bt-common::EventKind` 一致。
export type TraceKind =
  | 'diagnostic'
  | 'ioctl_enter'
  | 'ioctl_exit'
  | 'transaction'
  | 'reply';

/// Binder 设备名，与 `bt-common::BinderDevice::name` 一致。
export type BinderDeviceName =
  | 'binder'
  | 'hwbinder'
  | 'vndbinder'
  | 'binderfs'
  | 'unknown'
  | 'custom';

/// 记录 `object` 字段，标识逻辑事件类型。
export type TraceObject =
  | 'binder.transaction'
  | 'binder.reply'
  | 'binder.ioctl_enter'
  | 'binder.ioctl_exit'
  | 'agent.diagnostic'
  | 'program.version';

/// 进程三元组。
export interface RecordProcess {
  pid: number;
  tid: number;
  uid: number;
}

/// 事务详情；诊断类事件为 `null`。
export interface RecordTransaction {
  code: number;
  flags: number;
  data_size: number;
  offsets_size: number;
  target_handle: number;
  sender_pid: number;
  sender_euid: number;
  payload_truncated: boolean;
  payload_hex: string;
}

/// 单条记录 `data` 部分。
export interface RecordData {
  kind: TraceKind;
  binder_device: BinderDeviceName;
  process: RecordProcess;
  flags: number;
  sequence: number;
  transaction: RecordTransaction | null;
}

/// 单条 JSONL 记录的信封，对应 `JsonEnvelope`。
export interface RawTraceRecord {
  device_id: string;
  seq: number;
  timestamp_ns: number;
  object: TraceObject;
  data: RecordData;
}
