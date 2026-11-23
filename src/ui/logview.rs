use crate::types::{DisplayMode, LogEntry, LogKind};
use crate::ui::{dpi_scale, rgb, to_wide, window_dpi};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetTextMetricsW, InvalidateRect,
    ReleaseDC, SelectObject, SetBkColor, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, FF_DONTCARE, FF_MODERN, FIXED_PITCH, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TEXTMETRICW, TRANSPARENT,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_ADD, VK_CONTROL, VK_DOWN, VK_END,
    VK_HOME, VK_LEFT, VK_NEXT, VK_NUMPAD0, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RIGHT,
    VK_SUBTRACT, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetParent, GetScrollInfo, GetWindowLongPtrW,
    LoadCursorW, PostMessageW, RegisterClassExW, SetWindowLongPtrW, CREATESTRUCTW, CS_DBLCLKS,
    CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_IBEAM, SB_BOTTOM, SB_HORZ, SB_LINEDOWN, SB_LINELEFT,
    SB_LINERIGHT, SB_LINEUP, SB_PAGEDOWN, SB_PAGELEFT, SB_PAGERIGHT, SB_PAGEUP, SB_THUMBPOSITION,
    SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_ALL, SIF_TRACKPOS, WM_APP, WM_CREATE,
    WM_ERASEBKGND, WM_HSCROLL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_SIZE, WM_VSCROLL, WNDCLASSEXW,
    WS_BORDER, WS_CHILD, WS_HSCROLL, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

pub const LOGVIEW_CLASS_NAME: &str = "SsmLogViewClass";
pub const WM_LOGVIEW_AUTOSCROLL_CHANGED: u32 = WM_APP + 40;
pub const WM_LOGVIEW_ZOOM_CHANGED: u32 = WM_APP + 41;

const MAX_STORED_ENTRIES: usize = 8000;
const CF_UNICODETEXT: u32 = 13;
const MK_SHIFT: usize = 0x0004;
const MK_CONTROL: usize = 0x0008;

// Color palette (clean dark terminal, distinct hues per category)
const COL_BG: COLORREF = rgb(22, 25, 31);
const COL_RX_TEXT: COLORREF = rgb(230, 237, 243);
const COL_TX_TEXT: COLORREF = rgb(56, 232, 255);      // Bright Cyan for TX
const COL_TX_ACCENT: COLORREF = rgb(6, 182, 212);
const COL_TS_TEXT: COLORREF = rgb(125, 133, 144);
const COL_TS_BG: COLORREF = rgb(31, 36, 45);
const COL_SEL_BG: COLORREF = rgb(38, 79, 120);

const COL_SYS_CONN_BG: COLORREF = rgb(18, 44, 29);     // Emerald Green for Connect
const COL_SYS_CONN_FG: COLORREF = rgb(74, 222, 128);
const COL_SYS_CONN_BAR: COLORREF = rgb(34, 197, 94);

const COL_SYS_DISC_BG: COLORREF = rgb(48, 33, 14);     // Warm Amber for Disconnect
const COL_SYS_DISC_FG: COLORREF = rgb(245, 193, 86);
const COL_SYS_DISC_BAR: COLORREF = rgb(217, 151, 38);

const COL_SYS_ERR_BG: COLORREF = rgb(56, 21, 25);      // Coral Red for Occupied / Error
const COL_SYS_ERR_FG: COLORREF = rgb(255, 123, 114);
const COL_SYS_ERR_BAR: COLORREF = rgb(239, 68, 68);

const COL_SYS_INFO_BG: COLORREF = rgb(37, 27, 56);     // Soft Violet / Purple for Auto-detect / SysInfo
const COL_SYS_INFO_FG: COLORREF = rgb(196, 155, 255);
const COL_SYS_INFO_BAR: COLORREF = rgb(157, 107, 255);

struct LogViewCreateParams {
    auto_scroll: bool,
    show_timestamps: bool,
    word_wrap: bool,
    display_mode: DisplayMode,
    zoom_percent: u32,
}

/// One visual row on screen mapping back to a logical LogEntry slice.
#[derive(Clone)]
struct VisualRow {
    entry_idx: usize,
    is_first: bool,
    text: String,
    cols: usize,
}

struct LogViewState {
    hwnd: HWND,
    dpi: u32,
    zoom_percent: u32,
    auto_scroll: bool,
    show_timestamps: bool,
    word_wrap: bool,
    display_mode: DisplayMode,

    entries: Vec<LogEntry>,
    rows: Vec<VisualRow>,
    max_cols: usize,

