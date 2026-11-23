use crate::i18n::tr;
use crate::types::{NotifyMode, PortInfo};
use crate::ui::{dpi_scale, rgb, to_wide, window_dpi, IDI_APP_ICON};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK, FF_DONTCARE, FW_BOLD, FW_NORMAL, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, IsWindow, KillTimer, LoadCursorW, LoadImageW,
    RegisterClassExW, SetTimer, SetWindowLongPtrW, ShowWindow, CREATESTRUCTW, CS_HREDRAW,
    CS_VREDRAW, GWLP_USERDATA, HICON, IDC_HAND, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED,
    SM_CXSMICON, SM_CYSMICON, SW_SHOWNOACTIVATE, WM_CREATE, WM_DESTROY, WM_LBUTTONDOWN,
    WM_NCDESTROY, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

const TOAST_CLASS_NAME: &str = "SsmToastPopupClass";
const TRAY_ICON_ID: u32 = 9001;
const TOAST_TIMER_ID: usize = 77;

struct ToastData {
    title: String,
    body: String,
    dpi: u32,
}

pub unsafe fn register_toast_class(hinst: HINSTANCE) {
    let cls = to_wide(TOAST_CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(toast_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_HAND),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: cls.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc);
}

pub unsafe fn init_tray_icon(owner: HWND, hinst: HINSTANCE) -> bool {
    let icon = LoadImageW(
        hinst,
        IDI_APP_ICON as *const u16,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR | LR_SHARED,
    ) as HICON;
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = owner;
    nid.uID = TRAY_ICON_ID;
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = icon;
    copy_wide_fixed(&mut nid.szTip, "Simple Serial Monitor");
    Shell_NotifyIconW(NIM_ADD, &nid) != 0
}

pub unsafe fn remove_tray_icon(owner: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = owner;
    nid.uID = TRAY_ICON_ID;
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

pub unsafe fn notify_new_devices(
    owner: HWND,
    hinst: HINSTANCE,
    mode: NotifyMode,
    tray_ready: bool,
    new_ports: &[PortInfo],
) {
    if new_ports.is_empty() || mode == NotifyMode::Off {
        return;
    }

    let title = if new_ports.len() == 1 {
        tr("New serial device detected", "新しいシリアルデバイスを検出").to_string()
    } else {
        match crate::i18n::lang() {
            crate::i18n::Lang::En => format!("{} new serial devices detected", new_ports.len()),
            crate::i18n::Lang::Ja => format!("新しいシリアルデバイスを {} 件検出", new_ports.len()),
        }
    };
    let body = new_ports
        .iter()
        .map(|p| p.display_label())
        .collect::<Vec<_>>()
        .join("\n");

    if matches!(mode, NotifyMode::InApp | NotifyMode::Both) {
        show_in_app_toast(owner, hinst, &title, &body);
    }
    if matches!(mode, NotifyMode::Windows | NotifyMode::Both) && tray_ready {
        show_windows_balloon(owner, &title, &body);
    }
}

unsafe fn show_windows_balloon(owner: HWND, title: &str, body: &str) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = owner;
    nid.uID = TRAY_ICON_ID;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_wide_fixed(&mut nid.szInfoTitle, title);
    copy_wide_fixed(&mut nid.szInfo, body);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn show_in_app_toast(owner: HWND, hinst: HINSTANCE, title: &str, body: &str) {
    let dpi = if !owner.is_null() && IsWindow(owner) != 0 {
        window_dpi(owner)
    } else {
        96
    };
    let s = |v: i32| dpi_scale(v, dpi);
    let w = s(340);
    let line_count = body.lines().count().clamp(1, 4) as i32;
    let h = s(44 + line_count * 18);

    let (x, y) = if !owner.is_null() && IsWindow(owner) != 0 {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(owner, &mut rc);
        (
            (rc.right - w - s(18)).max(rc.left + s(12)),
            (rc.bottom - h - s(18)).max(rc.top + s(40)),
        )
    } else {
        (80, 80)
    };

    let data = Box::new(ToastData {
        title: title.to_string(),
        body: body.to_string(),
        dpi,
    });
    let cls = to_wide(TOAST_CLASS_NAME);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        cls.as_ptr(),
        null(),
        WS_POPUP,
        x,
        y,
        w,
        h,
        owner,
        null_mut(),
        hinst,
        Box::into_raw(data) as *const _,
    );
    if !hwnd.is_null() {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

unsafe extern "system" fn toast_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        SetTimer(hwnd, TOAST_TIMER_ID, 4500, None);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ToastData;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let data = &*ptr;

    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_null() {
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let s = |v: i32| dpi_scale(v, data.dpi);

                let bg = CreateSolidBrush(rgb(24, 34, 46));
                let border = CreateSolidBrush(rgb(46, 204, 113));
                FillRect(hdc, &rc, border);

                let inner = RECT {
                    left: rc.left + s(4),
                    top: rc.top + 1,
                    right: rc.right - 1,
                    bottom: rc.bottom - 1,
                };
                FillRect(hdc, &inner, bg);

                let face = to_wide("Segoe UI");
                let f_title = CreateFontW(
                    -s(13),
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
                    face.as_ptr(),
                );
                let f_body = CreateFontW(
                    -s(12),
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
                    face.as_ptr(),
                );

                SetBkMode(hdc, TRANSPARENT as i32);

                let old_f = SelectObject(hdc, f_title);
                SetTextColor(hdc, rgb(74, 222, 128));
                let mut t_rc = RECT {
                    left: inner.left + s(10),
                    top: inner.top + s(6),
                    right: inner.right - s(10),
                    bottom: inner.top + s(24),
                };
                let t_w = to_wide(&format!("● {}", data.title));
                DrawTextW(
                    hdc,
                    t_w.as_ptr(),
                    -1,
                    &mut t_rc,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );

                SelectObject(hdc, f_body);
                SetTextColor(hdc, rgb(230, 237, 243));
                let mut b_rc = RECT {
                    left: inner.left + s(10),
                    top: inner.top + s(26),
                    right: inner.right - s(10),
                    bottom: inner.bottom - s(6),
                };
                let b_w = to_wide(&data.body);
                DrawTextW(
                    hdc,
                    b_w.as_ptr(),
                    -1,
                    &mut b_rc,
                    DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                );

                SelectObject(hdc, old_f);
                DeleteObject(f_title);
                DeleteObject(f_body);
                DeleteObject(bg);
                DeleteObject(border);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN | WM_TIMER => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TOAST_TIMER_ID);
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

fn copy_wide_fixed(dst: &mut [u16], text: &str) {
    dst.fill(0);
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let n = encoded.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&encoded[..n]);
}
