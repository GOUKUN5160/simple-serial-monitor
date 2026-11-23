pub mod logview;
pub mod notify;
pub mod pane;
pub mod tooltip;
pub mod window;

use crate::config::{load_config, AppConfig};
use crate::manager::MonitorManager;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{COLORREF, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, FF_MODERN, FIXED_PITCH, FW_BOLD, FW_NORMAL,
    HBRUSH, HFONT, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX,
};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowTextLengthW, GetWindowTextW, SetWindowTextW,
    TranslateMessage, MSG, WM_APP,
};

pub const WM_SSM_DPI_CHANGED: u32 = WM_APP + 30;
pub const IDI_APP_ICON: usize = 1;

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn set_window_text(hwnd: HWND, text: &str) {
    let w = to_wide(text);
    unsafe {
        SetWindowTextW(hwnd, w.as_ptr());
    }
}

pub fn get_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn dpi_scale(val: i32, dpi: u32) -> i32 {
    ((val as i64 * dpi as i64 + 48) / 96) as i32
}

pub fn window_dpi(hwnd: HWND) -> u32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 {
        96
    } else {
        d
    }
}

pub struct UiTheme {
    pub dpi: u32,
    pub font_ui: HFONT,
    pub font_ui_bold: HFONT,
    pub font_mono: HFONT,
    pub brush_status_connected: HBRUSH,
    pub brush_status_detecting: HBRUSH,
    pub brush_status_occupied: HBRUSH,
    pub brush_status_disconnected: HBRUSH,
    pub brush_status_released: HBRUSH,
}

impl UiTheme {
    pub fn new(dpi: u32) -> Self {
        unsafe {
            let ui_face = to_wide("Segoe UI");
            let mono_face = to_wide("Consolas");
            let h_ui = -dpi_scale(13, dpi);
            let h_mono = -dpi_scale(13, dpi);

            let font_ui = CreateFontW(
                h_ui,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32,
                ui_face.as_ptr(),
            );
            let font_ui_bold = CreateFontW(
                h_ui,
                0,
                0,
                0,
                FW_BOLD as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32,
                ui_face.as_ptr(),
            );
            let font_mono = CreateFontW(
                h_mono,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                (FIXED_PITCH | FF_MODERN) as u32,
                mono_face.as_ptr(),
            );

            Self {
                dpi,
                font_ui,
                font_ui_bold,
                font_mono,
                brush_status_connected: CreateSolidBrush(rgb(27, 118, 56)),
                brush_status_detecting: CreateSolidBrush(rgb(25, 103, 210)),
                brush_status_occupied: CreateSolidBrush(rgb(198, 40, 40)),
                brush_status_disconnected: CreateSolidBrush(rgb(176, 104, 0)),
                brush_status_released: CreateSolidBrush(rgb(88, 96, 105)),
            }
        }
    }
}

impl Drop for UiTheme {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font_ui);
            DeleteObject(self.font_ui_bold);
            DeleteObject(self.font_mono);
            DeleteObject(self.brush_status_connected);
            DeleteObject(self.brush_status_detecting);
            DeleteObject(self.brush_status_occupied);
            DeleteObject(self.brush_status_disconnected);
            DeleteObject(self.brush_status_released);
        }
    }
}

pub const COLOR_STATUS_FG: COLORREF = rgb(255, 255, 255);
pub const COLOR_STATUS_CONNECTED: COLORREF = rgb(27, 118, 56);
pub const COLOR_STATUS_DETECTING: COLORREF = rgb(25, 103, 210);
pub const COLOR_STATUS_OCCUPIED: COLORREF = rgb(198, 40, 40);
pub const COLOR_STATUS_DISCONNECTED: COLORREF = rgb(176, 104, 0);
pub const COLOR_STATUS_RELEASED: COLORREF = rgb(88, 96, 105);

pub fn run_gui(manager: Arc<MonitorManager>, initial_config: AppConfig, api_port: Option<u16>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(null());
        window::register_window_classes(hinst);
        pane::register_pane_class(hinst);
        logview::register_logview_class(hinst);
        notify::register_toast_class(hinst);

        let _main_hwnd = window::create_initial_windows(hinst, manager.clone(), initial_config, api_port);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    manager.shutdown_all();
}

pub fn load_initial_config() -> AppConfig {
    load_config()
}