    scroll_y: usize,
    scroll_x: i32, // in pixels

    client_w: i32,
    client_h: i32,
    row_h: i32,
    char_w: i32,

    // Selection (in entry indices)
    selecting: bool,
    sel_anchor: Option<usize>,
    sel_active: Option<usize>,

    // GDI resources
    font_mono: HFONT,
    font_ts: HFONT,
    font_sys: HFONT,
    brush_bg: HBRUSH,
    brush_sel: HBRUSH,
    brush_ts_bg: HBRUSH,
    brush_tx_accent: HBRUSH,
    brush_sys_conn_bg: HBRUSH,
    brush_sys_conn_bar: HBRUSH,
    brush_sys_disc_bg: HBRUSH,
    brush_sys_disc_bar: HBRUSH,
    brush_sys_err_bg: HBRUSH,
    brush_sys_err_bar: HBRUSH,
    brush_sys_info_bg: HBRUSH,
    brush_sys_info_bar: HBRUSH,
}

pub unsafe fn register_logview_class(hinst: HINSTANCE) {
    let cls = to_wide(LOGVIEW_CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(logview_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_IBEAM),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: cls.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc);
}

pub unsafe fn create_logview(
    parent: HWND,
    hinst: HINSTANCE,
    auto_scroll: bool,
    show_timestamps: bool,
    word_wrap: bool,
    display_mode: DisplayMode,
    zoom_percent: u32,
) -> HWND {
    let params = Box::new(LogViewCreateParams {
        auto_scroll,
        show_timestamps,
        word_wrap,
        display_mode,
        zoom_percent: zoom_percent.clamp(50, 300),
    });
    let cls = to_wide(LOGVIEW_CLASS_NAME);
    CreateWindowExW(
        0,
        cls.as_ptr(),
        null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | WS_VSCROLL | WS_HSCROLL,
        0,
        0,
        100,
        100,
        parent,
        null_mut(),
        hinst,
        Box::into_raw(params) as *const _,
    )
}

pub unsafe fn logview_append_entries(hwnd: HWND, new_entries: &[LogEntry]) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).append_entries(new_entries);
    }
}

pub unsafe fn logview_clear(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).clear();
    }
}

pub unsafe fn logview_set_auto_scroll(hwnd: HWND, enabled: bool) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).set_auto_scroll_from_ui(enabled);
    }
}

pub unsafe fn logview_set_show_timestamps(hwnd: HWND, show: bool) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).show_timestamps = show;
        (*ptr).rebuild_visual_rows();
    }
}

pub unsafe fn logview_set_word_wrap(hwnd: HWND, wrap: bool) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).word_wrap = wrap;
        if wrap {
            (*ptr).scroll_x = 0;
        }
        (*ptr).rebuild_visual_rows();
    }
}

pub unsafe fn logview_set_display_mode(hwnd: HWND, mode: DisplayMode) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        if (*ptr).display_mode != mode {
            (*ptr).display_mode = mode;
            (*ptr).scroll_x = 0;
            (*ptr).rebuild_visual_rows();
        }
    }
}

pub unsafe fn logview_set_dpi(hwnd: HWND, dpi: u32) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        let s = &mut *ptr;
        if s.dpi != dpi {
            s.dpi = dpi;
            s.recreate_fonts();
            s.rebuild_visual_rows();
        }
    }
}

pub unsafe fn logview_adjust_zoom(hwnd: HWND, delta_percent: i32) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).adjust_zoom(delta_percent);
    }
}

pub unsafe fn logview_reset_zoom(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if !ptr.is_null() {
        (*ptr).set_zoom(100);
    }
}

