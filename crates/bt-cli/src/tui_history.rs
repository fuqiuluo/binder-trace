//! TUI 使用的二进制捕获历史。
//!
//! # 职责
//! - 把 socket 事件直接接收到 mmap 文件中的 `BinderEvent` 槽位。
//! - 按固定记录大小随机读取历史窗口，支撑 TUI 向上滚动加载旧事件。
//! - 用 `zerocopy` 维护事件结构和字节视图之间的转换，避免手写裸指针解析。

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use bt_agent::{BinderEvent, SocketIpcClient, SocketIpcError};
use memmap2::{MmapMut, MmapOptions};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const MAGIC: [u8; 8] = *b"BTCEVT01";
const FORMAT_VERSION: u32 = 1;
const EVENT_ABI_VERSION: u32 = 2;
const HEADER_SIZE: usize = size_of::<CaptureFileHeader>();
const EVENT_SIZE: usize = size_of::<BinderEvent>();
const DEFAULT_INITIAL_EVENTS: u64 = 65_536;

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
struct CaptureFileHeader {
    magic: [u8; 8],
    format_version: u32,
    header_size: u32,
    event_size: u32,
    event_abi_version: u32,
    count: u64,
    capacity: u64,
    reserved: [u8; 24],
}

impl CaptureFileHeader {
    const fn new(capacity: u64) -> Self {
        Self {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            header_size: HEADER_SIZE as u32,
            event_size: EVENT_SIZE as u32,
            event_abi_version: EVENT_ABI_VERSION,
            count: 0,
            capacity,
            reserved: [0; 24],
        }
    }
}

