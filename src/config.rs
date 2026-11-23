use crate::i18n::Lang;
use crate::json::{obj, parse, Json};
use crate::types::{LayoutMode, NotifyMode, PaneConfig, ThemeMode, WindowRect};
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

pub const DEFAULT_API_PORT: u16 = 27182;

#[derive(Debug, Clone)]
pub struct WindowStateConfig {
    pub rect: WindowRect,
    pub layout: LayoutMode,
    pub split_x: f32,
    pub split_y: f32,
    pub dedicated: bool,
    pub panes: Vec<PaneConfig>,
}

impl WindowStateConfig {
    pub fn single_pane(rect: WindowRect, pane: PaneConfig, dedicated: bool) -> Self {
        let mut panes = vec![pane];
        while panes.len() < 4 {
            panes.push(PaneConfig::default());
        }
        Self {
            rect,
            layout: LayoutMode::Grid1x1,
            split_x: 0.5,
            split_y: 0.5,
            dedicated,
            panes,
        }
    }

    pub fn to_json(&self) -> Json {
        obj(vec![
            ("rect", self.rect.to_json()),
            ("layout", Json::Str(self.layout.as_str().to_string())),
            ("split_x", Json::Num(self.split_x as f64)),
            ("split_y", Json::Num(self.split_y as f64)),
            ("dedicated", Json::Bool(self.dedicated)),
            (
                "panes",
                Json::Arr(self.panes.iter().map(|p| p.to_json()).collect()),
            ),
        ])
    }

