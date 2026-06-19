//! Binder trace 会话二进制捕获历史。
//!
//! # 职责
//! - 把 socket 事件直接接收到 mmap 文件中的 `BinderEvent` 槽位。
//! - 按固定记录大小随机读取历史窗口，支撑 TUI、MCP 等前端按需加载旧事件。
//! - 用 `zerocopy` 维护事件结构和字节视图之间的转换，避免手写裸指针解析。

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::mem::size_of;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use memmap2::{MmapMut, MmapOptions};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::{BinderEvent, SocketIpcClient, SocketIpcError};

const MAGIC: [u8; 8] = *b"BTCEVT01";
const FORMAT_VERSION: u32 = 1;
const EVENT_ABI_VERSION: u32 = 3;
const HEADER_SIZE: usize = size_of::<CaptureFileHeader>();
const EVENT_SIZE: usize = size_of::<BinderEvent>();
const DEFAULT_INITIAL_EVENTS: u64 = 65_536;
const DEFAULT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

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
    CapacityLimit {
        needed_events: u64,
        max_events: u64,
        max_file_bytes: u64,
    },
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Socket(error) => write!(f, "{error}"),
            Self::CapacityLimit {
                needed_events,
                max_events,
                max_file_bytes,
            } => write!(
                f,
                "btcap 历史文件达到容量上限: 需要 {needed_events} 条事件，最多 {max_events} 条事件，最大文件大小 {max_file_bytes} 字节"
            ),
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

/// 当前会话的全量二进制事件历史。
pub struct CaptureHistory {
    file: File,
    mmap: MmapMut,
    path: PathBuf,
    _path_lock: Option<CapturePathLock>,
    count: u64,
    capacity: u64,
    max_events: u64,
    max_file_bytes: u64,
}

impl CaptureHistory {
    /// 默认 btcap 文件上限，避免异常流量把设备存储写满。
    pub const DEFAULT_MAX_FILE_BYTES: u64 = DEFAULT_MAX_FILE_BYTES;

    /// 创建一个新的历史文件；同名旧文件会被当前会话覆盖。
    pub fn create(path: PathBuf, initial_events: usize) -> Result<Self, HistoryError> {
        Self::create_with_max_file_bytes(path, initial_events, Self::DEFAULT_MAX_FILE_BYTES)
    }

    /// 使用指定文件大小上限创建新的历史文件。
    pub fn create_with_max_file_bytes(
        path: PathBuf,
        initial_events: usize,
        max_file_bytes: u64,
    ) -> Result<Self, HistoryError> {
        create_parent_dir(&path)?;
        let max_events = max_event_capacity(max_file_bytes)?;
        let capacity = (initial_events as u64)
            .max(DEFAULT_INITIAL_EVENTS)
            .min(max_events);
        Self::create_with_capacity(path, capacity, max_events, max_file_bytes)
    }

    fn create_with_capacity(
        path: PathBuf,
        capacity: u64,
        max_events: u64,
        max_file_bytes: u64,
    ) -> Result<Self, HistoryError> {
        let path_lock = CapturePathLock::prepare(&path)?;
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
            _path_lock: path_lock,
            count: 0,
            capacity,
            max_events,
            max_file_bytes,
        };
        *history.header_mut()? = CaptureFileHeader::new(capacity);
        if let Some(path_lock) = history._path_lock.as_mut() {
            path_lock.lock()?;
        }
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

    /// 返回当前文件可容纳的事件数量；满后会自动扩容。
    pub const fn capacity(&self) -> u64 {
        self.capacity
    }

    /// 返回当前会话允许写入的最大事件数量。
    pub const fn max_events(&self) -> u64 {
        self.max_events
    }

    /// 返回当前会话允许使用的最大 btcap 文件字节数。
    pub const fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
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

    /// 追加一条已经从 socket 读取出的事件。
    pub fn append_event(&mut self, event: BinderEvent) -> Result<u64, HistoryError> {
        self.ensure_capacity(self.count + 1)?;
        *self.event_slot_mut(self.count)? = event;
        self.commit_event()
    }