/// mmap 历史文件读写错误。
#[derive(Debug)]
pub enum HistoryError {
    Io(io::Error),
    Socket(SocketIpcError),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Socket(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<io::Error> for HistoryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SocketIpcError> for HistoryError {
    fn from(error: SocketIpcError) -> Self {
        Self::Socket(error)
    }
}

/// 当前 TUI 会话的全量二进制事件历史。
pub struct CaptureHistory {
    file: File,
    mmap: MmapMut,
    path: PathBuf,
    count: u64,
    capacity: u64,
}

impl CaptureHistory {
    /// 创建一个新的历史文件；同名旧文件会被当前会话覆盖。
    pub fn create(path: PathBuf, initial_events: usize) -> Result<Self, HistoryError> {
        create_parent_dir(&path)?;
        let capacity = (initial_events as u64).max(DEFAULT_INITIAL_EVENTS);
        Self::create_with_capacity(path, capacity)
    }

    fn create_with_capacity(path: PathBuf, capacity: u64) -> Result<Self, HistoryError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(file_len(capacity)?)?;

        let mut history = Self {
            mmap: map_file(&file)?,
            file,
            path,
            count: 0,
            capacity,
        };
        *history.header_mut()? = CaptureFileHeader::new(capacity);
        Ok(history)
    }

    pub fn default_path() -> PathBuf {
        let android_tmp = Path::new("/data/local/tmp");
        if android_tmp.is_dir() {
            android_tmp.join("binder-trace/events.btcap")
        } else {
            PathBuf::from("binder-trace.btcap")
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn event_count(&self) -> u64 {
        self.count
    }

    /// 从 socket 读取事件，匹配后直接写入 mmap 文件中的下一个槽位。
    ///
    /// 不匹配的事件会被读取并跳过，但不会提交到历史文件；返回的下标只属于已提交
    /// 的事件序列。
    pub fn recv_next_matching(
        &mut self,
        client: &SocketIpcClient,
        mut accept: impl FnMut(&BinderEvent) -> bool,
    ) -> Result<Option<u64>, HistoryError> {
        loop {
            self.ensure_capacity(self.count + 1)?;
            let should_commit = {
                let slot = self.event_slot_mut(self.count)?;
                if !client.try_recv_event_into(slot)? {
                    return Ok(None);
                }
                accept(slot)
            };
            if should_commit {
                return self.commit_event().map(Some);
            }
        }
    }

    /// 从历史文件读取一个显示窗口。
    #[cfg(test)]
    pub fn load_window(&self, start: u64, limit: usize) -> Result<Vec<BinderEvent>, HistoryError> {
        let start = start.min(self.count);
        let count = self.count.saturating_sub(start).min(limit as u64) as usize;
        let mut events = Vec::with_capacity(count);

        for offset in 0..count {
            events.push(self.event_at(start + offset as u64)?);
        }

        Ok(events)
    }

    pub fn flush_async(&self) -> io::Result<()> {
        self.mmap.flush_async_range(0, HEADER_SIZE)
    }

    fn ensure_capacity(&mut self, needed: u64) -> Result<(), HistoryError> {
        if needed <= self.capacity {
            return Ok(());
        }

        let mut next_capacity = self.capacity.max(1);
        while next_capacity < needed {
            next_capacity = next_capacity.checked_mul(2).ok_or_else(capacity_error)?;
        }

        self.mmap.flush_async()?;
        self.file.set_len(file_len(next_capacity)?)?;
        self.mmap = map_file(&self.file)?;
        self.capacity = next_capacity;

        let count = self.count;
        let header = self.header_mut()?;
        header.capacity = next_capacity;
        header.count = count;
        Ok(())
    }

    fn commit_event(&mut self) -> Result<u64, HistoryError> {
        let index = self.count;
        self.count = self.count.checked_add(1).ok_or_else(capacity_error)?;
        self.header_mut()?.count = self.count;
        Ok(index)
    }

    fn header_mut(&mut self) -> io::Result<&mut CaptureFileHeader> {
        CaptureFileHeader::mut_from_bytes(&mut self.mmap[..HEADER_SIZE])
            .map_err(|_| invalid_data("历史文件头布局不合法"))
    }

    fn event_slot_mut(&mut self, index: u64) -> io::Result<&mut BinderEvent> {
        let range = event_range(index)?;
        BinderEvent::mut_from_bytes(&mut self.mmap[range])
            .map_err(|_| invalid_data("历史事件槽位布局不合法"))
    }

    pub fn event_at(&self, index: u64) -> io::Result<BinderEvent> {
        let range = event_range(index)?;
        BinderEvent::ref_from_bytes(&self.mmap[range])
            .copied()
            .map_err(|_| invalid_data("历史事件记录布局不合法"))
    }

    #[cfg(test)]
    pub(crate) fn append_for_test(&mut self, event: BinderEvent) -> Result<u64, HistoryError> {
        self.ensure_capacity(self.count + 1)?;
        *self.event_slot_mut(self.count)? = event;
        self.commit_event()
    }

    #[cfg(test)]
    fn create_with_capacity_for_test(path: PathBuf, capacity: u64) -> Result<Self, HistoryError> {
        create_parent_dir(&path)?;
        Self::create_with_capacity(path, capacity.max(1))
    }
}

impl Drop for CaptureHistory {
    fn drop(&mut self) {
        let _ = self.mmap.flush_async();
    }
}

fn create_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

fn map_file(file: &File) -> io::Result<MmapMut> {
    // SAFETY: 文件长度由 `file_len(capacity)` 设置，映射生命周期受 `CaptureHistory` 管理。
    unsafe { MmapOptions::new().map_mut(file) }
}

fn event_range(index: u64) -> io::Result<std::ops::Range<usize>> {
    let offset = (HEADER_SIZE as u64)
        .checked_add(
            index
                .checked_mul(EVENT_SIZE as u64)
                .ok_or_else(capacity_error)?,
        )
        .ok_or_else(capacity_error)?;
    let end = offset
        .checked_add(EVENT_SIZE as u64)
        .ok_or_else(capacity_error)?;

    Ok(offset as usize..end as usize)
}

fn file_len(capacity: u64) -> io::Result<u64> {
    (HEADER_SIZE as u64)
        .checked_add(
            capacity
                .checked_mul(EVENT_SIZE as u64)
                .ok_or_else(capacity_error)?,
        )
        .ok_or_else(capacity_error)
}

fn capacity_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "历史文件容量溢出")
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use bt_agent::BinderEvent;

    use super::{CaptureHistory, EVENT_SIZE, HEADER_SIZE};

    #[test]
    fn appends_and_loads_history_window() {
        let path = temp_path("window");
        let mut history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");

        for sequence in 0..5 {
            history
                .append_for_test(test_event(sequence))
                .expect("测试事件应可追加");
        }

        let events = history.load_window(2, 2).expect("历史窗口应可读取");
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(history.event_count(), 5);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn keeps_order_after_capacity_growth() {
        let path = temp_path("growth");
        let mut history = CaptureHistory::create_with_capacity_for_test(path.clone(), 2)
            .expect("历史文件应可创建");

        for sequence in 0..9 {
            history
                .append_for_test(test_event(sequence))
                .expect("测试事件应可追加");
        }

        assert!(history.capacity >= 9);
        assert_eq!(
            history
                .load_window(0, 9)
                .expect("完整历史窗口应可读取")
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert_eq!(
            history
                .load_window(4, 3)
                .expect("中间历史窗口应可读取")
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![4, 5, 6]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn creates_fixed_record_file() {
        let path = temp_path("layout");
        let mut history = CaptureHistory::create(path.clone(), 1).expect("历史文件应可创建");
        history
            .append_for_test(test_event(1))
            .expect("测试事件应可追加");

        let metadata = fs::metadata(&path).expect("历史文件元数据应可读取");
        assert!(metadata.len() >= (HEADER_SIZE + EVENT_SIZE) as u64);

        let _ = fs::remove_file(path);
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "binder-trace-{name}-{}-{nanos}.btcap",
            std::process::id()
        ))
    }

    fn test_event(sequence: u64) -> BinderEvent {
        BinderEvent {
            sequence,
            timestamp_ns: 0,
            kind: 1,
            pid: 0,
            tgid: 0,
            uid: 0,
            reply: 0,
            lost_before: 0,
            transaction: 0,
            proc: 0,
            thread: 0,
            extra_buffers_size: 0,
            code: sequence as u32,
            flags: 0,
            data_size: 0,
            offsets_size: 0,
            target_handle: 0,
            sender_pid: 0,
            sender_euid: 0,
            payload_len: 0,
            payload_truncated: 0,
            reserved: [0; 7],
            payload: [0; 256],
        }
    }
}
