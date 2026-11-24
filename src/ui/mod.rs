#[cfg(windows)]
pub mod logview;
#[cfg(windows)]
pub mod notify;
#[cfg(windows)]
pub mod pane;
#[cfg(windows)]
pub mod settings;
#[cfg(windows)]
pub mod tooltip;
#[cfg(windows)]
pub mod window;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
mod win32_ui {
    pub use super::{logview, notify, pane, settings, tooltip, window};
}

#[cfg(windows)]
use crate::config::{load_config, AppConfig};
#[cfg(windows)]
use crate::manager::MonitorManager;
#[cfg(windows)]
use crate::types::ThemeMode;
#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::ptr::{null, null_mut};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, FF_MODERN, FIXED_PITCH, FW_BOLD, FW_NORMAL,
    HBRUSH, HFONT, OUT_DEFAULT_PRECIS,
};
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX,
};
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowTextLengthW, GetWindowTextW, SetWindowTextW,
    TranslateMessage, MSG, WM_APP,
};

#[cfg(windows)]
pub const WM_SSM_DPI_CHANGED: u32 = WM_APP + 30;
#[cfg(windows)]
pub const IDI_APP_ICON: usize = 1;

#[cfg(windows)]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: HWND,
        dwattribute: u32,
        pvattribute: *const c_void,
        cbattribute: u32,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "uxtheme")]
extern "system" {
    fn SetWindowTheme(hwnd: HWND, pszsubappname: *const u16, pszsubidlist: *const u16) -> i32;
}

#[cfg(windows)]
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
pub unsafe fn copy_text_to_clipboard(hwnd: HWND, text: &str) -> bool {
    const CF_UNICODETEXT: u32 = 13;
    let wide = to_wide(text);
    if OpenClipboard(hwnd) == 0 {
        return false;
    }
    EmptyClipboard();
    let mut ok = false;
    let hmem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
    if !hmem.is_null() {
        let dst = GlobalLock(hmem) as *mut u16;
        if !dst.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            GlobalUnlock(hmem);
            ok = !SetClipboardData(CF_UNICODETEXT, hmem).is_null();
        }
    }
    CloseClipboard();
    ok
}

#[cfg(windows)]
pub fn set_window_text(hwnd: HWND, text: &str) {
    let w = to_wide(text);
    unsafe {
        SetWindowTextW(hwnd, w.as_ptr());
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

#[cfg(windows)]
pub fn dpi_scale(val: i32, dpi: u32) -> i32 {
    ((val as i64 * dpi as i64 + 48) / 96) as i32
}

#[cfg(windows)]
pub fn window_dpi(hwnd: HWND) -> u32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 {
        96
    } else {
        d
    }
}

#[cfg(windows)]
pub fn is_os_dark_mode() -> bool {
    unsafe {
        let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let val_name = to_wide("AppsUseLightTheme");
        let mut hkey: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS
        {
            return false;
        }
        let mut val: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut reg_type = 0u32;
        let st = RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            null(),
            &mut reg_type,
            &mut val as *mut u32 as *mut u8,
            &mut size,
        );
        RegCloseKey(hkey);
        st == ERROR_SUCCESS && reg_type == REG_DWORD && val == 0
    }
}

#[cfg(windows)]
pub fn resolve_is_dark(mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => is_os_dark_mode(),
    }
}

#[cfg(windows)]
pub unsafe fn apply_window_dark_titlebar(hwnd: HWND, dark: bool) {
    let val: i32 = if dark { 1 } else { 0 };
    // Attribute 20 (Win10 2004+ / Win11), fallback 19 (older Win10)
    let _ = DwmSetWindowAttribute(
        hwnd,
        20,
        &val as *const i32 as *const c_void,
        std::mem::size_of::<i32>() as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        19,
        &val as *const i32 as *const c_void,
        std::mem::size_of::<i32>() as u32,
    );
}

#[cfg(windows)]
#[derive(Clone, Copy)]
pub enum CtrlThemeKind {
    Button,
    Combo,
    Edit,
    Checkbox,
    Scroll,
}

#[cfg(windows)]
pub unsafe fn apply_ctrl_dark_mode(ctrl: HWND, dark: bool, kind: CtrlThemeKind) {
    if ctrl.is_null() {
        return;
    }
    match (dark, kind) {
        (true, CtrlThemeKind::Button | CtrlThemeKind::Edit | CtrlThemeKind::Scroll) => {
            let name = to_wide("DarkMode_Explorer");
            SetWindowTheme(ctrl, name.as_ptr(), null());
        }
        (true, CtrlThemeKind::Combo) => {
            let name = to_wide("DarkMode_CFD");
            SetWindowTheme(ctrl, name.as_ptr(), null());
        }
        (true, CtrlThemeKind::Checkbox) => {
            // Classic style allows WM_CTLCOLORSTATIC to paint crisp light text on dark background
            let empty = to_wide("");
            SetWindowTheme(ctrl, empty.as_ptr(), empty.as_ptr());
        }
        (false, CtrlThemeKind::Checkbox) => {
            SetWindowTheme(ctrl, null(), null());
        }
        (false, _) => {
            let name = to_wide("Explorer");
            SetWindowTheme(ctrl, name.as_ptr(), null());
        }
    }
}

