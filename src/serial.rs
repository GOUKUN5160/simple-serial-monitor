use crate::autodetect::{
    detect_7bit_parity, score_serial_sample, LineEndingDetector, CANDIDATE_BAUDS,
    CANDIDATE_FRAMINGS,
};
use crate::i18n::tr;
use crate::process_lock;
use crate::types::{
    bytes_to_hex_compact, now_ms_and_local_datetime, ConnectionState, LineEnding, LogEntry,
    LogKind, Parity, SerialConfig, StopBits,
};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

const MAX_LOG_ENTRIES: usize = 8000;

#[derive(Debug)]
enum PortCommand {
    Connect,
    Disconnect,
    UpdateConfig(SerialConfig),
    TriggerAutoDetect,
    Send { data: Vec<u8> },
    KillOwnerAndConnect,
    DeviceChanged,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct PortSnapshot {
    pub port: String,
    pub state: ConnectionState,
    pub config: SerialConfig,
    pub detected_eol: LineEnding,
    pub auto_locked: bool,
    pub latest_seq: u64,
    pub log_file: String,
}

struct SessionInner {
    state: ConnectionState,
    config: SerialConfig,
    detected_eol: LineEnding,
    auto_locked: bool,
    is_usb_cdc: bool,
    logs: VecDeque<LogEntry>,
    next_seq: u64,
    cmd_queue: VecDeque<PortCommand>,
    log_file_path: PathBuf,
    log_file: Option<File>,
}

pub struct PortSession {
    pub port_name: String,
    inner: Mutex<SessionInner>,
    wake_worker: Condvar,
    wake_subscribers: Condvar,
    running: AtomicBool,
}

impl PortSession {
    pub fn start(
        port_name: String,
        initial_config: SerialConfig,
        is_usb_cdc: bool,
        auto_connect: bool,
        log_dir: PathBuf,
    ) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&log_dir);
        let trimmed_dev = port_name.trim_start_matches("/dev/");
        let safe_name = trimmed_dev.replace(['\\', '/', ':', '*'], "_");
        let log_file_path = log_dir.join(format!("{}.log", safe_name));
        let log_file = open_shared_log_file(&log_file_path);

        let mut cmd_queue = VecDeque::new();
        if auto_connect {
            cmd_queue.push_back(PortCommand::Connect);
        }

        let session = Arc::new(Self {
            port_name: port_name.clone(),
            inner: Mutex::new(SessionInner {
                state: if auto_connect {
                    ConnectionState::Connecting
                } else {
                    ConnectionState::Released
                },
                config: initial_config,
                detected_eol: LineEnding::None,
                auto_locked: false,
                is_usb_cdc,
                logs: VecDeque::with_capacity(512),
                next_seq: 1,
                cmd_queue,
                log_file_path,
                log_file,
            }),
            wake_worker: Condvar::new(),
            wake_subscribers: Condvar::new(),
            running: AtomicBool::new(true),
        });

        let worker_session = Arc::clone(&session);
        let _ = thread::Builder::new()
            .name(format!("ser-{}", port_name))
            .spawn(move || worker_loop(worker_session));

        session
    }

    pub fn set_is_usb_cdc(&self, is_cdc: bool) {
        let mut g = self.inner.lock().unwrap();
        g.is_usb_cdc = is_cdc;
    }

    pub fn snapshot(&self) -> PortSnapshot {
        let g = self.inner.lock().unwrap();
        PortSnapshot {
            port: self.port_name.clone(),
            state: g.state.clone(),
            config: g.config,
            detected_eol: g.detected_eol,
            auto_locked: g.auto_locked,
            latest_seq: g.next_seq.saturating_sub(1),
            log_file: g.log_file_path.to_string_lossy().to_string(),
        }
    }

    pub fn connect(&self) {
        self.push_cmd(PortCommand::Connect);
    }

    pub fn disconnect(&self) {
        self.push_cmd(PortCommand::Disconnect);
    }

    pub fn update_config(&self, cfg: SerialConfig) {
        self.push_cmd(PortCommand::UpdateConfig(cfg));
    }

    pub fn trigger_autodetect(&self) {
        self.push_cmd(PortCommand::TriggerAutoDetect);
    }

    pub fn kill_owner_and_connect(&self) {
        self.push_cmd(PortCommand::KillOwnerAndConnect);
    }

    pub fn notify_device_changed(&self) {
        self.push_cmd(PortCommand::DeviceChanged);
    }

    pub fn send_text(&self, text: &str, eol_override: Option<LineEnding>) {
        let (tx_eol, detected_eol) = {
            let g = self.inner.lock().unwrap();
            (eol_override.unwrap_or(g.config.tx_eol), g.detected_eol)
        };
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(tx_eol.bytes(detected_eol));
        self.push_log_raw(LogKind::Tx, text.to_string(), bytes.clone());
        self.push_cmd(PortCommand::Send { data: bytes });
    }

    pub fn clear_logs(&self) {
        let mut g = self.inner.lock().unwrap();
        g.logs.clear();
        // Truncate live log file as well
        g.log_file = None;
        g.log_file = truncate_shared_log_file(&g.log_file_path);
        self.wake_subscribers.notify_all();
    }

    pub fn read_logs(&self, since_seq: u64, tail: usize) -> Vec<LogEntry> {
        let g = self.inner.lock().unwrap();
        let filtered: Vec<LogEntry> = g
            .logs
            .iter()
            .filter(|e| e.seq > since_seq)
            .cloned()
            .collect();
        if tail > 0 && filtered.len() > tail {
            filtered[filtered.len() - tail..].to_vec()
        } else {
            filtered
        }
    }

    /// Wait until new entries with seq > since_seq arrive or timeout elapses.
    pub fn wait_logs_since(&self, since_seq: u64, timeout: Duration) -> Vec<LogEntry> {
        let mut g = self.inner.lock().unwrap();
        if !g.logs.iter().any(|e| e.seq > since_seq) {
            let (guard, _) = self.wake_subscribers.wait_timeout(g, timeout).unwrap();
            g = guard;
        }
        g.logs
            .iter()
            .filter(|e| e.seq > since_seq)
            .cloned()
            .collect()
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.push_cmd(PortCommand::Shutdown);
        self.wake_subscribers.notify_all();
    }

    fn push_cmd(&self, cmd: PortCommand) {
        let mut g = self.inner.lock().unwrap();
        g.cmd_queue.push_back(cmd);
        self.wake_worker.notify_one();
    }

    fn set_state(&self, new_state: ConnectionState) {
        let mut g = self.inner.lock().unwrap();
        if g.state != new_state {
            g.state = new_state;
            self.wake_subscribers.notify_all();
        }
    }

    fn push_log(&self, kind: LogKind, text: String) {
        self.push_log_raw(kind, text, Vec::new());
    }

    fn push_log_raw(&self, kind: LogKind, text: String, raw_bytes: Vec<u8>) {
        let (timestamp_ms, datetime_str, time_str) = now_ms_and_local_datetime();
        let mut g = self.inner.lock().unwrap();
        let seq = g.next_seq;
        g.next_seq += 1;
        let is_binary_rx = kind == LogKind::Rx && looks_binary(&raw_bytes);
        let entry = LogEntry {
            seq,
            timestamp_ms,
            datetime_str: datetime_str.clone(),
            time_str,
            kind,
            text: text.clone(),
            raw_bytes: raw_bytes.clone(),
        };
        if g.logs.len() >= MAX_LOG_ENTRIES {
            g.logs.pop_front();
        }
        g.logs.push_back(entry);

        if let Some(ref mut f) = g.log_file {
            let prefix = match kind {
                LogKind::Rx => "",
                LogKind::Tx => "[TX] ",
                LogKind::SysConnect => "[SYS/CONNECT] ",
                LogKind::SysDisconnect => "[SYS/DISCONNECT] ",
                LogKind::SysError => "[SYS/ERROR] ",
                LogKind::SysInfo => "[SYS/INFO] ",
            };
            if is_binary_rx && !raw_bytes.is_empty() {
                let _ = writeln!(
                    f,
                    "[{}] [BIN] {}  | HEX: {}",
                    datetime_str,
                    text,
                    bytes_to_hex_compact(&raw_bytes)
                );
            } else {
                let _ = writeln!(f, "[{}] {}{}", datetime_str, prefix, text);
            }
            let _ = f.flush();
        }
        self.wake_subscribers.notify_all();
    }
}

