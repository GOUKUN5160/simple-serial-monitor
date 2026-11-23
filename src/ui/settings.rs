use crate::i18n::{lang, tr, Lang};
use crate::types::{NotifyMode, ThemeMode};
use crate::ui::{
    apply_ctrl_dark_mode, apply_window_dark_titlebar, dpi_scale, get_window_text, resolve_is_dark,
    set_window_text, to_wide, window_dpi, CtrlThemeKind, UiTheme, IDI_APP_ICON,
};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    FillRect, InvalidateRect, SetBkColor, SetTextColor, HDC, HFONT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, IsWindow, LoadCursorW, LoadImageW, MoveWindow, PostMessageW,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    BS_PUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT,
    CB_SETCURSEL, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, EN_CHANGE, ES_NUMBER, GWLP_USERDATA,
    HICON, IDC_ARROW, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, SM_CXSMICON, SM_CYSMICON, SW_SHOW,
    WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_NCDESTROY, WM_SETFONT, WNDCLASSEXW,
    WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

const SETTINGS_CLASS_NAME: &str = "SsmSettingsDialogClass";
pub const WM_SETTINGS_CHANGED: u32 = WM_APP + 50;

const ID_CMB_LANG: isize = 301;
const ID_CMB_THEME: isize = 302;
const ID_CMB_NOTIFY: isize = 303;
const ID_EDIT_API_PORT: isize = 304;
const ID_BTN_CLOSE: isize = 305;

const SS_CENTERIMAGE: u32 = 0x0200;

#[derive(Debug, Clone, Copy)]
pub struct SettingsValues {
    pub lang: Lang,
    pub theme_mode: ThemeMode,
    pub notify_mode: NotifyMode,
    pub api_port: u16,
}

thread_local! {
    static SETTINGS_HWND: RefCell<HWND> = const { RefCell::new(null_mut()) };
}

struct SettingsCreateParams {
    owner: HWND,
    values: SettingsValues,
}

struct SettingsState {
    hwnd: HWND,
    owner: HWND,
    values: SettingsValues,
    theme: UiTheme,
    updating: bool,

    lbl_lang: HWND,
    cmb_lang: HWND,
    lbl_theme: HWND,
    cmb_theme: HWND,
    lbl_notify: HWND,
    cmb_notify: HWND,
    lbl_port: HWND,
    edit_port: HWND,
    lbl_note: HWND,
    btn_close: HWND,
}

pub unsafe fn register_settings_class(hinst: HINSTANCE) {
    let icon_sm = LoadImageW(
        hinst,
        IDI_APP_ICON as *const u16,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR | LR_SHARED,
    ) as HICON;
    let cls = to_wide(SETTINGS_CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(settings_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: icon_sm,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: cls.as_ptr(),
        hIconSm: icon_sm,
    };
    RegisterClassExW(&wc);
}

pub unsafe fn open_settings_dialog(owner: HWND, hinst: HINSTANCE, values: SettingsValues) {
    let existing = SETTINGS_HWND.with(|h| *h.borrow());
    if !existing.is_null() && IsWindow(existing) != 0 {
        ShowWindow(existing, SW_SHOW);
        SetForegroundWindow(existing);
        return;
    }

    let dpi = if !owner.is_null() && IsWindow(owner) != 0 {
        window_dpi(owner)
    } else {
        96
    };
    let s = |v: i32| dpi_scale(v, dpi);
    let w = s(420);
    let h = s(255);

    let (x, y) = if !owner.is_null() && IsWindow(owner) != 0 {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(owner, &mut rc);
        (
            rc.left + ((rc.right - rc.left) - w) / 2,
            rc.top + ((rc.bottom - rc.top) - h) / 2,
        )
    } else {
        (200, 200)
    };

    let params = Box::new(SettingsCreateParams { owner, values });
    let cls = to_wide(SETTINGS_CLASS_NAME);
    let title = to_wide(tr("Settings - Simple Serial Monitor", "設定 - Simple Serial Monitor"));
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        x.max(20),
        y.max(20),
        w,
        h,
        owner,
        null_mut(),
        hinst,
        Box::into_raw(params) as *const _,
    );
    if !hwnd.is_null() {
        SETTINGS_HWND.with(|slot| *slot.borrow_mut() = hwnd);
        ShowWindow(hwnd, SW_SHOW);
    }
}

pub unsafe fn settings_get_values(hwnd: HWND) -> Option<SettingsValues> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if ptr.is_null() {
        return None;
    }
    Some((*ptr).values)
}