#[cfg(windows)]
pub struct UiTheme {
    pub dpi: u32,
    pub dark: bool,
    pub font_ui: HFONT,
    pub font_ui_bold: HFONT,
    pub font_mono: HFONT,
    pub col_bg: COLORREF,
    pub col_fg: COLORREF,
    pub col_edit_bg: COLORREF,
    pub col_splitter_bg: COLORREF,
    pub col_splitter_grip: COLORREF,
    pub brush_bg: HBRUSH,
    pub brush_edit_bg: HBRUSH,
    pub brush_splitter_bg: HBRUSH,
    pub brush_splitter_grip: HBRUSH,
    pub brush_status_connected: HBRUSH,
    pub brush_status_detecting: HBRUSH,
    pub brush_status_occupied: HBRUSH,
    pub brush_status_disconnected: HBRUSH,
    pub brush_status_released: HBRUSH,
}

#[cfg(windows)]
impl UiTheme {
    pub fn new(dpi: u32, dark: bool) -> Self {
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

            let (col_bg, col_fg, col_edit_bg, col_splitter_bg, col_splitter_grip) = if dark {
                (
                    rgb(30, 33, 40),
                    rgb(230, 237, 243),
                    rgb(40, 44, 52),
                    rgb(38, 42, 52),
                    rgb(96, 106, 124),
                )
            } else {
                (
                    rgb(243, 244, 246),
                    rgb(24, 28, 34),
                    rgb(255, 255, 255),
                    rgb(232, 235, 240),
                    rgb(165, 173, 184),
                )
            };

            Self {
                dpi,
                dark,
                font_ui,
                font_ui_bold,
                font_mono,
                col_bg,
                col_fg,
                col_edit_bg,
                col_splitter_bg,
                col_splitter_grip,
                brush_bg: CreateSolidBrush(col_bg),
                brush_edit_bg: CreateSolidBrush(col_edit_bg),
                brush_splitter_bg: CreateSolidBrush(col_splitter_bg),
                brush_splitter_grip: CreateSolidBrush(col_splitter_grip),
                brush_status_connected: CreateSolidBrush(rgb(27, 118, 56)),
                brush_status_detecting: CreateSolidBrush(rgb(25, 103, 210)),
                brush_status_occupied: CreateSolidBrush(rgb(198, 40, 40)),
                brush_status_disconnected: CreateSolidBrush(rgb(176, 104, 0)),
                brush_status_released: CreateSolidBrush(rgb(88, 96, 105)),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for UiTheme {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font_ui);
            DeleteObject(self.font_ui_bold);
            DeleteObject(self.font_mono);
            DeleteObject(self.brush_bg);
            DeleteObject(self.brush_edit_bg);
            DeleteObject(self.brush_splitter_bg);
            DeleteObject(self.brush_splitter_grip);
            DeleteObject(self.brush_status_connected);
            DeleteObject(self.brush_status_detecting);
            DeleteObject(self.brush_status_occupied);
            DeleteObject(self.brush_status_disconnected);
            DeleteObject(self.brush_status_released);
        }
    }
}

#[cfg(windows)]
pub const COLOR_STATUS_FG: COLORREF = rgb(255, 255, 255);
#[cfg(windows)]
pub const COLOR_STATUS_CONNECTED: COLORREF = rgb(27, 118, 56);
#[cfg(windows)]
pub const COLOR_STATUS_DETECTING: COLORREF = rgb(25, 103, 210);
#[cfg(windows)]
pub const COLOR_STATUS_OCCUPIED: COLORREF = rgb(198, 40, 40);
#[cfg(windows)]
pub const COLOR_STATUS_DISCONNECTED: COLORREF = rgb(176, 104, 0);
#[cfg(windows)]
pub const COLOR_STATUS_RELEASED: COLORREF = rgb(88, 96, 105);

#[cfg(windows)]
pub fn run_gui(manager: Arc<MonitorManager>, initial_config: AppConfig, api_port: Option<u16>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(null());
        win32_ui::window::register_window_classes(hinst);
        win32_ui::pane::register_pane_class(hinst);
        win32_ui::logview::register_logview_class(hinst);
        win32_ui::notify::register_toast_class(hinst);
        win32_ui::settings::register_settings_class(hinst);

        let _main_hwnd = win32_ui::window::create_initial_windows(
            hinst,
            manager.clone(),
            initial_config,
            api_port,
        );

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    manager.shutdown_all();
}

#[cfg(windows)]
pub fn load_initial_config() -> AppConfig {
    load_config()
}

#[cfg(target_os = "macos")]
pub use macos::{load_initial_config, run_gui};