fn open_shared_log_file(path: &PathBuf) -> Option<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(windows)]
    {
        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    opts.open(path).ok()
}

fn truncate_shared_log_file(path: &PathBuf) -> Option<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(windows)]
    {
        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    opts.open(path).ok()
}

#[cfg(windows)]
mod port_io {
    use crate::types::{Parity, SerialConfig};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Devices::Communication::{
        ClearCommError, GetCommState, PurgeComm, SetCommState, SetCommTimeouts, SetupComm,
        COMMTIMEOUTS, COMSTAT, DCB, PURGE_RXABORT, PURGE_RXCLEAR, PURGE_TXABORT, PURGE_TXCLEAR,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_IO_PENDING, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

    pub fn is_access_denied(err: u32) -> bool {
        err == ERROR_ACCESS_DENIED
    }

    pub fn is_not_found(err: u32) -> bool {
        err == 2 || err == 3 || err == 1167
    }

    pub struct PlatformSerialPort {
        handle: HANDLE,
        read_event: HANDLE,
        write_event: HANDLE,
    }

    impl PlatformSerialPort {
        pub fn open(port_name: &str) -> Result<Self, u32> {
            let dev_path = format!("\\\\.\\{}\0", port_name.trim_start_matches("\\\\.\\"));
            let wide: Vec<u16> = dev_path.encode_utf16().collect();
            unsafe {
                let handle = CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    null_mut(),
                );
                if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                    return Err(GetLastError());
                }

                let read_event = CreateEventW(null(), 1, 0, null());
                let write_event = CreateEventW(null(), 1, 0, null());
                if read_event.is_null() || write_event.is_null() {
                    if !read_event.is_null() {
                        CloseHandle(read_event);
                    }
                    if !write_event.is_null() {
                        CloseHandle(write_event);
                    }
                    CloseHandle(handle);
                    return Err(GetLastError());
                }

                SetupComm(handle, 16384, 16384);

                let timeouts = COMMTIMEOUTS {
                    ReadIntervalTimeout: u32::MAX,
                    ReadTotalTimeoutMultiplier: u32::MAX,
                    ReadTotalTimeoutConstant: 35,
                    WriteTotalTimeoutMultiplier: 0,
                    WriteTotalTimeoutConstant: 1000,
                };
                SetCommTimeouts(handle, &timeouts);

                Ok(Self {
                    handle,
                    read_event,
                    write_event,
                })
            }
        }

        pub fn apply_config(&self, cfg: SerialConfig, is_usb_cdc: bool) -> Result<(), u32> {
            unsafe {
                let mut dcb: DCB = std::mem::zeroed();
                dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
                if GetCommState(self.handle, &mut dcb) == 0 {
                    return Err(GetLastError());
                }
                dcb.BaudRate = cfg.baud;
                dcb.ByteSize = cfg.data_bits;
                dcb.Parity = cfg.parity.win32_val();
                dcb.StopBits = cfg.stop_bits.win32_val();

                let mut flags: u32 = 1; // fBinary
                if cfg.parity != Parity::None {
                    flags |= 1 << 1; // fParity
                }
                if is_usb_cdc {
                    flags |= 1 << 4; // DTR_CONTROL_ENABLE
                    flags |= 1 << 12; // RTS_CONTROL_ENABLE
                }
                dcb._bitfield = flags;

                if SetCommState(self.handle, &dcb) == 0 {
                    return Err(GetLastError());
                }
                Ok(())
            }
        }

        pub fn purge(&self) {
            unsafe {
                PurgeComm(
                    self.handle,
                    PURGE_RXCLEAR | PURGE_TXCLEAR | PURGE_RXABORT | PURGE_TXABORT,
                );
                let mut errs = 0u32;
                let mut stat: COMSTAT = std::mem::zeroed();
                ClearCommError(self.handle, &mut errs, &mut stat);
            }
        }

        pub fn read_chunk(&self, buf: &mut [u8], wait_ms: u32) -> Result<(usize, u32), u32> {
            unsafe {
                let mut pre_errs = 0u32;
                let mut pre_stat: COMSTAT = std::mem::zeroed();
                if ClearCommError(self.handle, &mut pre_errs, &mut pre_stat) == 0 {
                    return Err(GetLastError());
                }

                let req_len = if pre_stat.cbInQue > 0 {
                    (pre_stat.cbInQue as usize).min(buf.len()) as u32
                } else {
                    buf.len().min(512) as u32
                };

                ResetEvent(self.read_event);
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = self.read_event;
                let mut bytes_read = 0u32;

                let ok = ReadFile(
                    self.handle,
                    buf.as_mut_ptr(),
                    req_len,
                    &mut bytes_read,
                    &mut ov,
                );
                if ok == 0 {
                    let err = GetLastError();
                    if err != ERROR_IO_PENDING {
                        return Err(err);
                    }
                    let wait = WaitForSingleObject(self.read_event, wait_ms.max(80));
                    if wait == WAIT_OBJECT_0 {
                        if GetOverlappedResult(self.handle, &ov, &mut bytes_read, 0) == 0 {
                            return Err(GetLastError());
                        }
                    } else {
                        CancelIoEx(self.handle, &ov);
                        let mut tmp = 0u32;
                        let _ = GetOverlappedResult(self.handle, &ov, &mut tmp, 1);
                        bytes_read = tmp;
                    }
                }

                let mut comm_errs = 0u32;
                let mut stat: COMSTAT = std::mem::zeroed();
                if ClearCommError(self.handle, &mut comm_errs, &mut stat) == 0 {
                    return Err(GetLastError());
                }
                Ok((bytes_read as usize, pre_errs | comm_errs))
            }
        }