unsafe extern "system" fn logview_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        let params = Box::from_raw(cs.lpCreateParams as *mut LogViewCreateParams);
        let dpi = window_dpi(hwnd);
        let mut st = Box::new(LogViewState::new(hwnd, dpi, *params));
        st.recreate_fonts();
        let ptr = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LogViewState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = &mut *ptr;

    match msg {
        WM_ERASEBKGND => 1, // Handled cleanly in double-buffered WM_PAINT
        WM_SIZE => {
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            let new_w = (rc.right - rc.left).max(1);
            let new_h = (rc.bottom - rc.top).max(1);
            let width_changed = new_w != st.client_w;
            st.client_w = new_w;
            st.client_h = new_h;
            if st.word_wrap && width_changed {
                st.rebuild_visual_rows();
            } else {
                st.clamp_scroll_and_update_bars();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_PAINT => {
            st.paint();
            0
        }
        WM_VSCROLL => {
            st.handle_vscroll(wparam);
            0
        }
        WM_HSCROLL => {
            st.handle_hscroll(wparam);
            0
        }
        WM_MOUSEWHEEL => {
            let fw_keys = wparam & 0xFFFF;
            let delta = ((wparam >> 16) as u16) as i16;
            if (fw_keys & MK_CONTROL) != 0 {
                st.adjust_zoom(if delta > 0 { 10 } else { -10 });
            } else if (fw_keys & MK_SHIFT) != 0 {
                if !st.word_wrap {
                    let step = st.char_w * 6;
                    st.scroll_horizontal(if delta > 0 { -step } else { step });
                }
            } else {
                let lines = if delta > 0 { -3i32 } else { 3i32 };
                st.scroll_vertical_delta(lines);
            }
            0
        }
        WM_MOUSEHWHEEL => {
            if !st.word_wrap {
                let delta = ((wparam >> 16) as u16) as i16;
                let step = st.char_w * 6;
                st.scroll_horizontal(if delta > 0 { step } else { -step });
            }
            0
        }
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
            let y = ((lparam >> 16) as u16) as i16 as i32;
            if let Some(entry_idx) = st.hit_test_entry(y) {
                st.sel_anchor = Some(entry_idx);
                st.sel_active = Some(entry_idx);
                st.selecting = true;
                SetCapture(hwnd);
            } else {
                st.sel_anchor = None;
                st.sel_active = None;
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            if st.selecting {
                let y = ((lparam >> 16) as u16) as i16 as i32;
                if let Some(entry_idx) = st.hit_test_entry(y) {
                    if st.sel_active != Some(entry_idx) {
                        st.sel_active = Some(entry_idx);
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if st.selecting {
                st.selecting = false;
                ReleaseCapture();
            }
            0
        }
        WM_KEYDOWN => {
            let vk = wparam as u16;
            let ctrl = (GetKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
            if ctrl {
                match vk {
                    0x43 => st.copy_selection_to_clipboard(), // Ctrl+C
                    0x41 => {
                        // Ctrl+A
                        if !st.entries.is_empty() {
                            st.sel_anchor = Some(0);
                            st.sel_active = Some(st.entries.len() - 1);
                            InvalidateRect(hwnd, null(), 0);
                        }
                    }
                    k if k == VK_OEM_PLUS || k == VK_ADD => st.adjust_zoom(10),
                    k if k == VK_OEM_MINUS || k == VK_SUBTRACT => st.adjust_zoom(-10),
                    0x30 | VK_NUMPAD0 => st.set_zoom(100),
                    _ => {}
                }
            } else {
                match vk {
                    VK_UP => st.scroll_vertical_delta(-1),
                    VK_DOWN => st.scroll_vertical_delta(1),
                    VK_PRIOR => st.scroll_vertical_delta(-(st.visible_rows().max(1) as i32)),
                    VK_NEXT => st.scroll_vertical_delta(st.visible_rows().max(1) as i32),
                    VK_HOME => st.scroll_vertical_to(0, true),
                    VK_END => st.scroll_vertical_to(st.max_scroll_y(), true),
                    VK_LEFT if !st.word_wrap => st.scroll_horizontal(-st.char_w * 4),
                    VK_RIGHT if !st.word_wrap => st.scroll_horizontal(st.char_w * 4),
                    _ => {}
                }
            }
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl LogViewState {
    unsafe fn new(hwnd: HWND, dpi: u32, p: LogViewCreateParams) -> Self {
        Self {
            hwnd,
            dpi,
            zoom_percent: p.zoom_percent,
            auto_scroll: p.auto_scroll,
            show_timestamps: p.show_timestamps,
            word_wrap: p.word_wrap,
            display_mode: p.display_mode,
            entries: Vec::with_capacity(512),
            rows: Vec::with_capacity(512),
            max_cols: 0,
            scroll_y: 0,
            scroll_x: 0,
            client_w: 400,
            client_h: 200,
            row_h: 18,
            char_w: 8,
            selecting: false,
            sel_anchor: None,
            sel_active: None,
            font_mono: null_mut(),
            font_ts: null_mut(),
            font_sys: null_mut(),
            brush_bg: CreateSolidBrush(COL_BG),
            brush_sel: CreateSolidBrush(COL_SEL_BG),
            brush_ts_bg: CreateSolidBrush(COL_TS_BG),
            brush_tx_accent: CreateSolidBrush(COL_TX_ACCENT),
            brush_sys_conn_bg: CreateSolidBrush(COL_SYS_CONN_BG),
            brush_sys_conn_bar: CreateSolidBrush(COL_SYS_CONN_BAR),
            brush_sys_disc_bg: CreateSolidBrush(COL_SYS_DISC_BG),
            brush_sys_disc_bar: CreateSolidBrush(COL_SYS_DISC_BAR),
            brush_sys_err_bg: CreateSolidBrush(COL_SYS_ERR_BG),
            brush_sys_err_bar: CreateSolidBrush(COL_SYS_ERR_BAR),
            brush_sys_info_bg: CreateSolidBrush(COL_SYS_INFO_BG),
            brush_sys_info_bar: CreateSolidBrush(COL_SYS_INFO_BAR),
        }
    }

    unsafe fn recreate_fonts(&mut self) {
        if !self.font_mono.is_null() {
            DeleteObject(self.font_mono);
        }
        if !self.font_ts.is_null() {
            DeleteObject(self.font_ts);
        }
        if !self.font_sys.is_null() {
            DeleteObject(self.font_sys);
        }

        let effective_dpi = (self.dpi * self.zoom_percent) / 100;
        let h_mono = -dpi_scale(14, effective_dpi).max(7);
        let h_ts = -dpi_scale(12, effective_dpi).max(6);
        let h_sys = -dpi_scale(12, effective_dpi).max(6);

        let mono_face = to_wide("Consolas");
        let ui_face = to_wide("Segoe UI");

        self.font_mono = CreateFontW(
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
        self.font_ts = CreateFontW(
            h_ts,
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
        self.font_sys = CreateFontW(
            h_sys,
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

        let hdc = GetDC(self.hwnd);
        if !hdc.is_null() {
            let old = SelectObject(hdc, self.font_mono);
            let mut tm: TEXTMETRICW = std::mem::zeroed();
            if GetTextMetricsW(hdc, &mut tm) != 0 {
                self.row_h = (tm.tmHeight + tm.tmExternalLeading + dpi_scale(4, self.dpi)).max(12);
                self.char_w = tm.tmAveCharWidth.max(5);
            }
            SelectObject(hdc, old);
            ReleaseDC(self.hwnd, hdc);
        }
    }

    fn visible_rows(&self) -> usize {
        ((self.client_h / self.row_h.max(1)).max(1)) as usize
    }

    fn max_scroll_y(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows())
    }

    fn pad_x(&self) -> i32 {
        dpi_scale(8, self.dpi)
    }

    fn ts_area_px(&self) -> i32 {
        if self.show_timestamps {
            // "[HH:MM:SS.mmm] " ~ 15 chars
            self.char_w * 14 + dpi_scale(10, self.dpi)
        } else {
            0
        }
    }

    fn max_wrap_cols(&self) -> usize {
        let usable = self.client_w - self.pad_x() * 2 - self.ts_area_px() - self.char_w * 2;
        (usable / self.char_w.max(1)).max(16) as usize
    }

    unsafe fn append_entries(&mut self, new_entries: &[LogEntry]) {
        if new_entries.is_empty() {
            return;
        }
        let mut trimmed = false;
        for e in new_entries {
            self.entries.push(e.clone());
        }
        if self.entries.len() > MAX_STORED_ENTRIES {
            let drop_count = self.entries.len() - MAX_STORED_ENTRIES;
            self.entries.drain(0..drop_count);
            self.sel_anchor = None;
            self.sel_active = None;
            trimmed = true;
        }

        if trimmed {
            self.rebuild_visual_rows();
            return;
        }

        let start_idx = self.entries.len() - new_entries.len();
        let wrap_cols = self.max_wrap_cols();
        for idx in start_idx..self.entries.len() {
            self.push_visual_rows_for_entry(idx, wrap_cols);
        }

        if self.auto_scroll {
            self.scroll_y = self.max_scroll_y();
        }
        self.clamp_scroll_and_update_bars();
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn clear(&mut self) {
        self.entries.clear();
        self.rows.clear();
        self.max_cols = 0;
        self.scroll_y = 0;
        self.scroll_x = 0;
        self.sel_anchor = None;
        self.sel_active = None;
        self.clamp_scroll_and_update_bars();
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn set_auto_scroll_from_ui(&mut self, enabled: bool) {
        self.auto_scroll = enabled;
        if enabled {
            self.scroll_y = self.max_scroll_y();
            self.clamp_scroll_and_update_bars();
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    unsafe fn set_auto_scroll_internal(&mut self, enabled: bool) {
        if self.auto_scroll != enabled {
            self.auto_scroll = enabled;
            let parent = GetParent(self.hwnd);
            if !parent.is_null() {
                PostMessageW(
                    parent,
                    WM_LOGVIEW_AUTOSCROLL_CHANGED,
                    if enabled { 1 } else { 0 },
                    0,
                );
            }
        }
    }

    unsafe fn adjust_zoom(&mut self, delta: i32) {
        let next = (self.zoom_percent as i32 + delta).clamp(50, 300) as u32;
        self.set_zoom(next);
    }

    unsafe fn set_zoom(&mut self, zoom: u32) {
        let clamped = zoom.clamp(50, 300);
        if clamped != self.zoom_percent {
            self.zoom_percent = clamped;
            self.recreate_fonts();
            self.rebuild_visual_rows();
            let parent = GetParent(self.hwnd);
            if !parent.is_null() {
                PostMessageW(parent, WM_LOGVIEW_ZOOM_CHANGED, clamped as WPARAM, 0);
            }
        }
    }

    unsafe fn rebuild_visual_rows(&mut self) {
        let pinned = self.auto_scroll;
        let old_top_entry = self.rows.get(self.scroll_y).map(|r| r.entry_idx);
        self.rows.clear();
        self.max_cols = 0;
        let wrap_cols = self.max_wrap_cols();

        for idx in 0..self.entries.len() {
            self.push_visual_rows_for_entry(idx, wrap_cols);
        }

        if pinned {
            self.scroll_y = self.max_scroll_y();
        } else if let Some(ei) = old_top_entry {
            if let Some(pos) = self.rows.iter().position(|r| r.entry_idx >= ei) {
                self.scroll_y = pos.min(self.max_scroll_y());
            } else {
                self.scroll_y = self.max_scroll_y();
            }
        }
        self.clamp_scroll_and_update_bars();
        InvalidateRect(self.hwnd, null(), 0);
    }

    fn push_visual_rows_for_entry(&mut self, entry_idx: usize, wrap_cols: usize) {
        let entry = &self.entries[entry_idx];
        if entry.kind.is_system() {
            // System banners always render as a single centered visual row
            self.rows.push(VisualRow {
                entry_idx,
                is_first: true,
                text: entry.text.clone(),
                cols: display_width(&entry.text),
            });
            return;
        }

        let formatted = entry.format_for_mode(self.display_mode);

        if !self.word_wrap {
            let cols = display_width(&formatted) + if entry.kind == LogKind::Tx { 2 } else { 0 };
            if cols > self.max_cols {
                self.max_cols = cols;
            }
            self.rows.push(VisualRow {
                entry_idx,
                is_first: true,
                text: formatted,
                cols,
            });
            return;
        }

        // Word-wrap mode: split text into visual slices fitting wrap_cols
        let mut cur = String::new();
        let mut cur_w = 0usize;
        let mut first = true;
        for ch in formatted.chars() {
            let cw = char_display_width(ch);
            if cur_w + cw > wrap_cols && !cur.is_empty() {
                self.rows.push(VisualRow {
                    entry_idx,
                    is_first: first,
                    text: std::mem::take(&mut cur),
                    cols: cur_w,
                });
                first = false;
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += cw;
        }
        self.rows.push(VisualRow {
            entry_idx,
            is_first: first,
            text: cur,
            cols: cur_w,
        });
    }

    unsafe fn scroll_vertical_delta(&mut self, delta: i32) {
        let max_y = self.max_scroll_y();
        let new_y = if delta < 0 {
            self.scroll_y.saturating_sub((-delta) as usize)
        } else {
            (self.scroll_y + delta as usize).min(max_y)
        };
        self.scroll_vertical_to(new_y, true);
    }

    unsafe fn scroll_vertical_to(&mut self, target_y: usize, user_initiated: bool) {
        let max_y = self.max_scroll_y();
        let clamped = target_y.min(max_y);
        let changed = clamped != self.scroll_y;
        self.scroll_y = clamped;

        if user_initiated {
            // Auto-enable auto-scroll at bottom; auto-disable when scrolled up!
            if clamped >= max_y {
                self.set_auto_scroll_internal(true);
            } else {
                self.set_auto_scroll_internal(false);
            }
        }

        if changed {
            self.clamp_scroll_and_update_bars();
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    unsafe fn scroll_horizontal(&mut self, delta_px: i32) {
        let max_x = self.max_scroll_x();
        let next = (self.scroll_x + delta_px).clamp(0, max_x);
        if next != self.scroll_x {
            self.scroll_x = next;
            self.clamp_scroll_and_update_bars();
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    fn content_width_px(&self) -> i32 {
        if self.word_wrap {
            self.client_w
        } else {
            self.pad_x() * 2 + self.ts_area_px() + (self.max_cols as i32 + 4) * self.char_w
        }
    }

    fn max_scroll_x(&self) -> i32 {
        if self.word_wrap {
            0
        } else {
            (self.content_width_px() - self.client_w).max(0)
        }
    }

    unsafe fn clamp_scroll_and_update_bars(&mut self) {
        let max_y = self.max_scroll_y();
        if self.auto_scroll {
            self.scroll_y = max_y;
        } else if self.scroll_y > max_y {
            self.scroll_y = max_y;
        }

        let vis = self.visible_rows();
        let si_v = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            nMin: 0,
            nMax: self.rows.len().saturating_sub(1) as i32,
            nPage: vis as u32,
            nPos: self.scroll_y as i32,
            nTrackPos: 0,
        };
        SetScrollInfo(self.hwnd, SB_VERT, &si_v, 1);

        let max_x = self.max_scroll_x();
        self.scroll_x = self.scroll_x.clamp(0, max_x);
        let si_h = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            nMin: 0,
            nMax: if self.word_wrap || max_x == 0 {
                0
            } else {
                self.content_width_px().saturating_sub(1)
            },
            nPage: if self.word_wrap || max_x == 0 {
                0
            } else {
                self.client_w.max(1) as u32
            },
            nPos: self.scroll_x,
            nTrackPos: 0,
        };
        SetScrollInfo(self.hwnd, SB_HORZ, &si_h, 1);
    }

    unsafe fn handle_vscroll(&mut self, wparam: WPARAM) {
        let code = (wparam & 0xFFFF) as i32;
        let mut si: SCROLLINFO = std::mem::zeroed();
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_ALL | SIF_TRACKPOS;
        GetScrollInfo(self.hwnd, SB_VERT, &mut si);

        let vis = self.visible_rows();
        let max_y = self.max_scroll_y();
        let target = match code {
            SB_TOP => 0,
            SB_BOTTOM => max_y,
            SB_LINEUP => self.scroll_y.saturating_sub(1),
            SB_LINEDOWN => (self.scroll_y + 1).min(max_y),
            SB_PAGEUP => self.scroll_y.saturating_sub(vis),
            SB_PAGEDOWN => (self.scroll_y + vis).min(max_y),
            SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos.max(0) as usize,
            _ => return,
        };
        self.scroll_vertical_to(target, true);
    }

    unsafe fn handle_hscroll(&mut self, wparam: WPARAM) {
        if self.word_wrap {
            return;
        }
        let code = (wparam & 0xFFFF) as i32;
        let mut si: SCROLLINFO = std::mem::zeroed();
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_ALL | SIF_TRACKPOS;
        GetScrollInfo(self.hwnd, SB_HORZ, &mut si);

        let max_x = self.max_scroll_x();
        let step = self.char_w * 4;
        let next = match code {
            SB_LINELEFT => (self.scroll_x - step).max(0),
            SB_LINERIGHT => (self.scroll_x + step).min(max_x),
            SB_PAGELEFT => (self.scroll_x - self.client_w / 2).max(0),
            SB_PAGERIGHT => (self.scroll_x + self.client_w / 2).min(max_x),
            SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos.clamp(0, max_x),
            _ => return,
        };
        if next != self.scroll_x {
            self.scroll_x = next;
            self.clamp_scroll_and_update_bars();
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    fn hit_test_entry(&self, y: i32) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let row_offset = (y / self.row_h.max(1)).max(0) as usize;
        let idx = (self.scroll_y + row_offset).min(self.rows.len() - 1);
        Some(self.rows[idx].entry_idx)
    }

    fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.sel_anchor?;
        let b = self.sel_active?;
        Some((a.min(b), a.max(b)))
    }

    unsafe fn copy_selection_to_clipboard(&self) {
        let Some((lo, hi)) = self.selected_range() else {
            return;
        };
        let mut out = String::new();
        for idx in lo..=hi.min(self.entries.len().saturating_sub(1)) {
            let e = &self.entries[idx];
            if self.show_timestamps || e.kind.is_system() {
                out.push('[');
                out.push_str(&e.datetime_str);
                out.push_str("] ");
            }
            if e.kind == LogKind::Tx {
                out.push_str("» ");
            }
            out.push_str(&e.format_for_mode(self.display_mode));
            out.push_str("\r\n");
        }
        let wide = to_wide(&out);
        if OpenClipboard(self.hwnd) != 0 {
            EmptyClipboard();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if !hmem.is_null() {
                let dst = GlobalLock(hmem) as *mut u16;
                if !dst.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                    GlobalUnlock(hmem);
                    SetClipboardData(CF_UNICODETEXT, hmem);
                }
            }
            CloseClipboard();
        }
    }

    unsafe fn paint(&self) {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(self.hwnd, &mut ps);
        if hdc.is_null() {
            return;
        }

        let w = self.client_w.max(1);
        let h = self.client_h.max(1);

        // Double-buffer offscreen bitmap for zero flicker
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem_dc, mem_bmp);

        let full_rc = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(mem_dc, &full_rc, self.brush_bg);
        SetBkMode(mem_dc, TRANSPARENT as i32);

        let sel = self.selected_range();
        let vis = self.visible_rows() + 1;
        let end_row = (self.scroll_y + vis).min(self.rows.len());
        let pad_x = self.pad_x();
        let ts_px = self.ts_area_px();

        for r_idx in self.scroll_y..end_row {
            let y = ((r_idx - self.scroll_y) as i32) * self.row_h;
            let row = &self.rows[r_idx];
            let entry = &self.entries[row.entry_idx];
            let is_sel = sel
                .map(|(lo, hi)| row.entry_idx >= lo && row.entry_idx <= hi)
                .unwrap_or(false);

            let row_rc = RECT {
                left: 0,
                top: y,
                right: w,
                bottom: y + self.row_h,
            };

            if entry.kind.is_system() {
                self.paint_system_row(mem_dc, &row_rc, entry, is_sel);
            } else {
                self.paint_data_row(mem_dc, &row_rc, row, entry, is_sel, pad_x, ts_px);
            }
        }

        // Subtle zoom indicator pill in top-right when zoom != 100%
        if self.zoom_percent != 100 {
            let badge_w = dpi_scale(54, self.dpi);
            let badge_h = dpi_scale(18, self.dpi);
            let mut brc = RECT {
                left: w - badge_w - dpi_scale(6, self.dpi),
                top: dpi_scale(4, self.dpi),
                right: w - dpi_scale(6, self.dpi),
                bottom: dpi_scale(4, self.dpi) + badge_h,
            };
            FillRect(mem_dc, &brc, self.brush_sys_info_bg);
            let old_f = SelectObject(mem_dc, self.font_sys);
            SetTextColor(mem_dc, COL_SYS_INFO_FG);
            let ztxt = to_wide(&format!("{}%", self.zoom_percent));
            DrawTextW(
                mem_dc,
                ztxt.as_ptr(),
                -1,
                &mut brc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(mem_dc, old_f);
        }

        BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
        SelectObject(mem_dc, old_bmp);
        DeleteObject(mem_bmp);
        DeleteDC(mem_dc);
        EndPaint(self.hwnd, &ps);
    }

    unsafe fn paint_system_row(&self, hdc: HDC, row_rc: &RECT, entry: &LogEntry, is_sel: bool) {
        let (bg_brush, bar_brush, fg_col, symbol) = match entry.kind {
            LogKind::SysConnect => (
                self.brush_sys_conn_bg,
                self.brush_sys_conn_bar,
                COL_SYS_CONN_FG,
                "●",
            ),
            LogKind::SysDisconnect => (
                self.brush_sys_disc_bg,
                self.brush_sys_disc_bar,
                COL_SYS_DISC_FG,
                "▲",
            ),
            LogKind::SysError => (
                self.brush_sys_err_bg,
                self.brush_sys_err_bar,
                COL_SYS_ERR_FG,
                "■",
            ),
            _ => (
                self.brush_sys_info_bg,
                self.brush_sys_info_bar,
                COL_SYS_INFO_FG,
                "◆",
            ),
        };

        let inset_y = dpi_scale(1, self.dpi).max(1);
        let inset_x = dpi_scale(6, self.dpi);
        let banner_rc = RECT {
            left: row_rc.left + inset_x,
            top: row_rc.top + inset_y,
            right: (row_rc.right - inset_x).max(row_rc.left + inset_x + 10),
            bottom: (row_rc.bottom - inset_y).max(row_rc.top + inset_y + 2),
        };

        FillRect(hdc, &banner_rc, if is_sel { self.brush_sel } else { bg_brush });

        // Left & right accent strips on the centered system banner
        let bar_w = dpi_scale(3, self.dpi).max(2);
        let left_bar = RECT {
            left: banner_rc.left,
            top: banner_rc.top,
            right: banner_rc.left + bar_w,
            bottom: banner_rc.bottom,
        };
        let right_bar = RECT {
            left: banner_rc.right - bar_w,
            top: banner_rc.top,
            right: banner_rc.right,
            bottom: banner_rc.bottom,
        };
        FillRect(hdc, &left_bar, bar_brush);
        FillRect(hdc, &right_bar, bar_brush);

        // Trim milliseconds from datetime_str for clean centered system banner (YYYY-MM-DD HH:MM:SS)
        let short_dt = entry.datetime_str.get(..19).unwrap_or(&entry.datetime_str);
        let label = format!("──  {}  {}  {}  ──", symbol, short_dt, entry.text);
        let wide = to_wide(&label);

        let old_f = SelectObject(hdc, self.font_sys);
        SetTextColor(hdc, fg_col);
        let mut text_rc = banner_rc;
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            &mut text_rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old_f);
    }

    unsafe fn paint_data_row(
        &self,
        hdc: HDC,
        row_rc: &RECT,
        row: &VisualRow,
        entry: &LogEntry,
        is_sel: bool,
        pad_x: i32,
        ts_px: i32,
    ) {
        if is_sel {
            FillRect(hdc, row_rc, self.brush_sel);
        }

        // Subtle cyan left accent bar on TX lines
        if entry.kind == LogKind::Tx {
            let bar = RECT {
                left: 0,
                top: row_rc.top + 1,
                right: dpi_scale(3, self.dpi).max(2),
                bottom: row_rc.bottom - 1,
            };
            FillRect(hdc, &bar, self.brush_tx_accent);
        }

        let base_x = pad_x - self.scroll_x;

        // Optional muted timestamp badge on first visual line of each entry
        if self.show_timestamps && row.is_first {
            let ts_rc = RECT {
                left: base_x,
                top: row_rc.top + 1,
                right: base_x + ts_px - dpi_scale(6, self.dpi),
                bottom: row_rc.bottom - 1,
            };
            if !is_sel {
                FillRect(hdc, &ts_rc, self.brush_ts_bg);
            }
            let old_f = SelectObject(hdc, self.font_ts);
            SetTextColor(hdc, COL_TS_TEXT);
            let ts_wide = to_wide(&entry.time_str);
            let mut tr = ts_rc;
            DrawTextW(
                hdc,
                ts_wide.as_ptr(),
                -1,
                &mut tr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(hdc, old_f);
        }

        let text_x = base_x + ts_px;
        let old_f = SelectObject(hdc, self.font_mono);
        SetTextColor(
            hdc,
            if entry.kind == LogKind::Tx {
                COL_TX_TEXT
            } else {
                COL_RX_TEXT
            },
        );
        SetBkColor(hdc, COL_BG);

        let display_str = if entry.kind == LogKind::Tx && row.is_first {
            format!("» {}", row.text)
        } else {
            row.text.clone()
        };
        let wide = to_wide(&display_str);
        let mut tr = RECT {
            left: text_x,
            top: row_rc.top,
            right: (text_x + (row.cols as i32 + 12) * self.char_w).max(row_rc.right),
            bottom: row_rc.bottom,
        };
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            &mut tr,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old_f);
    }
}

impl Drop for LogViewState {
    fn drop(&mut self) {
        unsafe {
            if !self.font_mono.is_null() {
                DeleteObject(self.font_mono);
            }
            if !self.font_ts.is_null() {
                DeleteObject(self.font_ts);
            }
            if !self.font_sys.is_null() {
                DeleteObject(self.font_sys);
            }
            DeleteObject(self.brush_bg);
            DeleteObject(self.brush_sel);
            DeleteObject(self.brush_ts_bg);
            DeleteObject(self.brush_tx_accent);
            DeleteObject(self.brush_sys_conn_bg);
            DeleteObject(self.brush_sys_conn_bar);
            DeleteObject(self.brush_sys_disc_bg);
            DeleteObject(self.brush_sys_disc_bar);
            DeleteObject(self.brush_sys_err_bg);
            DeleteObject(self.brush_sys_err_bar);
            DeleteObject(self.brush_sys_info_bg);
            DeleteObject(self.brush_sys_info_bar);
        }
    }
}

fn char_display_width(ch: char) -> usize {
    let cp = ch as u32;
    if cp < 0x80 {
        1
    } else if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE10..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x20000..=0x2FFFD).contains(&cp)
    {
        2
    } else {
        1
    }
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_display_width).sum()
}