unsafe extern "system" fn settings_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        let params = Box::from_raw(cs.lpCreateParams as *mut SettingsCreateParams);
        let dpi = window_dpi(hwnd);
        let dark = resolve_is_dark(params.values.theme_mode);
        let theme = UiTheme::new(dpi, dark);
        let f_ui = theme.font_ui;

        let lbl_lang = create_ctrl(
            hwnd,
            cs.hInstance,
            "STATIC",
            "",
            WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE,
            0,
            f_ui,
        );
        let cmb_lang = create_ctrl(
            hwnd,
            cs.hInstance,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_LANG,
            f_ui,
        );
        let lbl_theme = create_ctrl(
            hwnd,
            cs.hInstance,
            "STATIC",
            "",
            WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE,
            0,
            f_ui,
        );
        let cmb_theme = create_ctrl(
            hwnd,
            cs.hInstance,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_THEME,
            f_ui,
        );
        let lbl_notify = create_ctrl(
            hwnd,
            cs.hInstance,
            "STATIC",
            "",
            WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE,
            0,
            f_ui,
        );
        let cmb_notify = create_ctrl(
            hwnd,
            cs.hInstance,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_NOTIFY,
            f_ui,
        );
        let lbl_port = create_ctrl(
            hwnd,
            cs.hInstance,
            "STATIC",
            "",
            WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE,
            0,
            f_ui,
        );
        let edit_port = create_ctrl(
            hwnd,
            cs.hInstance,
            "EDIT",
            &params.values.api_port.to_string(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_NUMBER as u32,
            ID_EDIT_API_PORT,
            f_ui,
        );
        let lbl_note = create_ctrl(
            hwnd,
            cs.hInstance,
            "STATIC",
            "",
            WS_CHILD | WS_VISIBLE,
            0,
            f_ui,
        );
        let btn_close = create_ctrl(
            hwnd,
            cs.hInstance,
            "BUTTON",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_CLOSE,
            f_ui,
        );

        let mut st = Box::new(SettingsState {
            hwnd,
            owner: params.owner,
            values: params.values,
            theme,
            updating: false,
            lbl_lang,
            cmb_lang,
            lbl_theme,
            cmb_theme,
            lbl_notify,
            cmb_notify,
            lbl_port,
            edit_port,
            lbl_note,
            btn_close,
        });
        st.refresh_texts();
        st.apply_dark_styles();
        st.layout();

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = &mut *ptr;

    match msg {
        WM_ERASEBKGND => {
            let hdc = wparam as HDC;
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            FillRect(hdc, &rc, st.theme.brush_bg);
            1
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, st.theme.col_fg);
            SetBkColor(hdc, st.theme.col_bg);
            st.theme.brush_bg as LRESULT
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, st.theme.col_fg);
            SetBkColor(hdc, st.theme.col_edit_bg);
            st.theme.brush_edit_bg as LRESULT
        }
        WM_COMMAND => {
            if st.updating {
                return 0;
            }
            let id = (wparam & 0xFFFF) as isize;
            let notif = ((wparam >> 16) & 0xFFFF) as u32;
            let mut changed = false;
            match id {
                ID_CMB_LANG if notif == CBN_SELCHANGE => {
                    let idx = SendMessageW(st.cmb_lang, CB_GETCURSEL, 0, 0);
                    st.values.lang = if idx == 1 { Lang::Ja } else { Lang::En };
                    crate::i18n::set_lang(st.values.lang);
                    st.refresh_texts();
                    changed = true;
                }
                ID_CMB_THEME if notif == CBN_SELCHANGE => {
                    let idx = SendMessageW(st.cmb_theme, CB_GETCURSEL, 0, 0);
                    st.values.theme_mode = match idx {
                        1 => ThemeMode::Light,
                        2 => ThemeMode::Dark,
                        _ => ThemeMode::System,
                    };
                    let dark = resolve_is_dark(st.values.theme_mode);
                    st.theme = UiTheme::new(st.theme.dpi, dark);
                    st.apply_dark_styles();
                    InvalidateRect(hwnd, null(), 1);
                    changed = true;
                }
                ID_CMB_NOTIFY if notif == CBN_SELCHANGE => {
                    let idx = SendMessageW(st.cmb_notify, CB_GETCURSEL, 0, 0);
                    st.values.notify_mode = match idx {
                        0 => NotifyMode::Off,
                        2 => NotifyMode::Windows,
                        3 => NotifyMode::Both,
                        _ => NotifyMode::InApp,
                    };
                    changed = true;
                }
                ID_EDIT_API_PORT if notif == EN_CHANGE => {
                    if let Ok(p) = get_window_text(st.edit_port).trim().parse::<u16>() {
                        if p >= 1024 {
                            st.values.api_port = p;
                            changed = true;
                        }
                    }
                }
                ID_BTN_CLOSE => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            if changed && !st.owner.is_null() && IsWindow(st.owner) != 0 {
                PostMessageW(st.owner, WM_SETTINGS_CHANGED, hwnd as WPARAM, 0);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            SETTINGS_HWND.with(|slot| *slot.borrow_mut() = null_mut());
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

impl SettingsState {
    unsafe fn refresh_texts(&mut self) {
        self.updating = true;
        set_window_text(
            self.hwnd,
            tr("Settings - Simple Serial Monitor", "設定 - Simple Serial Monitor"),
        );
        set_window_text(self.lbl_lang, tr("Language:", "言語:"));
        set_window_text(self.lbl_theme, tr("Appearance:", "外観テーマ:"));
        set_window_text(self.lbl_notify, tr("New Device Alert:", "新デバイス通知:"));
        set_window_text(self.lbl_port, tr("Agent API Port:", "Agent API ポート:"));
        set_window_text(
            self.lbl_note,
            tr(
                "* Language, Theme, and Alerts apply immediately.\n* API Port change takes effect after restart.",
                "※ 言語・テーマ・通知設定は即座に反映されます。\n※ API ポート変更は次回起動時から有効になります。",
            ),
        );
        set_window_text(self.btn_close, tr("Close", "閉じる"));

        // Language combo
        SendMessageW(self.cmb_lang, CB_RESETCONTENT, 0, 0);
        for item in ["English", "日本語"] {
            let w = to_wide(item);
            SendMessageW(self.cmb_lang, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        SendMessageW(
            self.cmb_lang,
            CB_SETCURSEL,
            if self.values.lang == Lang::Ja { 1 } else { 0 },
            0,
        );

        // Theme combo
        SendMessageW(self.cmb_theme, CB_RESETCONTENT, 0, 0);
        let themes = match lang() {
            Lang::En => ["System Default", "Light", "Dark"],
            Lang::Ja => ["システム設定に従う", "ライト", "ダーク"],
        };
        for item in themes {
            let w = to_wide(item);
            SendMessageW(self.cmb_theme, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        let tidx = match self.values.theme_mode {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        };
        SendMessageW(self.cmb_theme, CB_SETCURSEL, tidx, 0);

        // Notify combo
        SendMessageW(self.cmb_notify, CB_RESETCONTENT, 0, 0);
        let notifies = match lang() {
            Lang::En => ["Off", "In-App Popup", "Windows Notification", "Both"],
            Lang::Ja => ["OFF", "アプリ内ポップアップ", "Windows 通知", "両方"],
        };
        for item in notifies {
            let w = to_wide(item);
            SendMessageW(self.cmb_notify, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        let nidx = match self.values.notify_mode {
            NotifyMode::Off => 0,
            NotifyMode::InApp => 1,
            NotifyMode::Windows => 2,
            NotifyMode::Both => 3,
        };
        SendMessageW(self.cmb_notify, CB_SETCURSEL, nidx, 0);

        self.updating = false;
    }

    unsafe fn apply_dark_styles(&self) {
        let dark = self.theme.dark;
        apply_window_dark_titlebar(self.hwnd, dark);
        apply_ctrl_dark_mode(self.cmb_lang, dark, CtrlThemeKind::Combo);
        apply_ctrl_dark_mode(self.cmb_theme, dark, CtrlThemeKind::Combo);
        apply_ctrl_dark_mode(self.cmb_notify, dark, CtrlThemeKind::Combo);
        apply_ctrl_dark_mode(self.edit_port, dark, CtrlThemeKind::Edit);
        apply_ctrl_dark_mode(self.btn_close, dark, CtrlThemeKind::Button);
    }

    unsafe fn layout(&self) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        let s = |v: i32| dpi_scale(v, self.theme.dpi);

        let pad = s(14);
        let lbl_w = s(130);
        let ctrl_x = pad + lbl_w + s(8);
        let ctrl_w = (w - ctrl_x - pad).max(s(120));
        let row_h = s(24);
        let gap = s(10);
        let drop_h = s(180);

        let mut y = pad;
        MoveWindow(self.lbl_lang, pad, y, lbl_w, row_h, 1);
        MoveWindow(self.cmb_lang, ctrl_x, y, ctrl_w, drop_h, 1);
        y += row_h + gap;

        MoveWindow(self.lbl_theme, pad, y, lbl_w, row_h, 1);
        MoveWindow(self.cmb_theme, ctrl_x, y, ctrl_w, drop_h, 1);
        y += row_h + gap;

        MoveWindow(self.lbl_notify, pad, y, lbl_w, row_h, 1);
        MoveWindow(self.cmb_notify, ctrl_x, y, ctrl_w, drop_h, 1);
        y += row_h + gap;

        MoveWindow(self.lbl_port, pad, y, lbl_w, row_h, 1);
        MoveWindow(self.edit_port, ctrl_x, y, s(100), row_h, 1);
        y += row_h + s(12);

        MoveWindow(self.lbl_note, pad, y, w - pad * 2, s(34), 1);

        let btn_w = s(84);
        let btn_h = s(26);
        MoveWindow(self.btn_close, w - pad - btn_w, h - pad - btn_h, btn_w, btn_h, 1);
    }
}

unsafe fn create_ctrl(
    parent: HWND,
    hinst: HINSTANCE,
    class: &str,
    text: &str,
    style: u32,
    id: isize,
    font: HFONT,
) -> HWND {
    let cls = to_wide(class);
    let txt = to_wide(text);
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        txt.as_ptr(),
        style,
        0,
        0,
        10,
        10,
        parent,
        id as _,
        hinst,
        null(),
    );
    if !hwnd.is_null() {
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
    }
    hwnd
}