        pub fn write_all(&self, data: &[u8]) -> Result<(), u32> {
            if data.is_empty() {
                return Ok(());
            }
            unsafe {
                ResetEvent(self.write_event);
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = self.write_event;
                let mut written = 0u32;
                let ok = WriteFile(
                    self.handle,
                    data.as_ptr(),
                    data.len() as u32,
                    &mut written,
                    &mut ov,
                );
                if ok == 0 {
                    let err = GetLastError();
                    if err != ERROR_IO_PENDING {
                        return Err(err);
                    }
                    let wait = WaitForSingleObject(self.write_event, 1500);
                    if wait != WAIT_OBJECT_0 {
                        CancelIoEx(self.handle, &ov);
                        return Err(1460); // ERROR_TIMEOUT
                    }
                    if GetOverlappedResult(self.handle, &ov, &mut written, 0) == 0 {
                        return Err(GetLastError());
                    }
                }
                Ok(())
            }
        }
    }

    impl Drop for PlatformSerialPort {
        fn drop(&mut self) {
            unsafe {
                CancelIoEx(self.handle, null());
                CloseHandle(self.read_event);
                CloseHandle(self.write_event);
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod port_io {
    use crate::types::{Parity, SerialConfig, StopBits};
    use std::ffi::{c_char, c_int, c_ulong, c_void, CString};

    const O_RDWR: c_int = 0x0002;
    const O_NONBLOCK: c_int = 0x0004;
    const O_NOCTTY: c_int = 0x20000;

    const TCSANOW: c_int = 0;
    const TCIOFLUSH: c_int = 3;

    const CSIZE: u64 = 0x0000_0300;
    const CS5: u64 = 0x0000_0000;
    const CS6: u64 = 0x0000_0100;
    const CS7: u64 = 0x0000_0200;
    const CS8: u64 = 0x0000_0300;
    const CSTOPB: u64 = 0x0000_0400;
    const CREAD: u64 = 0x0000_0800;
    const PARENB: u64 = 0x0000_1000;
    const PARODD: u64 = 0x0000_2000;
    const HUPCL: u64 = 0x0000_4000;
    const CLOCAL: u64 = 0x0000_8000;

    const TIOCEXCL: c_ulong = 0x2000_740d;
    const TIOCNXCL: c_ulong = 0x2000_740e;
    const TIOCMGET: c_ulong = 0x4004_746a;
    const TIOCMSET: c_ulong = 0x8004_746d;
    const TIOCM_DTR: c_int = 0x0002;
    const TIOCM_RTS: c_int = 0x0004;
    const IOSSIOSPEED: c_ulong = 0x8008_5402;

    const POLLIN: i16 = 0x0001;
    const POLLOUT: i16 = 0x0004;
    const POLLERR: i16 = 0x0008;
    const POLLHUP: i16 = 0x0010;
    const POLLNVAL: i16 = 0x0020;

    const EACCES: u32 = 13;
    const EBUSY: u32 = 16;
    const ENOENT: u32 = 2;
    const ENXIO: u32 = 6;
    const EAGAIN: i32 = 35;
    const EINTR: i32 = 4;

    #[repr(C)]
    struct Termios {
        c_iflag: u64,
        c_oflag: u64,
        c_cflag: u64,
        c_lflag: u64,
        c_cc: [u8; 20],
        c_ispeed: u64,
        c_ospeed: u64,
    }

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    extern "C" {
        fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
        fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        fn poll(fds: *mut PollFd, nfds: u32, timeout: c_int) -> c_int;
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn tcgetattr(fd: c_int, termios_p: *mut Termios) -> c_int;
        fn tcsetattr(fd: c_int, optional_actions: c_int, termios_p: *const Termios) -> c_int;
        fn tcflush(fd: c_int, queue_selector: c_int) -> c_int;
        fn cfsetspeed(termios_p: *mut Termios, speed: u64) -> c_int;
        fn __error() -> *mut c_int;
    }

    fn last_errno() -> u32 {
        unsafe { (*__error()).max(0) as u32 }
    }

    pub fn is_access_denied(err: u32) -> bool {
        err == EBUSY || err == EACCES
    }

    pub fn is_not_found(err: u32) -> bool {
        err == ENOENT || err == ENXIO
    }

    pub struct PlatformSerialPort {
        fd: c_int,
    }

    impl PlatformSerialPort {
        pub fn open(port_name: &str) -> Result<Self, u32> {
            let cpath = CString::new(port_name).map_err(|_| ENOENT)?;
            unsafe {
                let fd = open(cpath.as_ptr(), O_RDWR | O_NOCTTY | O_NONBLOCK);
                if fd < 0 {
                    return Err(last_errno());
                }
                // Request exclusive access so conflicts with screen / esptool / Arduino IDE are detected
                if ioctl(fd, TIOCEXCL) < 0 {
                    let err = last_errno();
                    close(fd);
                    return Err(if err == 0 { EBUSY } else { err });
                }
                Ok(Self { fd })
            }
        }

        pub fn apply_config(&self, cfg: SerialConfig, is_usb_cdc: bool) -> Result<(), u32> {
            unsafe {
                let mut tio: Termios = std::mem::zeroed();
                if tcgetattr(self.fd, &mut tio) != 0 {
                    return Err(last_errno());
                }

                // Raw mode
                tio.c_iflag = 0;
                tio.c_oflag = 0;
                tio.c_lflag = 0;
                tio.c_cflag &= !(CSIZE | PARENB | PARODD | CSTOPB | HUPCL);
                tio.c_cflag |= CREAD | CLOCAL;

                tio.c_cflag |= match cfg.data_bits {
                    5 => CS5,
                    6 => CS6,
                    7 => CS7,
                    _ => CS8,
                };

                match cfg.parity {
                    Parity::None => {}
                    Parity::Odd => tio.c_cflag |= PARENB | PARODD,
                    Parity::Even => tio.c_cflag |= PARENB,
                    Parity::Mark | Parity::Space => {}
                }

                if matches!(cfg.stop_bits, StopBits::Two) {
                    tio.c_cflag |= CSTOPB;
                }

                // Non-blocking read behavior
                tio.c_cc[16] = 0; // VMIN
                tio.c_cc[17] = 0; // VTIME

                // Set a standard base speed first (9600), then apply exact speed via IOSSIOSPEED
                let _ = cfsetspeed(&mut tio, 9600);
                if tcsetattr(self.fd, TCSANOW, &tio) != 0 {
                    return Err(last_errno());
                }

                let speed: c_ulong = cfg.baud as c_ulong;
                if ioctl(self.fd, IOSSIOSPEED, &speed) != 0 {
                    // Fallback: try standard cfsetspeed if driver doesn't support IOSSIOSPEED (e.g. PTY)
                    let _ = cfsetspeed(&mut tio, cfg.baud as u64);
                    let _ = tcsetattr(self.fd, TCSANOW, &tio);
                }

                // Configure DTR/RTS modem lines
                let mut status: c_int = 0;
                if ioctl(self.fd, TIOCMGET, &mut status) == 0 {
                    if is_usb_cdc {
                        status |= TIOCM_DTR | TIOCM_RTS;
                    } else {
                        status &= !(TIOCM_DTR | TIOCM_RTS);
                    }
                    let _ = ioctl(self.fd, TIOCMSET, &status);
                }

                Ok(())
            }
        }

        pub fn purge(&self) {
            unsafe {
                let _ = tcflush(self.fd, TCIOFLUSH);
            }
        }

        pub fn read_chunk(&self, buf: &mut [u8], wait_ms: u32) -> Result<(usize, u32), u32> {
            unsafe {
                let mut pfd = PollFd {
                    fd: self.fd,
                    events: POLLIN,
                    revents: 0,
                };
                let ret = poll(&mut pfd, 1, wait_ms as c_int);
                if ret < 0 {
                    let err = last_errno() as i32;
                    if err == EINTR {
                        return Ok((0, 0));
                    }
                    return Err(err as u32);
                }
                if ret == 0 {
                    return Ok((0, 0));
                }
                if (pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) != 0 {
                    return Err(ENXIO);
                }
                if (pfd.revents & POLLIN) != 0 {
                    let n = read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len());
                    if n > 0 {
                        return Ok((n as usize, 0));
                    } else if n == 0 {
                        // EOF on serial device indicates USB unplug / hangup
                        return Err(ENXIO);
                    } else {
                        let err = last_errno() as i32;
                        if err == EAGAIN || err == EINTR {
                            return Ok((0, 0));
                        }
                        return Err(err as u32);
                    }
                }
                Ok((0, 0))
            }
        }

        pub fn write_all(&self, data: &[u8]) -> Result<(), u32> {
            if data.is_empty() {
                return Ok(());
            }
            let mut offset = 0usize;
            while offset < data.len() {
                unsafe {
                    let mut pfd = PollFd {
                        fd: self.fd,
                        events: POLLOUT,
                        revents: 0,
                    };
                    let ret = poll(&mut pfd, 1, 1500);
                    if ret < 0 {
                        let err = last_errno() as i32;
                        if err == EINTR {
                            continue;
                        }
                        return Err(err as u32);
                    }
                    if ret == 0 {
                        return Err(60); // ETIMEDOUT
                    }
                    if (pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) != 0 {
                        return Err(ENXIO);
                    }
                    let n = write(
                        self.fd,
                        data[offset..].as_ptr() as *const c_void,
                        data.len() - offset,
                    );
                    if n > 0 {
                        offset += n as usize;
                    } else if n < 0 {
                        let err = last_errno() as i32;
                        if err == EAGAIN || err == EINTR {
                            continue;
                        }
                        return Err(err as u32);
                    }
                }
            }
            Ok(())
        }
    }

    impl Drop for PlatformSerialPort {
        fn drop(&mut self) {
            unsafe {
                let _ = ioctl(self.fd, TIOCNXCL);
                close(self.fd);
            }
        }
    }
}

use port_io::{is_access_denied, is_not_found, PlatformSerialPort};

/// Detects whether a raw byte slice looks like binary/custom protocol data rather than plain text.
fn looks_binary(raw: &[u8]) -> bool {
    if raw.is_empty() {
        return false;
    }
    let mut non_text = 0usize;
    for &b in raw {
        if b == 0x00 || (b < 0x20 && !matches!(b, b'\r' | b'\n' | b'\t' | 0x1B)) || b == 0x7F {
            non_text += 1;
        }
    }
    if non_text * 5 >= raw.len() {
        return true;
    }
    // Also check if UTF-8 decoding fails with replacement characters on >25% of bytes
    if std::str::from_utf8(raw).is_err() {
        let s = String::from_utf8_lossy(raw);
        let repl = s.chars().filter(|&c| c == '\u{FFFD}').count();
        if repl * 4 >= raw.len() {
            return true;
        }
    }
    false
}

/// Assembles incoming serial bytes into displayable lines/packets while ALWAYS preserving raw bytes.
struct LineAssembler {
    buf: Vec<u8>,
    last_rx: Instant,
    prev_cr: bool,
}

impl LineAssembler {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512),
            last_rx: Instant::now(),
            prev_cr: false,
        }
    }

    fn push(&mut self, data: &[u8], out_packets: &mut Vec<(String, Vec<u8>)>) {
        if data.is_empty() {
            return;
        }
        self.last_rx = Instant::now();
        let bin_chunk = looks_binary(data);

        if bin_chunk {
            // Preserve every raw byte (including 0x0D/0x0A) in binary packets; flush at 32-byte chunks
            for &b in data {
                self.buf.push(b);
                if self.buf.len() >= 32 {
                    self.flush_packet(out_packets, true);
                }
            }
            return;
        }

        for &b in data {
            if self.prev_cr {
                self.prev_cr = false;
                if b == b'\n' {
                    // Attach '\n' to the previous packet's raw bytes if we just flushed on '\r'
                    if let Some(last) = out_packets.last_mut() {
                        last.1.push(b'\n');
                    }
                    continue;
                }
            }
            if b == b'\r' {
                self.prev_cr = true;
                self.buf.push(b'\r');
                self.flush_packet(out_packets, false);
            } else if b == b'\n' {
                self.buf.push(b'\n');
                self.flush_packet(out_packets, false);
            } else {
                self.buf.push(b);
                if self.buf.len() >= 4096 {
                    self.flush_packet(out_packets, false);
                }
            }
        }
    }

    fn flush_if_idle(&mut self, idle_ms: u64, out_packets: &mut Vec<(String, Vec<u8>)>) {
        if !self.buf.is_empty() && self.last_rx.elapsed() >= Duration::from_millis(idle_ms) {
            let is_bin = looks_binary(&self.buf);
            self.flush_packet(out_packets, is_bin);
        }
    }

    fn flush_packet(&mut self, out_packets: &mut Vec<(String, Vec<u8>)>, is_binary: bool) {
        if self.buf.is_empty() {
            return;
        }
        let raw = std::mem::take(&mut self.buf);
        let text = if is_binary {
            format_rx_text_lossy(&raw, true)
        } else {
            let trimmed = trim_trailing_eol(&raw);
            format_rx_text_lossy(trimmed, false)
        };
        out_packets.push((text, raw));
    }

    fn clear(&mut self) {
        self.buf.clear();
        self.prev_cr = false;
    }
}