    pub fn from_json(j: &Json, default_w: i32, default_h: i32) -> Self {
        let rect = j
            .get("rect")
            .map(|r| WindowRect::from_json(r, default_w, default_h))
            .unwrap_or(WindowRect {
                x: 160,
                y: 120,
                width: default_w,
                height: default_h,
                maximized: false,
            });
        let dedicated = j
            .get("dedicated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let layout = if dedicated {
            LayoutMode::Grid1x1
        } else {
            j.get("layout")
                .and_then(|v| v.as_str())
                .map(LayoutMode::from_str)
                .unwrap_or(LayoutMode::Grid1x1)
        };
        let split_x = match j.get("split_x") {
            Some(Json::Num(n)) => (*n as f32).clamp(0.15, 0.85),
            _ => 0.5,
        };
        let split_y = match j.get("split_y") {
            Some(Json::Num(n)) => (*n as f32).clamp(0.15, 0.85),
            _ => 0.5,
        };
        let mut panes: Vec<PaneConfig> = if let Some(arr) = j.get("panes").and_then(|v| v.as_arr())
        {
            arr.iter().map(PaneConfig::from_json).collect()
        } else if let Some(single) = j.get("pane") {
            vec![PaneConfig::from_json(single)]
        } else {
            vec![PaneConfig::default()]
        };
        while panes.len() < 4 {
            panes.push(PaneConfig::default());
        }
        Self {
            rect,
            layout,
            split_x,
            split_y,
            dedicated,
            panes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub api_port: u16,
    pub lang: Lang,
    pub theme_mode: ThemeMode,
    pub notify_mode: NotifyMode,
    pub main_rect: WindowRect,
    pub layout: LayoutMode,
    pub main_split_x: f32,
    pub main_split_y: f32,
    pub main_panes: Vec<PaneConfig>,
    pub extra_windows: Vec<WindowStateConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut main_panes = Vec::with_capacity(4);
        for _ in 0..4 {
            main_panes.push(PaneConfig::default());
        }
        Self {
            api_port: DEFAULT_API_PORT,
            lang: Lang::detect_os_default(),
            theme_mode: ThemeMode::System,
            notify_mode: NotifyMode::InApp,
            main_rect: WindowRect::default(),
            layout: LayoutMode::Grid1x1,
            main_split_x: 0.5,
            main_split_y: 0.5,
            main_panes,
            extra_windows: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn to_json(&self) -> Json {
        let panes_json: Vec<Json> = self.main_panes.iter().map(|p| p.to_json()).collect();
        let extra_json: Vec<Json> = self.extra_windows.iter().map(|w| w.to_json()).collect();

        obj(vec![
            ("version", Json::Num(3.0)),
            ("api_port", Json::Num(self.api_port as f64)),
            ("lang", Json::Str(self.lang.as_str().to_string())),
            ("theme", Json::Str(self.theme_mode.as_str().to_string())),
            (
                "notify_mode",
                Json::Str(self.notify_mode.as_str().to_string()),
            ),
            ("main_window", self.main_rect.to_json()),
            ("layout", Json::Str(self.layout.as_str().to_string())),
            ("main_split_x", Json::Num(self.main_split_x as f64)),
            ("main_split_y", Json::Num(self.main_split_y as f64)),
            ("main_panes", Json::Arr(panes_json)),
            ("extra_windows", Json::Arr(extra_json)),
        ])
    }

    pub fn from_json(j: &Json) -> Self {
        let mut cfg = Self::default();
        if let Some(p) = j.get("api_port").and_then(|v| v.as_u64()) {
            if (1024..=65535).contains(&p) {
                cfg.api_port = p as u16;
            }
        }
        if let Some(l) = j.get("lang").and_then(|v| v.as_str()).and_then(Lang::from_str) {
            cfg.lang = l;
        }
        if let Some(t) = j.get("theme").and_then(|v| v.as_str()) {
            cfg.theme_mode = ThemeMode::from_str(t);
        }
        if let Some(nm) = j.get("notify_mode").and_then(|v| v.as_str()) {
            cfg.notify_mode = NotifyMode::from_str(nm);
        }
        if let Some(mw) = j.get("main_window") {
            cfg.main_rect = WindowRect::from_json(mw, 1100, 740);
        }
        if let Some(l) = j.get("layout").and_then(|v| v.as_str()) {
            cfg.layout = LayoutMode::from_str(l);
        }
        if let Some(Json::Num(n)) = j.get("main_split_x") {
            cfg.main_split_x = (*n as f32).clamp(0.15, 0.85);
        }
        if let Some(Json::Num(n)) = j.get("main_split_y") {
            cfg.main_split_y = (*n as f32).clamp(0.15, 0.85);
        }
        if let Some(arr) = j.get("main_panes").and_then(|v| v.as_arr()) {
            let parsed: Vec<PaneConfig> = arr.iter().map(PaneConfig::from_json).collect();
            if !parsed.is_empty() {
                cfg.main_panes = parsed;
            }
        }
        if let Some(arr) = j
            .get("extra_windows")
            .or_else(|| j.get("detached_windows"))
            .and_then(|v| v.as_arr())
        {
            cfg.extra_windows = arr
                .iter()
                .map(|item| WindowStateConfig::from_json(item, 860, 620))
                .collect();
        }
        while cfg.main_panes.len() < 4 {
            cfg.main_panes.push(PaneConfig::default());
        }
        cfg
    }
}

pub fn portable_base_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let probe = dir.join(".ssm_write_test");
            if std::fs::write(&probe, b"").is_ok() {
                let _ = std::fs::remove_file(&probe);
                return dir.to_path_buf();
            }
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("SimpleSerialMonitor");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn config_file_path() -> PathBuf {
    portable_base_dir().join("ssm_config.json")
}

pub fn logs_dir_path() -> PathBuf {
    let dir = portable_base_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn load_config() -> AppConfig {
    let path = config_file_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(json) = parse(&text) {
            return AppConfig::from_json(&json);
        }
    }
    AppConfig::default()
}

pub fn save_config(cfg: &AppConfig) {
    let path = config_file_path();
    let content = cfg.to_json().to_json_pretty();
    let _ = std::fs::write(path, content);
}

pub fn clamp_rect_to_screen(rect: &WindowRect) -> WindowRect {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(800);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(600);

        let w = rect.width.clamp(380, vw);
        let h = rect.height.clamp(280, vh);
        let x = rect.x.clamp(vx, vx + vw - 120);
        let y = rect.y.clamp(vy, vy + vh - 80);

        WindowRect {
            x,
            y,
            width: w,
            height: h,
            maximized: rect.maximized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DisplayMode, LineEnding, Parity, StopBits};

    #[test]
    fn test_config_roundtrip_multi_window() {
        let mut cfg = AppConfig::default();
        cfg.notify_mode = NotifyMode::Both;
        cfg.layout = LayoutMode::Grid2x2;
        cfg.main_split_x = 0.35;
        cfg.main_split_y = 0.65;
        cfg.main_rect = WindowRect {
            x: 50,
            y: 60,
            width: 1280,
            height: 800,
            maximized: true,
        };
        cfg.main_panes[0].port = "COM9".into();
        cfg.main_panes[0].serial.baud = 921600;
        cfg.main_panes[0].serial.auto_detect = false;
        cfg.main_panes[0].serial.parity = Parity::Even;
        cfg.main_panes[0].serial.stop_bits = StopBits::Two;
        cfg.main_panes[0].serial.tx_eol = LineEnding::Lf;
        cfg.main_panes[0].word_wrap = true;
        cfg.main_panes[0].display_mode = DisplayMode::Hex;
        cfg.main_panes[0].zoom_percent = 130;

        let ew = WindowStateConfig::single_pane(
            WindowRect {
                x: 200,
                y: 220,
                width: 680,
                height: 480,
                maximized: false,
            },
            PaneConfig {
                port: "COM11".into(),
                display_mode: DisplayMode::Bin,
                ..PaneConfig::default()
            },
            true,
        );
        cfg.extra_windows.push(ew);

        let text = cfg.to_json().to_json_pretty();
        let parsed = AppConfig::from_json(&parse(&text).unwrap());
        assert_eq!(parsed.notify_mode, NotifyMode::Both);
        assert_eq!(parsed.layout, LayoutMode::Grid2x2);
        assert!((parsed.main_split_x - 0.35).abs() < 0.001);
        assert!((parsed.main_split_y - 0.65).abs() < 0.001);
        assert_eq!(parsed.main_panes[0].display_mode, DisplayMode::Hex);
        assert_eq!(parsed.extra_windows.len(), 1);
        assert!(parsed.extra_windows[0].dedicated);
        assert_eq!(parsed.extra_windows[0].panes[0].port, "COM11");
        assert_eq!(parsed.extra_windows[0].panes[0].display_mode, DisplayMode::Bin);
    }
}
