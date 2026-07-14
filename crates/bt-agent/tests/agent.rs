//! `bt-agent` 公共 socket 事件转换契约。

use bt_agent::BinderEvent;
use bt_common::{BinderDevice, EventKind, MAX_INLINE_PAYLOAD};

#[test]
fn converts_transaction_event_without_device_path_lookup() {
    let mut payload = [0; MAX_INLINE_PAYLOAD];
    payload[..3].copy_from_slice(&[1, 2, 3]);
    let event = BinderEvent {
        sequence: 7,
        timestamp_ns: 8,
        kind: 1,
        pid: 9,
        tgid: 10,
        uid: 11,
        reply: 0,
        lost_before: 0,
        transaction_debug_id: 0,
        reply_to_debug_id: 0,
        transaction: 0,
        proc: 0,
        thread: 0,
        extra_buffers_size: 0,
        code: 12,
        flags: 13,
        data_size: 14,
        offsets_size: 15,
        target_handle: 16,
        sender_pid: 17,
        sender_euid: 18,
        payload_len: 3,
        payload_truncated: 0,
        reserved: [0; 7],
        payload,
    };

    let raw = event
        .to_raw_event()
        .expect("transaction event should convert");

    assert_eq!(raw.header.kind, EventKind::Transaction.as_raw());
    assert_eq!(raw.header.device, BinderDevice::UNKNOWN.as_raw());
    assert_eq!(raw.header.pid, 10);
    assert_eq!(raw.header.tid, 9);
    assert_eq!(raw.transaction.code, 12);
    assert_eq!(&raw.transaction.payload[..3], &[1, 2, 3]);
}