fn trim_trailing_eol(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    if end > 0 && raw[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && raw[end - 1] == b'\r' {
        end -= 1;
    }
    &raw[..end]
}

/// Converts raw RX bytes into visible text (never drops binary bytes to blank).
/// - Valid ANSI CSI color codes (ESC [ ... m) are stripped in text mode.
/// - Invalid UTF-8 renders as U+FFFD ('') so garbled data remains visible.
/// - Non-printable control bytes (< 0x20 except '\t', and 0x7F) render visibly as '·'.
fn format_rx_text_lossy(raw: &[u8], preserve_eol_symbols: bool) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let s = String::from_utf8_lossy(raw);
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Strip genuine ANSI SGR/CSI sequences only in text mode
        if !preserve_eol_symbols && bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'['
        {
            let mut j = i + 2;
            while j < bytes.len()
                && j < i + 20
                && (bytes[j].is_ascii_digit() || bytes[j] == b';' || bytes[j] == b'?')
            {
                j += 1;
            }
            if j < bytes.len() && (0x40..=0x7E).contains(&bytes[j]) {
                i = j + 1;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        if ch == '\t' {
            out.push('\t');
        } else if ch.is_control() {
            out.push('·');
        } else {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    out
}

fn worker_loop(session: Arc<PortSession>) {
    let mut want_connected = {
        let g = session.inner.lock().unwrap();
        !matches!(g.state, ConnectionState::Released)
    };
    let mut port_handle: Option<PlatformSerialPort> = None;
    let mut assembler = LineAssembler::new();
    let mut eol_detector = LineEndingDetector::default();
    let mut last_reconnect_try = Instant::now() - Duration::from_secs(10);
    let mut last_owner_check = Instant::now() - Duration::from_secs(10);
    let mut had_connected_once = false;
    let mut read_buf = [0u8; 2048];
    let mut auto_sample_buf: Vec<u8> = Vec::with_capacity(256);
    let mut auto_sample_errs: u32 = 0;

    while session.running.load(Ordering::SeqCst) {
        // 1. Drain pending commands
        let cmds: Vec<PortCommand> = {
            let mut g = session.inner.lock().unwrap();
            g.cmd_queue.drain(..).collect()
        };

        for cmd in cmds {
            match cmd {
                PortCommand::Shutdown => {
                    drop(port_handle);
                    return;
                }
                PortCommand::Disconnect => {
                    want_connected = false;
                    if port_handle.is_some() {
                        port_handle = None;
                        assembler.clear();
                        session.push_log(
                            LogKind::SysDisconnect,
                            format!(
                                "{} ({})",
                                tr("Disconnected", "切断しました"),
                                session.port_name
                            ),
                        );
                    }
                    session.set_state(ConnectionState::Released);
                }
                PortCommand::Connect => {
                    want_connected = true;
                    last_reconnect_try = Instant::now() - Duration::from_secs(10);
                    if port_handle.is_none() {
                        session.set_state(ConnectionState::Connecting);
                    }
                }
                PortCommand::UpdateConfig(new_cfg) => {
                    let (changed, is_cdc) = {
                        let mut g = session.inner.lock().unwrap();
                        if g.config == new_cfg {
                            (false, g.is_usb_cdc)
                        } else {
                            let prev_auto = g.config.auto_detect;
                            g.config = new_cfg;
                            if !new_cfg.auto_detect {
                                g.auto_locked = true;
                            } else if !prev_auto && new_cfg.auto_detect {
                                g.auto_locked = false;
                                auto_sample_buf.clear();
                                auto_sample_errs = 0;
                            }
                            (true, g.is_usb_cdc)
                        }
                    };
                    if changed {
                        if let Some(ref p) = port_handle {
                            let _ = p.apply_config(new_cfg, is_cdc);
                            session.push_log(
                                LogKind::SysInfo,
                                format!(
                                    "{}: {} {} {}",
                                    tr("Config updated", "設定変更"),
                                    new_cfg.baud,
                                    new_cfg.framing_label(),
                                    if new_cfg.auto_detect {
                                        "[Auto]"
                                    } else {
                                        tr("[Manual]", "[手動]")
                                    }
                                ),
                            );
                        }
                    }
                }
                PortCommand::TriggerAutoDetect => {
                    {
                        let mut g = session.inner.lock().unwrap();
                        g.config.auto_detect = true;
                        g.auto_locked = false;
                    }
                    auto_sample_buf.clear();
                    auto_sample_errs = 0;
                    eol_detector.reset();
                    if let Some(ref p) = port_handle {
                        run_active_autodetect(&session, p, &mut assembler, &mut eol_detector, None);
                    } else {
                        want_connected = true;
                        last_reconnect_try = Instant::now() - Duration::from_secs(10);
                    }
                }
                PortCommand::Send { data } => {
                    if let Some(ref p) = port_handle {
                        if let Err(err) = p.write_all(&data) {
                            session.push_log(
                                LogKind::SysError,
                                format!("{} (OS error {})", tr("Send failed", "送信エラー"), err),
                            );
                        }
                    } else {
                        session.push_log(
                            LogKind::SysError,
                            tr("Not connected — cannot send", "未接続のため送信できません").into(),
                        );
                    }
                }
                PortCommand::KillOwnerAndConnect => {
                    let owner_pid = {
                        let g = session.inner.lock().unwrap();
                        g.state.owner().map(|o| (o.pid, o.name.clone()))
                    };
                    let target = owner_pid.or_else(|| {
                        process_lock::find_port_owner(&session.port_name).map(|o| (o.pid, o.name))
                    });
                    if let Some((pid, name)) = target {
                        match process_lock::terminate_process(pid) {
                            Ok(()) => {
                                session.push_log(
                                    LogKind::SysInfo,
                                    match crate::i18n::lang() {
                                        crate::i18n::Lang::En => {
                                            format!("Terminated owner {} (PID {})", name, pid)
                                        }
                                        crate::i18n::Lang::Ja => format!(
                                            "占有プロセス {} (PID {}) を終了しました",
                                            name, pid
                                        ),
                                    },
                                );
                                want_connected = true;
                                last_reconnect_try = Instant::now() - Duration::from_secs(10);
                                thread::sleep(Duration::from_millis(150));
                            }
                            Err(e) => {
                                session.push_log(LogKind::SysError, e);
                            }
                        }
                    } else {
                        session.push_log(
                            LogKind::SysInfo,
                            tr(
                                "Occupying process not found (retrying connection)",
                                "占有プロセスが見つかりませんでした（再接続を試行します）",
                            )
                            .into(),
                        );
                        want_connected = true;
                        last_reconnect_try = Instant::now() - Duration::from_secs(10);
                    }
                }
                PortCommand::DeviceChanged => {
                    if want_connected && port_handle.is_none() {
                        last_reconnect_try = Instant::now() - Duration::from_secs(10);
                    }
                }
            }
        }

        // 2. If user wants disconnected, sleep on condvar until command arrives
        if !want_connected {
            let g = session.inner.lock().unwrap();
            if g.cmd_queue.is_empty() && session.running.load(Ordering::SeqCst) {
                let _ = session
                    .wake_worker
                    .wait_timeout(g, Duration::from_millis(250));
            }
            continue;
        }

        // 3. If not currently open, attempt to connect / reconnect
        if port_handle.is_none() {
            if last_reconnect_try.elapsed() < Duration::from_millis(450) {
                let g = session.inner.lock().unwrap();
                if g.cmd_queue.is_empty() && session.running.load(Ordering::SeqCst) {
                    let _ = session
                        .wake_worker
                        .wait_timeout(g, Duration::from_millis(120));
                }
                continue;
            }
            last_reconnect_try = Instant::now();

            match PlatformSerialPort::open(&session.port_name) {
                Ok(port) => {
                    let (cfg, is_cdc) = {
                        let mut g = session.inner.lock().unwrap();
                        if g.config.auto_detect {
                            g.auto_locked = false;
                        }
                        (g.config, g.is_usb_cdc)
                    };
                    if let Err(err) = port.apply_config(cfg, is_cdc) {
                        session.set_state(ConnectionState::Disconnected {
                            reason: format!("設定失敗 (error {})", err),
                        });
                        continue;
                    }
                    port.purge();
                    auto_sample_buf.clear();
                    auto_sample_errs = 0;
                    assembler.clear();
                    eol_detector.reset();

                    let verb = if had_connected_once {
                        tr("Reconnected", "再接続しました")
                    } else {
                        tr("Connected", "接続しました")
                    };
                    had_connected_once = true;
                    session.set_state(ConnectionState::Connected);
                    session.push_log(
                        LogKind::SysConnect,
                        format!(
                            "{} ({}: {} {}{})",
                            verb,
                            session.port_name,
                            cfg.baud,
                            cfg.framing_label(),
                            if cfg.auto_detect { " Auto" } else { "" }
                        ),
                    );
                    port_handle = Some(port);
                }
                Err(err) if is_access_denied(err) => {
                    let existing_owner = {
                        let g = session.inner.lock().unwrap();
                        g.state.owner().cloned()
                    };
                    let owner = if existing_owner.is_none()
                        || last_owner_check.elapsed() >= Duration::from_millis(2500)
                    {
                        last_owner_check = Instant::now();
                        process_lock::find_port_owner(&session.port_name)
                    } else {
                        existing_owner
                    };
                    let prev_code = {
                        let g = session.inner.lock().unwrap();
                        g.state.code()
                    };
                    if prev_code != "occupied" {
                        let msg = match (&owner, crate::i18n::lang()) {
                            (Some(o), crate::i18n::Lang::En) => {
                                format!("{} is in use by {} (PID {})", session.port_name, o.name, o.pid)
                            }
                            (Some(o), crate::i18n::Lang::Ja) => {
                                format!("{} は {} (PID {}) によって使用中です", session.port_name, o.name, o.pid)
                            }
                            (None, crate::i18n::Lang::En) => {
                                format!("{} is in use by another process", session.port_name)
                            }
                            (None, crate::i18n::Lang::Ja) => {
                                format!("{} は他のプロセスによって使用中です", session.port_name)
                            }
                        };
                        session.push_log(LogKind::SysError, msg);
                    }
                    session.set_state(ConnectionState::Occupied { owner });
                }
                Err(err) => {
                    let prev_code = {
                        let g = session.inner.lock().unwrap();
                        g.state.code()
                    };
                    let reason = if is_not_found(err) {
                        tr("Device not connected (waiting)", "デバイス未接続 (再接続待機)").to_string()
                    } else {
                        format!("{} (error {})", tr("Waiting", "接続待機中"), err)
                    };
                    if prev_code == "connected" || prev_code == "autodetecting" {
                        session.push_log(
                            LogKind::SysDisconnect,
                            format!(
                                "{} ({}: {})",
                                tr("Disconnected", "切断されました"),
                                session.port_name,
                                reason
                            ),
                        );
                    }
                    session.set_state(ConnectionState::Disconnected { reason });
                }
            }
            continue;
        }

        // 4. Active connected read loop iteration
        let read_res = port_handle.as_ref().unwrap().read_chunk(&mut read_buf, 50);
        match read_res {
            Ok((n, comm_errs)) => {
                if n > 0 {
                    let chunk = &read_buf[..n];
                    let (auto_enabled, auto_locked) = {
                        let g = session.inner.lock().unwrap();
                        (g.config.auto_detect, g.auto_locked)
                    };

                    if auto_enabled && !auto_locked {
                        auto_sample_buf.extend_from_slice(chunk);
                        auto_sample_errs |= comm_errs;

                        if auto_sample_buf.len() >= 16 {
                            let score = score_serial_sample(&auto_sample_buf, auto_sample_errs);
                            if let Some((bits, parity)) = detect_7bit_parity(&auto_sample_buf) {
                                // Immediate 7E1 / 7O1 detection!
                                let (new_cfg, is_cdc) = {
                                    let mut g = session.inner.lock().unwrap();
                                    g.config.data_bits = bits;
                                    g.config.parity = parity;
                                    g.config.stop_bits = StopBits::One;
                                    g.auto_locked = true;
                                    (g.config, g.is_usb_cdc)
                                };
                                let _ = port_handle
                                    .as_ref()
                                    .unwrap()
                                    .apply_config(new_cfg, is_cdc);
                                session.push_log(
                                    LogKind::SysInfo,
                                    format!(
                                        "{}: {} {}",
                                        tr("Auto-detected", "Auto検出"),
                                        new_cfg.baud,
                                        new_cfg.framing_label()
                                    ),
                                );
                                // Strip parity bit from buffered sample and emit cleanly
                                let cleaned: Vec<u8> =
                                    auto_sample_buf.iter().map(|&b| b & 0x7F).collect();
                                emit_rx_bytes(&session, &cleaned, &mut assembler, &mut eol_detector);
                                auto_sample_buf.clear();
                                auto_sample_errs = 0;
                            } else if score >= 0.82 && (auto_sample_errs & 0x0C) == 0 {
                                // Current baud/framing is clean — lock immediately!
                                let cfg = {
                                    let mut g = session.inner.lock().unwrap();
                                    g.auto_locked = true;
                                    g.config
                                };
                                session.push_log(
                                    LogKind::SysInfo,
                                    format!(
                                        "{}: {} {} {}",
                                        tr("Auto-detect", "Auto検出"),
                                        cfg.baud,
                                        cfg.framing_label(),
                                        tr("confirmed", "を確認しました")
                                    ),
                                );
                                emit_rx_bytes(
                                    &session,
                                    &auto_sample_buf,
                                    &mut assembler,
                                    &mut eol_detector,
                                );
                                auto_sample_buf.clear();
                                auto_sample_errs = 0;
                            } else if score < 0.72 || (auto_sample_errs & 0x0C) != 0 {
                                // Garbled/binary data or framing error: run active baud/framing sweep!
                                let saved = std::mem::take(&mut auto_sample_buf);
                                auto_sample_errs = 0;
                                run_active_autodetect(
                                    &session,
                                    port_handle.as_ref().unwrap(),
                                    &mut assembler,
                                    &mut eol_detector,
                                    Some(&saved),
                                );
                            } else {
                                // Still ambiguous with small sample, emit so user isn't blind if buffer grows
                                if auto_sample_buf.len() >= 96 {
                                    let mut g = session.inner.lock().unwrap();
                                    g.auto_locked = true;
                                    drop(g);
                                    emit_rx_bytes(
                                        &session,
                                        &auto_sample_buf,
                                        &mut assembler,
                                        &mut eol_detector,
                                    );
                                    auto_sample_buf.clear();
                                    auto_sample_errs = 0;
                                }
                            }
                        } else {
                            // Small burst (<16 bytes), wait briefly or emit if clean ASCII newline arrives
                            if chunk.contains(&b'\n') || chunk.contains(&b'\r') {
                                let score = score_serial_sample(&auto_sample_buf, auto_sample_errs);
                                if score >= 0.85 && (auto_sample_errs & 0x0C) == 0 {
                                    {
                                        let mut g = session.inner.lock().unwrap();
                                        g.auto_locked = true;
                                    }
                                    emit_rx_bytes(
                                        &session,
                                        &auto_sample_buf,
                                        &mut assembler,
                                        &mut eol_detector,
                                    );
                                    auto_sample_buf.clear();
                                    auto_sample_errs = 0;
                                }
                            }
                        }
                    } else {
                        emit_rx_bytes(&session, chunk, &mut assembler, &mut eol_detector);
                    }
                } else {
                    // No bytes this tick — flush idle partial line / small binary packet
                    if !auto_sample_buf.is_empty() {
                        let score = score_serial_sample(&auto_sample_buf, auto_sample_errs);
                        if score >= 0.80 && (auto_sample_errs & 0x0C) == 0 {
                            emit_rx_bytes(
                                &session,
                                &auto_sample_buf,
                                &mut assembler,
                                &mut eol_detector,
                            );
                            auto_sample_buf.clear();
                            auto_sample_errs = 0;
                        } else {
                            // Small non-ASCII / binary packet arrived and paused:
                            // run one-shot autodetect (which locks even on binary) and never drop the packet!
                            let saved = std::mem::take(&mut auto_sample_buf);
                            auto_sample_errs = 0;
                            run_active_autodetect(
                                &session,
                                port_handle.as_ref().unwrap(),
                                &mut assembler,
                                &mut eol_detector,
                                Some(&saved),
                            );
                        }
                    }
                    let mut packets = Vec::new();
                    assembler.flush_if_idle(55, &mut packets);
                    for (text, raw) in packets {
                        session.push_log_raw(LogKind::Rx, text, raw);
                    }
                }
            }
            Err(err) => {
                // Device unplugged or hardware error!
                port_handle = None;
                assembler.clear();
                let reason = format!("{} (error {})", tr("Disconnected", "切断されました"), err);
                session.push_log(
                    LogKind::SysDisconnect,
                    format!(
                        "{} ({}) — {}",
                        tr("Device disconnected", "デバイスが切断されました"),
                        session.port_name,
                        tr("waiting for reconnect", "再接続を待機します")
                    ),
                );
                session.set_state(ConnectionState::Disconnected { reason });
                last_reconnect_try = Instant::now();
            }
        }
    }
}

fn emit_rx_bytes(
    session: &Arc<PortSession>,
    data: &[u8],
    assembler: &mut LineAssembler,
    eol_detector: &mut LineEndingDetector,
) {
    if !looks_binary(data) {
        eol_detector.feed(data);
        let det = eol_detector.detected();
        if det != LineEnding::None {
            let mut g = session.inner.lock().unwrap();
            if g.detected_eol != det {
                g.detected_eol = det;
            }
        }
    }
    let mut packets = Vec::new();
    assembler.push(data, &mut packets);
    for (text, raw) in packets {
        session.push_log_raw(LogKind::Rx, text, raw);
    }
}

fn run_active_autodetect(
    session: &Arc<PortSession>,
    port: &PlatformSerialPort,
    assembler: &mut LineAssembler,
    eol_detector: &mut LineEndingDetector,
    initial_sample: Option<&[u8]>,
) {
    let (orig_cfg, is_cdc) = {
        let g = session.inner.lock().unwrap();
        (g.config, g.is_usb_cdc)
    };

    let mut best_cfg = orig_cfg;
    let mut best_score: f32 = -1.0;
    let mut best_sample: Vec<u8> = Vec::new();
    let mut any_data_received = initial_sample.map(|s| !s.is_empty()).unwrap_or(false);
    let mut fallback_cfg = orig_cfg;
    let mut fallback_errs = u32::MAX;
    let mut fallback_sample: Vec<u8> = initial_sample.map(|s| s.to_vec()).unwrap_or_default();
    let mut probe_buf = [0u8; 512];

    // Phase 1: Sweep candidate baud rates with 8N1
    for &baud in CANDIDATE_BAUDS {
        if !session.running.load(Ordering::SeqCst) {
            return;
        }
        {
            let g = session.inner.lock().unwrap();
            if !g.cmd_queue.is_empty() {
                break;
            }
        }
        session.set_state(ConnectionState::AutoDetecting { probing_baud: baud });

        let mut test_cfg = orig_cfg;
        test_cfg.baud = baud;
        test_cfg.data_bits = 8;
        test_cfg.parity = Parity::None;
        test_cfg.stop_bits = StopBits::One;

        if port.apply_config(test_cfg, is_cdc).is_err() {
            continue;
        }
        port.purge();

        let mut sample = Vec::with_capacity(128);
        let mut errs = 0u32;
        let deadline = Instant::now() + Duration::from_millis(280);

        while Instant::now() < deadline && sample.len() < 64 {
            match port.read_chunk(&mut probe_buf, 35) {
                Ok((n, e)) => {
                    errs |= e;
                    if n > 0 {
                        sample.extend_from_slice(&probe_buf[..n]);
                    }
                }
                Err(_) => {
                    session.set_state(ConnectionState::Connected);
                    return;
                }
            }
        }

        if sample.is_empty() {
            continue;
        }

        any_data_received = true;
        // Track cleanest baud for binary fallback (prefer 0 framing errors)
        let frame_err_rank = (errs & 0x1C).count_ones();
        if frame_err_rank < fallback_errs
            || (frame_err_rank == fallback_errs && baud == orig_cfg.baud)
        {
            fallback_errs = frame_err_rank;
            fallback_cfg = test_cfg;
            fallback_sample = sample.clone();
        }

        // Check if this baud actually carries 7E1 / 7O1
        if let Some((bits, parity)) = detect_7bit_parity(&sample) {
            test_cfg.data_bits = bits;
            test_cfg.parity = parity;
            let cleaned: Vec<u8> = sample.iter().map(|&b| b & 0x7F).collect();
            let s = score_serial_sample(&cleaned, 0);
            if s > best_score {
                best_score = s;
                best_cfg = test_cfg;
                best_sample = cleaned;
            }
            if s >= 0.90 {
                break;
            }
            continue;
        }

        let s = score_serial_sample(&sample, errs);
        if s > best_score {
            best_score = s;
            best_cfg = test_cfg;
            best_sample = sample;
        }
        if s >= 0.88 && (errs & 0x0C) == 0 && best_sample.len() >= 12 {
            break;
        }
    }

    // Phase 2: If best baud had mediocre score (possible parity/stopbits mismatch), test framing variants
    if best_score > 0.25 && best_score < 0.85 {
        for &(bits, parity, stop_bits) in &CANDIDATE_FRAMINGS[1..] {
            let mut test_cfg = best_cfg;
            test_cfg.data_bits = bits;
            test_cfg.parity = parity;
            test_cfg.stop_bits = stop_bits;
            if port.apply_config(test_cfg, is_cdc).is_err() {
                continue;
            }
            port.purge();
            let mut sample = Vec::with_capacity(64);
            let mut errs = 0u32;
            let deadline = Instant::now() + Duration::from_millis(220);
            while Instant::now() < deadline && sample.len() < 48 {
                if let Ok((n, e)) = port.read_chunk(&mut probe_buf, 30) {
                    errs |= e;
                    if n > 0 {
                        sample.extend_from_slice(&probe_buf[..n]);
                    }
                } else {
                    break;
                }
            }
            if !sample.is_empty() {
                let s = score_serial_sample(&sample, errs);
                if s > best_score {
                    best_score = s;
                    best_cfg = test_cfg;
                    best_sample = sample;
                }
                if s >= 0.88 && (errs & 0x0C) == 0 {
                    break;
                }
            }
        }
    }

    let found_text = best_score >= 0.50;
    let (final_cfg, lock_now) = if found_text {
        (best_cfg, true)
    } else if any_data_received {
        // Binary / custom protocol data received: LOCK so we immediately display all packets instead of re-probing!
        (fallback_cfg, true)
    } else {
        (orig_cfg, false)
    };
    let _ = port.apply_config(final_cfg, is_cdc);

    {
        let mut g = session.inner.lock().unwrap();
        g.config = final_cfg;
        g.auto_locked = lock_now;
    }
    session.set_state(ConnectionState::Connected);

    if found_text {
        session.push_log(
            LogKind::SysInfo,
            format!(
                "{}: {} {} ({} {:.0}%)",
                tr("Auto-detected", "Auto検出完了"),
                final_cfg.baud,
                final_cfg.framing_label(),
                tr("score", "スコア"),
                best_score * 100.0
            ),
        );
        if !best_sample.is_empty() {
            emit_rx_bytes(session, &best_sample, assembler, eol_detector);
        }
    } else if any_data_received {
        session.push_log(
            LogKind::SysInfo,
            format!(
                "{} — {} {} ({})",
                tr("Binary / custom packet detected", "バイナリ/独自形式データを検出"),
                final_cfg.baud,
                final_cfg.framing_label(),
                tr("switch Text/HEX/BIN", "テキスト/HEX/BIN 表示切替可")
            ),
        );
        if !fallback_sample.is_empty() {
            emit_rx_bytes(session, &fallback_sample, assembler, eol_detector);
            let mut packets = Vec::new();
            assembler.flush_if_idle(0, &mut packets);
            for (text, raw) in packets {
                session.push_log_raw(LogKind::Rx, text, raw);
            }
        }
    } else {
        session.push_log(
            LogKind::SysInfo,
            format!(
                "{} — {} {}",
                tr("Auto-detect: waiting for data", "Auto検出: データ受信待機中"),
                final_cfg.baud,
                final_cfg.framing_label()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DisplayMode;

    #[test]
    fn test_binary_packet_never_hidden() {
        // Real binary quaternion-like packet captured from CH340 COM11
        let pkt: Vec<u8> = vec![
            0xBB, 0x08, 0xA0, 0x10, 0x3F, 0xE8, 0x70, 0x0F, 0xBF, 0x2F, 0x36, 0xE0, 0xBE, 0xCC,
            0x41, 0xD6,
        ];
        assert!(looks_binary(&pkt));

        let mut asm = LineAssembler::new();
        let mut out = Vec::new();
        asm.push(&pkt, &mut out);
        asm.flush_if_idle(0, &mut out);
        assert_eq!(out.len(), 1);
        let (text, raw) = &out[0];
        assert_eq!(raw, &pkt);
        assert!(!text.is_empty(), "Text mode must show binary bytes visibly");

        let entry = LogEntry {
            seq: 1,
            timestamp_ms: 0,
            datetime_str: "2026-11-24 00:00:00.000".into(),
            time_str: "00:00:00.000".into(),
            kind: LogKind::Rx,
            text: text.clone(),
            raw_bytes: raw.clone(),
        };
        let hex = entry.format_for_mode(DisplayMode::Hex);
        assert!(hex.starts_with("BB 08 A0 10 3F"));
        assert!(hex.contains("| "));

        let bin = entry.format_for_mode(DisplayMode::Bin);
        assert!(bin.starts_with("10111011 00001000 "));
    }

    #[test]
    fn test_all_control_bytes_visible_in_text_mode() {
        let ctrl = vec![0x00, 0x01, 0x02, 0x03, 0x7F];
        let text = format_rx_text_lossy(&ctrl, true);
        assert_eq!(text, "·····");
    }
}