    /// 清空当前会话历史；文件容量保留，后续事件从下标 0 重新写入。
    pub fn clear(&mut self) -> Result<(), HistoryError> {
        self.count = 0;
        self.header_mut()?.count = 0;
        Ok(())
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
        if needed > self.max_events {
            return Err(HistoryError::CapacityLimit {
                needed_events: needed,
                max_events: self.max_events,
                max_file_bytes: self.max_file_bytes,
            });
        }

        let mut next_capacity = self.capacity.max(1);
        while next_capacity < needed {
            next_capacity = next_capacity.saturating_mul(2);
            next_capacity = next_capacity.min(self.max_events);
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

    /// 按历史下标顺序遍历当前会话中已经提交的事件。
    pub fn for_each_event(
        &self,
        mut visit: impl FnMut(u64, BinderEvent) -> Result<(), HistoryError>,
    ) -> Result<(), HistoryError> {
        for index in 0..self.count {
            let event = self.event_at(index).map_err(HistoryError::Io)?;
            visit(index, event)?;
        }

        Ok(())
    }

    #[cfg(test)]
    fn create_with_capacity_for_test(path: PathBuf, capacity: u64) -> Result<Self, HistoryError> {
        create_parent_dir(&path)?;
        let capacity = capacity.max(1);
        let max_events = capacity.saturating_mul(8);
        let max_file_bytes = file_len(max_events).map_err(HistoryError::Io)?;
        Self::create_with_capacity(path, capacity, max_events, max_file_bytes)
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

fn max_event_capacity(max_file_bytes: u64) -> Result<u64, HistoryError> {
    let event_bytes = EVENT_SIZE as u64;
    let Some(data_bytes) = max_file_bytes.checked_sub(HEADER_SIZE as u64) else {
        return Err(HistoryError::CapacityLimit {
            needed_events: 1,
            max_events: 0,
            max_file_bytes,
        });
    };
    let max_events = data_bytes / event_bytes;
    if max_events == 0 {
        return Err(HistoryError::CapacityLimit {
            needed_events: 1,
            max_events,
            max_file_bytes,
        });
    }

    Ok(max_events)
}

fn capacity_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "历史文件容量溢出")
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(unix)]
struct CapturePathLock {
    parent: PathBuf,
    restore_mode: u32,
    locked: bool,
}

#[cfg(unix)]
impl CapturePathLock {
    fn prepare(path: &Path) -> io::Result<Option<Self>> {
        const MODE_MASK: u32 = 0o7777;

        if !should_lock_default_capture_path(path) {
            return Ok(None);
        }

        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "历史文件缺少父目录"))?;
        let mode = fs::metadata(parent)?.permissions().mode() & MODE_MASK;
        let restore_mode = mode | 0o700;
        if restore_mode != mode {
            fs::set_permissions(parent, fs::Permissions::from_mode(restore_mode))?;
        }

        Ok(Some(Self {
            parent: parent.to_path_buf(),
            restore_mode,
            locked: false,
        }))
    }

    fn lock(&mut self) -> io::Result<()> {
        // unlink 权限来自父目录；普通文件锁挡不住 `rm events.btcap`。
        fs::set_permissions(
            &self.parent,
            fs::Permissions::from_mode(self.restore_mode & !0o222),
        )?;
        self.locked = true;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for CapturePathLock {
    fn drop(&mut self) {
        if self.locked {
            let _ =
                fs::set_permissions(&self.parent, fs::Permissions::from_mode(self.restore_mode));
        }
    }
}

#[cfg(unix)]
fn should_lock_default_capture_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "events.btcap")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "binder-trace")
}

#[cfg(not(unix))]
struct CapturePathLock;

#[cfg(not(unix))]
impl CapturePathLock {
    fn prepare(_path: &Path) -> io::Result<Option<Self>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{CaptureHistory, EVENT_SIZE, HEADER_SIZE, HistoryError};
    use crate::BinderEvent;

    #[test]
    fn appends_and_loads_history_window() {
        let path = temp_path("window");
        let mut history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");

        for sequence in 0..5 {
            history
                .append_event(test_event(sequence))
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
                .append_event(test_event(sequence))
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
            .append_event(test_event(1))
            .expect("测试事件应可追加");

        let metadata = fs::metadata(&path).expect("历史文件元数据应可读取");
        assert!(metadata.len() >= (HEADER_SIZE + EVENT_SIZE) as u64);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_append_after_max_file_bytes() {
        let path = temp_path("capacity-limit");
        let max_file_bytes = HEADER_SIZE as u64 + EVENT_SIZE as u64 * 2;
        let mut history =
            CaptureHistory::create_with_max_file_bytes(path.clone(), 1, max_file_bytes)
                .expect("历史文件应可按指定上限创建");

        history
            .append_event(test_event(1))
            .expect("第一条测试事件应可追加");
        history
            .append_event(test_event(2))
            .expect("第二条测试事件应可追加");
        let error = history
            .append_event(test_event(3))
            .expect_err("超过文件上限时应拒绝追加");

        assert!(matches!(
            error,
            HistoryError::CapacityLimit {
                needed_events: 3,
                max_events: 2,
                max_file_bytes: actual_max_file_bytes,
            } if actual_max_file_bytes == max_file_bytes
        ));

        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn locks_default_events_parent_until_history_is_dropped() {
        let root = temp_path("protected-root");
        let path = root.join("binder-trace/events.btcap");
        let mut history = CaptureHistory::create(path.clone(), 1).expect("历史文件应可创建");

        history
            .append_event(test_event(1))
            .expect("受保护历史文件仍应可追加");

        let parent = path.parent().expect("历史文件应有父目录").to_path_buf();
        let locked_mode = fs::metadata(&parent)
            .expect("父目录元数据应可读取")
            .permissions()
            .mode();
        assert_eq!(locked_mode & 0o222, 0);

        drop(history);

        let restored_mode = fs::metadata(&parent)
            .expect("父目录元数据应可读取")
            .permissions()
            .mode();
        assert_ne!(restored_mode & 0o200, 0);

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(parent);
        let _ = fs::remove_dir(root);
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
            transaction_debug_id: 0,
            reply_to_debug_id: 0,
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
