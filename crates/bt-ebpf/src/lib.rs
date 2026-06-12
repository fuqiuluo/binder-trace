#![no_std]

use bt_common::{MAX_INLINE_PAYLOAD, RawBinderEvent};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CapturePolicy {
    pub max_payload_bytes: usize,
    pub target_pid: Option<u32>,
    pub target_uid: Option<u32>,
}

impl CapturePolicy {
    pub const fn metadata_only() -> Self {
        Self {
            max_payload_bytes: 0,
            target_pid: None,
            target_uid: None,
        }
    }

    pub fn allows_identity(self, pid: u32, uid: u32) -> bool {
        self.target_pid.is_none_or(|target| target == pid)
            && self.target_uid.is_none_or(|target| target == uid)
    }
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            max_payload_bytes: MAX_INLINE_PAYLOAD,
            target_pid: None,
            target_uid: None,
        }
    }
}

pub fn bounded_payload_len(payload_size: usize, policy: CapturePolicy) -> (usize, bool) {
    let max_capture = core::cmp::min(policy.max_payload_bytes, MAX_INLINE_PAYLOAD);
    let captured = core::cmp::min(payload_size, max_capture);
    (captured, captured < payload_size)
}

pub fn empty_event() -> RawBinderEvent {
    RawBinderEvent::default()
}
