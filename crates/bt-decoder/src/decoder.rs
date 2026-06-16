//! raw Binder event 解码器实现。

use bt_common::{BinderDevice, EventKind, MAX_INLINE_PAYLOAD, RawBinderEvent};

use crate::{DecodeError, DecodedEvent, DecodedTransaction, DecoderConfig};

/// raw Binder event 解码器。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Decoder {
    config: DecoderConfig,
}

impl Decoder {
    /// 使用指定配置创建解码器。
    pub const fn new(config: DecoderConfig) -> Self {
        Self { config }
    }

    /// 解码一条 raw Binder event。
    ///
    /// # 错误
    /// 当事件类型未知或 payload 长度超出固定 inline 缓冲区时返回 [`DecodeError`]。
    pub fn decode(&self, raw: &RawBinderEvent) -> Result<DecodedEvent, DecodeError> {
        let kind = EventKind::try_from(raw.header.kind)?;
        let transaction = match kind {
            EventKind::Diagnostic => None,
            EventKind::IoctlEnter
            | EventKind::IoctlExit
            | EventKind::Transaction
            | EventKind::Reply => Some(self.decode_transaction(raw)?),
        };

        Ok(DecodedEvent {
            kind,
            device: BinderDevice::from(raw.header.device),
            pid: raw.header.pid,
            tid: raw.header.tid,
            uid: raw.header.uid,
            flags: raw.header.flags,
            timestamp_ns: raw.header.timestamp_ns,
            sequence: raw.header.sequence,
            transaction,
        })
    }

    fn decode_transaction(&self, raw: &RawBinderEvent) -> Result<DecodedTransaction, DecodeError> {
        let payload_len = usize::try_from(raw.transaction.payload_len)
            .map_err(|_| DecodeError::PayloadLengthOutOfRange(raw.transaction.payload_len))?;

        if payload_len > MAX_INLINE_PAYLOAD {
            return Err(DecodeError::PayloadLengthOutOfRange(
                raw.transaction.payload_len,
            ));
        }

        let payload_len = core::cmp::min(payload_len, self.config.max_payload_bytes);

        Ok(DecodedTransaction {
            code: raw.transaction.code,
            flags: raw.transaction.flags,
            data_size: raw.transaction.data_size,
            offsets_size: raw.transaction.offsets_size,
            target_handle: raw.transaction.target_handle,
            sender_pid: raw.transaction.sender_pid,
            sender_euid: raw.transaction.sender_euid,
            payload_truncated: raw.transaction.payload_truncated != 0
                || payload_len < raw.transaction.payload_len as usize,
            payload: raw.transaction.payload[..payload_len].to_vec(),
        })
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(DecoderConfig::default())
    }
}
