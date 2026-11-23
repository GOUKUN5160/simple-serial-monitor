use crate::json::{obj, Json};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
    Mark,
    Space,
}

impl Parity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Parity::None => "N",
            Parity::Odd => "O",
            Parity::Even => "E",
            Parity::Mark => "M",
            Parity::Space => "S",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "O" | "ODD" => Parity::Odd,
            "E" | "EVEN" => Parity::Even,
            "M" | "MARK" => Parity::Mark,
            "S" | "SPACE" => Parity::Space,
            _ => Parity::None,
        }
    }

    pub fn win32_val(&self) -> u8 {
        match self {
            Parity::None => 0,  // NOPARITY
            Parity::Odd => 1,   // ODDPARITY
            Parity::Even => 2,  // EVENPARITY
            Parity::Mark => 3,  // MARKPARITY
            Parity::Space => 4, // SPACEPARITY
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    OnePointFive,
    Two,
}

impl StopBits {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopBits::One => "1",
            StopBits::OnePointFive => "1.5",
            StopBits::Two => "2",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim() {
            "1.5" => StopBits::OnePointFive,
            "2" => StopBits::Two,
            _ => StopBits::One,
        }
    }

    pub fn win32_val(&self) -> u8 {
        match self {
            StopBits::One => 0,          // ONESTOPBIT
            StopBits::OnePointFive => 1, // ONE5STOPBITS
            StopBits::Two => 2,          // TWOSTOPBITS
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Auto,
    CrLf,
    Lf,
    Cr,
    None,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::Auto => "auto",
            LineEnding::CrLf => "crlf",
            LineEnding::Lf => "lf",
            LineEnding::Cr => "cr",
            LineEnding::None => "none",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            LineEnding::Auto => "Auto",
            LineEnding::CrLf => "CRLF",
            LineEnding::Lf => "LF",
            LineEnding::Cr => "CR",
            LineEnding::None => "None",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "crlf" | "\\r\\n" => LineEnding::CrLf,
            "lf" | "\\n" => LineEnding::Lf,
            "cr" | "\\r" => LineEnding::Cr,
            "none" | "" => LineEnding::None,
            _ => LineEnding::Auto,
        }
    }

    pub fn bytes(&self, detected: LineEnding) -> &'static [u8] {
        let effective = match self {
            LineEnding::Auto => match detected {
                LineEnding::CrLf => LineEnding::CrLf,
                LineEnding::Lf => LineEnding::Lf,
                LineEnding::Cr => LineEnding::Cr,
                _ => LineEnding::CrLf,
            },
            other => *other,
        };
        match effective {
            LineEnding::CrLf => b"\r\n",
            LineEnding::Lf => b"\n",
            LineEnding::Cr => b"\r",
            _ => b"",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialConfig {
    pub auto_detect: bool,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub tx_eol: LineEnding,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            auto_detect: true,
            baud: 115200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: StopBits::One,
            tx_eol: LineEnding::Auto,
        }
    }
}

impl SerialConfig {
    pub fn framing_label(&self) -> String {
        format!("{}{}{}", self.data_bits, self.parity.as_str(), self.stop_bits.as_str())
    }

    pub fn to_json(&self) -> Json {
        obj(vec![
            ("auto_detect", Json::Bool(self.auto_detect)),
            ("baud", Json::Num(self.baud as f64)),
            ("data_bits", Json::Num(self.data_bits as f64)),
            ("parity", Json::Str(self.parity.as_str().to_string())),
            ("stop_bits", Json::Str(self.stop_bits.as_str().to_string())),
            ("tx_eol", Json::Str(self.tx_eol.as_str().to_string())),
        ])
    }

    pub fn from_json(j: &Json) -> Self {
        let mut cfg = Self::default();
        if let Some(v) = j.get("auto_detect").and_then(|v| v.as_bool()) {
            cfg.auto_detect = v;
        }
        if let Some(v) = j.get("baud").and_then(|v| v.as_u64()) {
            if (110..=4_000_000).contains(&v) {
                cfg.baud = v as u32;
            }
        }
        if let Some(v) = j.get("data_bits").and_then(|v| v.as_u64()) {
            if (5..=8).contains(&v) {
                cfg.data_bits = v as u8;
            }
        }
        if let Some(v) = j.get("parity").and_then(|v| v.as_str()) {
            cfg.parity = Parity::from_str(v);
        }
        if let Some(v) = j.get("stop_bits").and_then(|v| v.as_str()) {
            cfg.stop_bits = StopBits::from_str(v);
        }
        if let Some(v) = j.get("tx_eol").and_then(|v| v.as_str()) {
            cfg.tx_eol = LineEnding::from_str(v);
        }
        cfg
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,          // e.g. "COM3"
    pub friendly_name: String, // e.g. "Silicon Labs CP210x USB to UART Bridge (COM3)"
    pub hardware_label: String,// Enriched concise label e.g. "ESP32 USB JTAG/Serial"
    pub manufacturer: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub is_usb_cdc: bool,
}

impl PortInfo {
    pub fn display_label(&self) -> String {
        if self.hardware_label.is_empty() {
            self.name.clone()
        } else {
            format!("{} - {}", self.name, self.hardware_label)
        }
    }

