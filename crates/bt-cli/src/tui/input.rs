use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum UiCommand {
    Quit,
    Space,
    Clear,
    NextPane,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RawFdSource {
    fd: RawFd,
}

impl RawFdSource {
    pub(super) const fn new(fd: RawFd) -> Self {
        Self { fd }
    }
}

impl AsRawFd for RawFdSource {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

pub(super) struct TtyInput {
    file: File,
    original_termios: libc::termios,
    parser: KeyParser,
}

impl TtyInput {
    pub(super) fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open("/dev/tty")?;
        let original_termios = get_termios(file.as_raw_fd())?;
        let mut raw_termios = original_termios;

        raw_termios.c_iflag &=
            !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw_termios.c_oflag &= !libc::OPOST;
        raw_termios.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw_termios.c_cc[libc::VMIN] = 0;
        raw_termios.c_cc[libc::VTIME] = 0;
        set_termios(file.as_raw_fd(), &raw_termios)?;

        Ok(Self {
            file,
            original_termios,
            parser: KeyParser::default(),
        })
    }

    pub(super) fn read_command(&mut self) -> io::Result<Option<UiCommand>> {
        while poll_fd(self.file.as_raw_fd())? {
            let mut bytes = [0_u8; 32];
            let read = match self.file.read(&mut bytes) {
                Ok(0) => return Ok(None),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };

            for byte in bytes.into_iter().take(read) {
                if let Some(command) = self.parser.push(byte) {
                    return Ok(Some(command));
                }
            }
        }

        Ok(None)
    }

    pub(super) fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Drop for TtyInput {
    fn drop(&mut self) {
        let _ = set_termios(self.file.as_raw_fd(), &self.original_termios);
    }
}

#[derive(Debug, Default)]
struct KeyParser {
    escape: Vec<u8>,
}

impl KeyParser {
    fn push(&mut self, byte: u8) -> Option<UiCommand> {
        if self.escape.is_empty() {
            return match byte {
                b'q' | 0x03 => Some(UiCommand::Quit),
                b' ' => Some(UiCommand::Space),
                b'c' => Some(UiCommand::Clear),
                b'\t' => Some(UiCommand::NextPane),
                0x1b => {
                    self.escape.push(byte);
                    None
                }
                _ => None,
            };
        }

        self.escape.push(byte);
        if let Some(command) = escape_to_command(&self.escape) {
            self.escape.clear();
            return Some(command);
        }
        if !is_escape_prefix(&self.escape) {
            self.escape.clear();
        }

        None
    }
}

const ESCAPE_COMMANDS: &[(&[u8], UiCommand)] = &[
    (b"\x1b[A", UiCommand::Up),
    (b"\x1b[B", UiCommand::Down),
    (b"\x1b[5~", UiCommand::PageUp),
    (b"\x1b[6~", UiCommand::PageDown),
    (b"\x1b[H", UiCommand::Home),
    (b"\x1b[1~", UiCommand::Home),
    (b"\x1bOH", UiCommand::Home),
    (b"\x1b[F", UiCommand::End),
    (b"\x1b[4~", UiCommand::End),
    (b"\x1bOF", UiCommand::End),
];

fn escape_to_command(sequence: &[u8]) -> Option<UiCommand> {
    ESCAPE_COMMANDS
        .iter()
        .find_map(|(candidate, command)| (*candidate == sequence).then_some(*command))
}

fn is_escape_prefix(sequence: &[u8]) -> bool {
    ESCAPE_COMMANDS
        .iter()
        .any(|(candidate, _)| candidate.starts_with(sequence))
}

fn get_termios(fd: RawFd) -> io::Result<libc::termios> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `termios` 指向未初始化但足够大的栈内存，`fd` 是当前进程打开的 tty fd。
    let ret = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if ret == 0 {
        // SAFETY: `tcgetattr` 成功后已经完整初始化 `termios`。
        Ok(unsafe { termios.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

fn set_termios(fd: RawFd, termios: &libc::termios) -> io::Result<()> {
    // SAFETY: `termios` 是有效引用，`fd` 是当前进程打开的 tty fd。
    let ret = unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn poll_fd(fd: RawFd) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        // SAFETY: `pollfd` 指向栈上有效内存，长度参数与实际元素数量一致。
        let ret = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if ret > 0 {
            return Ok((pollfd.revents & libc::POLLIN) != 0);
        }
        if ret == 0 {
            return Ok(false);
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