    pub fn to_json(&self) -> Json {
        obj(vec![
            ("port", Json::Str(self.name.clone())),
            ("friendly_name", Json::Str(self.friendly_name.clone())),
            ("hardware_name", Json::Str(self.hardware_label.clone())),
            ("manufacturer", Json::Str(self.manufacturer.clone())),
            (
                "vid",
                self.vid
                    .map(|v| Json::Str(format!("{:04X}", v)))
                    .unwrap_or(Json::Null),
            ),
            (
                "pid",
                self.pid
                    .map(|v| Json::Str(format!("{:04X}", v)))
                    .unwrap_or(Json::Null),
            ),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOwner {
    pub pid: u32,
    pub name: String,
    pub exe_path: String,
}

impl ProcessOwner {
    pub fn to_json(&self) -> Json {
        obj(vec![
            ("pid", Json::Num(self.pid as f64)),
            ("name", Json::Str(self.name.clone())),
            ("exe_path", Json::Str(self.exe_path.clone())),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Released,
    Connecting,
    AutoDetecting { probing_baud: u32 },
    Connected,
    Disconnected { reason: String },
    Occupied { owner: Option<ProcessOwner> },
}

impl ConnectionState {
    pub fn code(&self) -> &'static str {
        match self {
            ConnectionState::Released => "released",
            ConnectionState::Connecting => "connecting",
            ConnectionState::AutoDetecting { .. } => "autodetecting",
            ConnectionState::Connected => "connected",
            ConnectionState::Disconnected { .. } => "disconnected",
            ConnectionState::Occupied { .. } => "occupied",
        }
    }

    pub fn owner(&self) -> Option<&ProcessOwner> {
        match self {
            ConnectionState::Occupied { owner } => owner.as_ref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Text,
    Hex,
    Bin,
}

impl DisplayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DisplayMode::Text => "text",
            DisplayMode::Hex => "hex",
            DisplayMode::Bin => "bin",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "hex" => DisplayMode::Hex,
            "bin" | "binary" => DisplayMode::Bin,
            _ => DisplayMode::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMode {
    Off,
    InApp,
    Windows,
    Both,
}

impl NotifyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotifyMode::Off => "off",
            NotifyMode::InApp => "in_app",
            NotifyMode::Windows => "windows",
            NotifyMode::Both => "both",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" => NotifyMode::Off,
            "windows" | "os" => NotifyMode::Windows,
            "both" => NotifyMode::Both,
            _ => NotifyMode::InApp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Rx,
    Tx,
    SysConnect,
    SysDisconnect,
    SysError,
    SysInfo,
}

impl LogKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogKind::Rx => "rx",
            LogKind::Tx => "tx",
            LogKind::SysConnect => "sys_connect",
            LogKind::SysDisconnect => "sys_disconnect",
            LogKind::SysError => "sys_error",
            LogKind::SysInfo => "sys_info",
        }
    }

    pub fn is_system(&self) -> bool {
        matches!(
            self,
            LogKind::SysConnect | LogKind::SysDisconnect | LogKind::SysError | LogKind::SysInfo
        )
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub datetime_str: String,
    pub time_str: String,
    pub kind: LogKind,
    pub text: String,
    pub raw_bytes: Vec<u8>,
}

impl LogEntry {
    pub fn to_json(&self) -> Json {
        let mut fields = vec![
            ("seq", Json::Num(self.seq as f64)),
            ("ts_ms", Json::Num(self.timestamp_ms as f64)),
            ("datetime", Json::Str(self.datetime_str.clone())),
            ("time", Json::Str(self.time_str.clone())),
            ("kind", Json::Str(self.kind.as_str().to_string())),
            ("text", Json::Str(self.text.clone())),
        ];
        if !self.raw_bytes.is_empty() {
            fields.push(("raw_hex", Json::Str(bytes_to_hex_compact(&self.raw_bytes))));
        }
        obj(fields)
    }

    pub fn format_for_mode(&self, mode: DisplayMode) -> String {
        if self.kind.is_system() || self.raw_bytes.is_empty() {
            return self.text.clone();
        }
        match mode {
            DisplayMode::Text => self.text.clone(),
            DisplayMode::Hex => bytes_to_hex_preview(&self.raw_bytes),
            DisplayMode::Bin => bytes_to_bin(&self.raw_bytes),
        }
    }
}

pub fn bytes_to_hex_compact(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        use std::fmt::Write as _;
        let _ = write!(out, "{:02X}", b);
    }
    out
}

pub fn bytes_to_hex_preview(bytes: &[u8]) -> String {
    let hex = bytes_to_hex_compact(bytes);
    let mut ascii = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x20..=0x7E).contains(&b) {
            ascii.push(b as char);
        } else {
            ascii.push('.');
        }
    }
    format!("{}  | {}", hex, ascii)
}

pub fn bytes_to_bin(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 9);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        use std::fmt::Write as _;
        let _ = write!(out, "{:08b}", b);
    }
    out
}

pub fn now_ms_and_local_datetime() -> (u64, String, String) {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut st = windows_sys::Win32::Foundation::SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
    }
    let time_str = format!(
        "{:02}:{:02}:{:02}.{:03}",
        st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    );
    let datetime_str = format!(
        "{:04}-{:02}-{:02} {}",
        st.wYear, st.wMonth, st.wDay, time_str
    );
    (ms, datetime_str, time_str)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Grid1x1,
    Grid1x2,
    Grid2x1,
    Grid2x2,
}

impl LayoutMode {
    pub fn pane_count(&self) -> usize {
        match self {
            LayoutMode::Grid1x1 => 1,
            LayoutMode::Grid1x2 | LayoutMode::Grid2x1 => 2,
            LayoutMode::Grid2x2 => 4,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::Grid1x1 => "1x1",
            LayoutMode::Grid1x2 => "1x2",
            LayoutMode::Grid2x1 => "2x1",
            LayoutMode::Grid2x2 => "2x2",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "1x2" => LayoutMode::Grid1x2,
            "2x1" => LayoutMode::Grid2x1,
            "2x2" => LayoutMode::Grid2x2,
            _ => LayoutMode::Grid1x1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl Default for WindowRect {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 1100,
            height: 740,
            maximized: false,
        }
    }
}

impl WindowRect {
    pub fn to_json(&self) -> Json {
        obj(vec![
            ("x", Json::Num(self.x as f64)),
            ("y", Json::Num(self.y as f64)),
            ("width", Json::Num(self.width as f64)),
            ("height", Json::Num(self.height as f64)),
            ("maximized", Json::Bool(self.maximized)),
        ])
    }

    pub fn from_json(j: &Json, default_w: i32, default_h: i32) -> Self {
        Self {
            x: j.get("x").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            y: j.get("y").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            width: j.get("width").and_then(|v| v.as_i64()).unwrap_or(default_w as i64).max(360) as i32,
            height: j.get("height").and_then(|v| v.as_i64()).unwrap_or(default_h as i64).max(260) as i32,
            maximized: j.get("maximized").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaneConfig {
    pub port: String,
    pub serial: SerialConfig,
    pub auto_scroll: bool,
    pub show_timestamps: bool,
    pub word_wrap: bool,
    pub display_mode: DisplayMode,
    pub zoom_percent: u32,
    pub connected: bool,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            port: String::new(),
            serial: SerialConfig::default(),
            auto_scroll: true,
            show_timestamps: false,
            word_wrap: false,
            display_mode: DisplayMode::Text,
            zoom_percent: 100,
            connected: true,
        }
    }
}

impl PaneConfig {
    pub fn to_json(&self) -> Json {
        obj(vec![
            ("port", Json::Str(self.port.clone())),
            ("serial", self.serial.to_json()),
            ("auto_scroll", Json::Bool(self.auto_scroll)),
            ("show_timestamps", Json::Bool(self.show_timestamps)),
            ("word_wrap", Json::Bool(self.word_wrap)),
            (
                "display_mode",
                Json::Str(self.display_mode.as_str().to_string()),
            ),
            ("zoom_percent", Json::Num(self.zoom_percent as f64)),
            ("connected", Json::Bool(self.connected)),
        ])
    }

    pub fn from_json(j: &Json) -> Self {
        let mut pc = Self::default();
        if let Some(s) = j.get("port").and_then(|v| v.as_str()) {
            pc.port = s.to_string();
        }
        if let Some(s) = j.get("serial") {
            pc.serial = SerialConfig::from_json(s);
        }
        if let Some(b) = j.get("auto_scroll").and_then(|v| v.as_bool()) {
            pc.auto_scroll = b;
        }
        if let Some(b) = j.get("show_timestamps").and_then(|v| v.as_bool()) {
            pc.show_timestamps = b;
        }
        if let Some(b) = j.get("word_wrap").and_then(|v| v.as_bool()) {
            pc.word_wrap = b;
        }
        if let Some(m) = j.get("display_mode").and_then(|v| v.as_str()) {
            pc.display_mode = DisplayMode::from_str(m);
        }
        if let Some(z) = j.get("zoom_percent").and_then(|v| v.as_u64()) {
            pc.zoom_percent = (z as u32).clamp(50, 300);
        }
        if let Some(b) = j.get("connected").and_then(|v| v.as_bool()) {
            pc.connected = b;
        }
        pc
    }
}
