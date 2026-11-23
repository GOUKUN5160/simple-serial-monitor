use crate::i18n::{lang, tr, Lang};
use crate::manager::MonitorManager;
use crate::serial::PortSession;
use crate::types::{
    ConnectionState, DisplayMode, LineEnding, PaneConfig, Parity, PortInfo, SerialConfig, StopBits,
};
use crate::ui::logview::{
    create_logview, logview_adjust_zoom, logview_append_entries, logview_clear,
    logview_reset_zoom, logview_set_auto_scroll, logview_set_dark_mode, logview_set_display_mode,
    logview_set_dpi, logview_set_show_timestamps, logview_set_word_wrap,
    WM_LOGVIEW_AUTOSCROLL_CHANGED, WM_LOGVIEW_ZOOM_CHANGED,
};
use crate::ui::tooltip::{add_combo_tooltip, add_tooltip, create_tooltip_window};
use crate::ui::{
    apply_ctrl_dark_mode, dpi_scale, get_window_text, set_window_text, to_wide, CtrlThemeKind,
    UiTheme, COLOR_STATUS_CONNECTED, COLOR_STATUS_DETECTING, COLOR_STATUS_DISCONNECTED,
    COLOR_STATUS_FG, COLOR_STATUS_OCCUPIED, COLOR_STATUS_RELEASED,
};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    FillRect, InvalidateRect, SetBkColor, SetTextColor, COLOR_BTNFACE, HDC, HFONT,
};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetKeyState, SetFocus, VK_ADD, VK_CONTROL, VK_DOWN, VK_NUMPAD0, VK_OEM_MINUS,
    VK_OEM_PLUS, VK_RETURN, VK_SUBTRACT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetParent, GetWindowLongPtrW,
    KillTimer, MoveWindow, PostMessageW, RegisterClassExW, SendMessageW, SetTimer,
    SetWindowLongPtrW, ShowWindow, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON,
    CBN_EDITCHANGE, CBN_SELCHANGE, CBS_DROPDOWN, CBS_DROPDOWNLIST, CB_ADDSTRING, CB_DELETESTRING,
    CB_GETCURSEL, CB_GETDROPPEDSTATE, CB_INSERTSTRING, CB_RESETCONTENT, CB_SETCURSEL,
    CREATESTRUCTW, CW_USEDEFAULT, ES_AUTOHSCROLL, GWLP_USERDATA, SW_HIDE, SW_SHOW, WM_APP, WM_CHAR,
    WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_NCDESTROY, WM_SETFONT, WM_SIZE, WM_TIMER,
    WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_CLIPSIBLINGS, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const SS_CENTERIMAGE: u32 = 0x0200;
const SS_LEFTNOWORDWRAP: u32 = 0x000C;
const SS_NOTIFY: u32 = 0x0100;

pub const PANE_CLASS_NAME: &str = "SsmMonitorPaneClass";
pub const WM_PANE_POPOUT: u32 = WM_APP + 10;
pub const WM_PANE_CONFIG_CHANGED: u32 = WM_APP + 11;

const ID_CMB_PORT: isize = 101;
const ID_BTN_CONNECT: isize = 102;
const ID_BTN_AUTO: isize = 103;
const ID_BTN_KILL: isize = 104;
const ID_BTN_POPOUT: isize = 105;
const ID_CMB_BAUD: isize = 106;
const ID_CMB_DATABITS: isize = 107;
const ID_CMB_PARITY: isize = 108;
const ID_CMB_STOPBITS: isize = 109;
const ID_CMB_EOL: isize = 110;
const ID_CHK_AUTOSCROLL: isize = 111;
const ID_CHK_TIMESTAMP: isize = 112;
const ID_CHK_WRAP: isize = 113;
const ID_BTN_CLEAR: isize = 114;
const ID_EDIT_SEND: isize = 115;
const ID_BTN_SEND: isize = 116;
const ID_LBL_STATUS: isize = 117;
const ID_CMB_DISPLAY: isize = 118;

const TIMER_POLL_ID: usize = 1;

const BAUD_FIXED_ITEMS: &[&str] = &[
    "9600", "19200", "38400", "57600", "74880", "115200", "230400", "460800", "921600", "1000000",
    "2000000",
];
const DATABIT_ITEMS: &[&str] = &["8bit", "7bit", "6bit", "5bit"];
const PARITY_ITEMS: &[&str] = &["N", "E", "O", "M", "S"];
const STOPBIT_ITEMS: &[&str] = &["1", "1.5", "2"];
const EOL_FIXED_ITEMS: &[&str] = &["CRLF", "LF", "CR", "None"];

struct PaneCreateParams {
    manager: Arc<MonitorManager>,
    config: PaneConfig,
    dedicated: bool,
    theme: Arc<UiTheme>,
}

pub struct PaneState {
    hwnd: HWND,
    hinst: HINSTANCE,
    manager: Arc<MonitorManager>,
    theme: Arc<UiTheme>,
    dedicated: bool,
    config: PaneConfig,
    session: Option<Arc<PortSession>>,
    last_seq: u64,
    last_state: ConnectionState,
    last_status_text: String,
    last_applied_serial: SerialConfig,
    last_auto_baud_label: String,
    last_auto_eol_label: String,
    combo_ports: Vec<String>,
    updating_ui: bool,
    send_history: Vec<String>,
    history_idx: Option<usize>,

    tooltip_hwnd: HWND,

    // Child control HWNDs
    lbl_status: HWND,
    cmb_port: HWND,
    btn_connect: HWND,
    btn_auto: HWND,
    btn_kill: HWND,
    btn_popout: HWND,
    cmb_baud: HWND,
    cmb_databits: HWND,
    cmb_parity: HWND,
    cmb_stopbits: HWND,
    cmb_eol: HWND,
    cmb_display: HWND,
    chk_autoscroll: HWND,
    chk_timestamp: HWND,
    chk_wrap: HWND,
    btn_clear: HWND,
    log_view: HWND,
    edit_send: HWND,
    btn_send: HWND,
}

pub unsafe fn register_pane_class(hinst: HINSTANCE) {
    let class_name = to_wide(PANE_CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(pane_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: (COLOR_BTNFACE + 1) as _,
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc);
}

pub unsafe fn create_pane(
    parent: HWND,
    hinst: HINSTANCE,
    manager: Arc<MonitorManager>,
    config: PaneConfig,
    dedicated: bool,
    theme: Arc<UiTheme>,
) -> HWND {
    let params = Box::new(PaneCreateParams {
        manager,
        config,
        dedicated,
        theme,
    });
    let class_name = to_wide(PANE_CLASS_NAME);
    CreateWindowExW(
        0,
        class_name.as_ptr(),
        null(),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_BORDER,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        400,
        300,
        parent,
        null_mut(),
        hinst,
        Box::into_raw(params) as *const _,
    )
}

pub unsafe fn pane_get_config(hwnd: HWND) -> Option<PaneConfig> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneState;
    if ptr.is_null() {
        return None;
    }
    Some((*ptr).config.clone())
}

pub unsafe fn pane_refresh_ports(hwnd: HWND, ports: &[PortInfo]) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneState;
    if ptr.is_null() {
        return;
    }
    (*ptr).populate_port_combo(ports);
}

pub unsafe fn pane_set_visible(hwnd: HWND, visible: bool) {
    ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
}

pub unsafe fn pane_apply_theme(hwnd: HWND, theme: Arc<UiTheme>) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneState;
    if !ptr.is_null() {
        (*ptr).apply_theme(theme);
    }
}

pub unsafe fn pane_apply_language(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneState;
    if !ptr.is_null() {
        (*ptr).apply_language();
    }
}

unsafe extern "system" fn pane_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        let params = Box::from_raw(cs.lpCreateParams as *mut PaneCreateParams);
        let state = Box::new(PaneState::create_controls(
            hwnd,
            cs.hInstance,
            params.manager,
            params.config,
            params.dedicated,
            params.theme,
        ));
        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        (*ptr).init_after_create();
        SetTimer(hwnd, TIMER_POLL_ID, 35, None);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PaneState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *ptr;

    match msg {
        WM_SIZE => {
            state.layout();
            0
        }
        WM_TIMER => {
            if wparam == TIMER_POLL_ID {
                state.poll_session();
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as isize;
            let notif = ((wparam >> 16) & 0xFFFF) as u32;
            state.handle_command(id, notif);
            0
        }
        WM_LOGVIEW_AUTOSCROLL_CHANGED => {
            let enabled = wparam != 0;
            if state.config.auto_scroll != enabled {
                state.config.auto_scroll = enabled;
                state.updating_ui = true;
                SendMessageW(
                    state.chk_autoscroll,
                    BM_SETCHECK,
                    if enabled { 1 } else { 0 },
                    0,
                );
                state.updating_ui = false;
            }
            0
        }
        WM_LOGVIEW_ZOOM_CHANGED => {
            let z = (wparam as u32).clamp(50, 300);
            if state.config.zoom_percent != z {
                state.config.zoom_percent = z;
                state.notify_parent_changed();
            }
            0
        }
        WM_ERASEBKGND => {
            let hdc = wparam as HDC;
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            FillRect(hdc, &rc, state.theme.brush_bg);
            1
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, state.theme.col_fg);
            SetBkColor(hdc, state.theme.col_edit_bg);
            state.theme.brush_edit_bg as LRESULT
        }
        WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, state.theme.col_fg);
            SetBkColor(hdc, state.theme.col_bg);
            state.theme.brush_bg as LRESULT
        }
        WM_CTLCOLORSTATIC => {
            let hdc = wparam as HDC;
            let ctrl = lparam as HWND;
            if ctrl == state.lbl_status {
                SetTextColor(hdc, COLOR_STATUS_FG);
                let (bg, brush) = match &state.last_state {
                    ConnectionState::Connected => {
                        (COLOR_STATUS_CONNECTED, state.theme.brush_status_connected)
                    }
                    ConnectionState::AutoDetecting { .. } | ConnectionState::Connecting => {
                        (COLOR_STATUS_DETECTING, state.theme.brush_status_detecting)
                    }
                    ConnectionState::Occupied { .. } => {
                        (COLOR_STATUS_OCCUPIED, state.theme.brush_status_occupied)
                    }
                    ConnectionState::Disconnected { .. } => (
                        COLOR_STATUS_DISCONNECTED,
                        state.theme.brush_status_disconnected,
                    ),
                    ConnectionState::Released => {
                        (COLOR_STATUS_RELEASED, state.theme.brush_status_released)
                    }
                };
                SetBkColor(hdc, bg);
                return brush as LRESULT;
            }
            SetTextColor(hdc, state.theme.col_fg);
            SetBkColor(hdc, state.theme.col_bg);
            state.theme.brush_bg as LRESULT
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_POLL_ID);
            RemoveWindowSubclass(state.edit_send, Some(send_edit_subclass_proc), 1);
            if !state.tooltip_hwnd.is_null() {
                DestroyWindow(state.tooltip_hwnd);
                state.tooltip_hwnd = null_mut();
            }
            if let Some(prev) = state.session.take() {
                state.manager.release_pane_ref(&prev.port_name);
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

unsafe extern "system" fn send_edit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid: usize,
    ref_data: usize,
) -> LRESULT {
    let pane_hwnd = ref_data as HWND;
    if msg == WM_KEYDOWN {
        let vk = wparam as u16;
        let ctrl = (GetKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0;
        let ptr = GetWindowLongPtrW(pane_hwnd, GWLP_USERDATA) as *mut PaneState;
        if !ptr.is_null() {
            if ctrl {
                if vk == VK_OEM_PLUS || vk == VK_ADD {
                    logview_adjust_zoom((*ptr).log_view, 10);
                    return 0;
                } else if vk == VK_OEM_MINUS || vk == VK_SUBTRACT {
                    logview_adjust_zoom((*ptr).log_view, -10);
                    return 0;
                } else if vk == 0x30 || vk == VK_NUMPAD0 {
                    logview_reset_zoom((*ptr).log_view);
                    return 0;
                }
            }
            if vk == VK_RETURN {
                (*ptr).send_current_input();
                return 0;
            } else if vk == VK_UP || vk == VK_DOWN {
                (*ptr).navigate_history(vk == VK_UP);
                return 0;
            }
        }
    } else if msg == WM_CHAR && wparam == 13 {
        return 0;
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

impl PaneState {
    unsafe fn create_controls(
        hwnd: HWND,
        hinst: HINSTANCE,
        manager: Arc<MonitorManager>,
        config: PaneConfig,
        dedicated: bool,
        theme: Arc<UiTheme>,
    ) -> Self {
        let f_ui = theme.font_ui;
        let f_bold = theme.font_ui_bold;
        let f_mono = theme.font_mono;

        let lbl_status = create_ctrl(
            hwnd,
            hinst,
            "STATIC",
            " ○ ポート未選択",
            WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE | SS_LEFTNOWORDWRAP | SS_NOTIFY,
            ID_LBL_STATUS,
            f_bold,
        );
        let cmb_port = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
            ID_CMB_PORT,
            f_ui,
        );
        if dedicated && !config.port.is_empty() {
            EnableWindow(cmb_port, 0);
        }
        let btn_connect = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "接続",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_CONNECT,
            f_ui,
        );
        let btn_auto = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "Auto検出",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_AUTO,
            f_ui,
        );
        let btn_kill = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "占有終了",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_KILL,
            f_ui,
        );
        let btn_popout = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "別窓",
            WS_CHILD
                | (if dedicated { 0 } else { WS_VISIBLE })
                | WS_TABSTOP
                | BS_PUSHBUTTON as u32,
            ID_BTN_POPOUT,
            f_ui,
        );

        let cmb_baud = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWN as u32 | WS_VSCROLL,
            ID_CMB_BAUD,
            f_ui,
        );
        let cmb_databits = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_DATABITS,
            f_ui,
        );
        let cmb_parity = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_PARITY,
            f_ui,
        );
        let cmb_stopbits = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_STOPBITS,
            f_ui,
        );
        let cmb_eol = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_EOL,
            f_ui,
        );
        let cmb_display = create_ctrl(
            hwnd,
            hinst,
            "COMBOBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            ID_CMB_DISPLAY,
            f_ui,
        );
        let chk_autoscroll = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "自動スクロール",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ID_CHK_AUTOSCROLL,
            f_ui,
        );
        let chk_timestamp = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "時刻",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ID_CHK_TIMESTAMP,
            f_ui,
        );
        let chk_wrap = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "折り返し",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ID_CHK_WRAP,
            f_ui,
        );
        let btn_clear = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "クリア",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_CLEAR,
            f_ui,
        );

        let log_view = create_logview(
            hwnd,
            hinst,
            config.auto_scroll,
            config.show_timestamps,
            config.word_wrap,
            config.display_mode,
            config.zoom_percent,
        );

        let edit_send = create_ctrl(
            hwnd,
            hinst,
            "EDIT",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            ID_EDIT_SEND,
            f_mono,
        );
        SetWindowSubclass(edit_send, Some(send_edit_subclass_proc), 1, hwnd as usize);

        let btn_send = create_ctrl(
            hwnd,
            hinst,
            "BUTTON",
            "送信",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_BTN_SEND,
            f_ui,
        );

        // Populate dropdowns (Item 0 of Baud & EOL dynamically shows current Auto value!)
        let init_auto_baud = format!("Auto ({})", config.serial.baud);
        let w_ab = to_wide(&init_auto_baud);
        SendMessageW(cmb_baud, CB_ADDSTRING, 0, w_ab.as_ptr() as LPARAM);
        populate_combo(cmb_baud, BAUD_FIXED_ITEMS);

        populate_combo(cmb_databits, DATABIT_ITEMS);
        populate_combo(cmb_parity, PARITY_ITEMS);
        populate_combo(cmb_stopbits, STOPBIT_ITEMS);

        let init_auto_eol = tr("Auto (wait)", "Auto (待機)").to_string();
        let w_ae = to_wide(&init_auto_eol);
        SendMessageW(cmb_eol, CB_ADDSTRING, 0, w_ae.as_ptr() as LPARAM);
        populate_combo(cmb_eol, EOL_FIXED_ITEMS);

        populate_combo(cmb_display, &[tr("Text", "テキスト"), "HEX", "BIN"]);

        EnableWindow(btn_kill, 0);

        let last_applied_serial = config.serial;
        let mut st = Self {
            hwnd,
            hinst,
            manager,
            theme,
            dedicated,
            config,
            session: None,
            last_seq: 0,
            last_state: ConnectionState::Released,
            last_status_text: String::new(),
            last_applied_serial,
            last_auto_baud_label: init_auto_baud,
            last_auto_eol_label: init_auto_eol,
            combo_ports: Vec::new(),
            updating_ui: false,
            send_history: Vec::new(),
            history_idx: None,
            tooltip_hwnd: null_mut(),
            lbl_status,
            cmb_port,
            btn_connect,
            btn_auto,
            btn_kill,
            btn_popout,
            cmb_baud,
            cmb_databits,
            cmb_parity,
            cmb_stopbits,
            cmb_eol,
            cmb_display,
            chk_autoscroll,
            chk_timestamp,
            chk_wrap,
            btn_clear,
            log_view,
            edit_send,
            btn_send,
        };
        st.apply_dark_styles();
        st.apply_language_labels();
        st.rebuild_tooltips();
        st
    }

    unsafe fn init_after_create(&mut self) {
        self.sync_controls_from_config();
        let ports = self.manager.list_ports();
        self.populate_port_combo(&ports);

        if !self.config.port.is_empty() {
            self.attach_port(self.config.port.clone(), self.config.connected);
        }
        self.layout();
    }

    unsafe fn apply_dark_styles(&self) {
        let dark = self.theme.dark;
        for b in [
            self.btn_connect,
            self.btn_auto,
            self.btn_kill,
            self.btn_popout,
            self.btn_clear,
            self.btn_send,
        ] {
            apply_ctrl_dark_mode(b, dark, CtrlThemeKind::Button);
        }
        for c in [
            self.cmb_port,
            self.cmb_baud,
            self.cmb_databits,
            self.cmb_parity,
            self.cmb_stopbits,
            self.cmb_eol,
            self.cmb_display,
        ] {
            apply_ctrl_dark_mode(c, dark, CtrlThemeKind::Combo);
        }
        for chk in [self.chk_autoscroll, self.chk_timestamp, self.chk_wrap] {
            apply_ctrl_dark_mode(chk, dark, CtrlThemeKind::Checkbox);
        }
        apply_ctrl_dark_mode(self.edit_send, dark, CtrlThemeKind::Edit);
        logview_set_dark_mode(self.log_view, dark);
    }

    unsafe fn apply_theme(&mut self, theme: Arc<UiTheme>) {
        self.theme = theme;
        let f_ui = self.theme.font_ui as WPARAM;
        let f_bold = self.theme.font_ui_bold as WPARAM;
        let f_mono = self.theme.font_mono as WPARAM;

        SendMessageW(self.lbl_status, WM_SETFONT, f_bold, 1);
        for ctrl in [
            self.cmb_port,
            self.btn_connect,
            self.btn_auto,
            self.btn_kill,
            self.btn_popout,
            self.cmb_baud,
            self.cmb_databits,
            self.cmb_parity,
            self.cmb_stopbits,
            self.cmb_eol,
            self.cmb_display,
            self.chk_autoscroll,
            self.chk_timestamp,
            self.chk_wrap,
            self.btn_clear,
            self.btn_send,
        ] {
            SendMessageW(ctrl, WM_SETFONT, f_ui, 1);
        }
        SendMessageW(self.edit_send, WM_SETFONT, f_mono, 1);
        self.apply_dark_styles();
        logview_set_dpi(self.log_view, self.theme.dpi);
        self.layout();
        InvalidateRect(self.hwnd, null(), 1);
    }

    unsafe fn apply_language_labels(&mut self) {
        let released = matches!(self.last_state, ConnectionState::Released);
        set_window_text(
            self.btn_connect,
            if released {
                tr("Connect", "接続")
            } else {
                tr("Disc.", "切断")
            },
        );
        set_window_text(self.btn_auto, tr("Auto", "Auto検出"));
        set_window_text(self.btn_kill, tr("Kill Owner", "占有終了"));
        set_window_text(self.btn_popout, tr("Pop", "別窓"));
        set_window_text(self.chk_autoscroll, tr("Auto-scroll", "自動スクロール"));
        set_window_text(self.chk_timestamp, tr("Time", "時刻"));
        set_window_text(self.chk_wrap, tr("Wrap", "折り返し"));
        set_window_text(self.btn_clear, tr("Clear", "クリア"));
        set_window_text(self.btn_send, tr("Send", "送信"));

        // Update DisplayMode item 0 ("Text" / "テキスト")
        self.update_auto_combo_item0(self.cmb_display, tr("Text", "テキスト"));

        if self.config.port.is_empty() {
            let txt = format!(" ○ {}", tr("No port selected", "ポート未選択"));
            self.update_status_banner(&txt, ConnectionState::Released);
        }
    }

    unsafe fn apply_language(&mut self) {
        self.apply_language_labels();
        self.rebuild_tooltips();
        // Force port combo placeholder & status banner refresh
        self.combo_ports.clear();
        let ports = self.manager.list_ports();
        self.populate_port_combo(&ports);
        self.last_status_text.clear();
        self.last_auto_eol_label.clear();
        self.poll_session();
        self.layout();
        InvalidateRect(self.hwnd, null(), 1);
    }

    unsafe fn rebuild_tooltips(&mut self) {
        if !self.tooltip_hwnd.is_null() {
            DestroyWindow(self.tooltip_hwnd);
        }
        let tip = create_tooltip_window(self.hwnd, self.hinst);
        self.tooltip_hwnd = tip;
        let hwnd = self.hwnd;

        add_tooltip(
            tip,
            hwnd,
            self.lbl_status,
            tr(
                "Live connection state, serial settings, and detected hardware.\nGreen: Connected / Blue: Detecting / Amber: Waiting / Red: Occupied / Gray: Released",
                "現在の接続状態・通信パラメータ・検出ハードウェア名を表示します\n緑:接続中 / 青:自動検出中 / 橙:再接続待機 / 赤:他プロセス占有中 / 灰:解放中",
            ),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_port,
            if self.dedicated {
                tr(
                    "Dedicated COM port for this popped-out window (fixed)",
                    "このウィンドウ専用の COM ポート（別窓モードのため固定）",
                )
            } else {
                tr(
                    "Select a COM port to monitor (displays detected hardware name)",
                    "監視する COM ポートを選択します（接続中デバイスのハードウェア名も表示）",
                )
            },
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_connect,
            tr(
                "Connect or release this COM port (release before flashing firmware)",
                "COM ポートの接続／切断（解放）を切り替えます\nファームウェア書き込み前の一時解放にも使えます",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_auto,
            tr(
                "Re-run automatic detection for baud rate, framing, and line ending",
                "ボーレート・データビット・パリティ・ストップビット・改行コードの自動検出を再実行します",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_kill,
            tr(
                "Terminate the process occupying this COM port and reconnect (active when occupied)",
                "この COM ポートを占有している他プロセスを終了し、即座に接続します（占有中のみ有効）",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_popout,
            tr(
                "Open a dedicated single-port window for this COM port",
                "この COM ポート専用の独立したウィンドウ（分割不可）で開きます",
            ),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_baud,
            tr(
                "Baud rate (bps). Shows detected value in parentheses when Auto. Custom values allowed.",
                "ボーレート (bps)。Auto 時は現在の検出値を括弧内に表示します\n任意の数値の手入力も可能です",
            ),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_databits,
            tr("Data bits (usually 8bit)", "データビット長（通常は 8bit）"),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_parity,
            tr(
                "Parity (N: None / E: Even / O: Odd / M: Mark / S: Space)",
                "パリティビット（N:なし / E:偶数 / O:奇数 / M:マーク / S:スペース）",
            ),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_stopbits,
            tr("Stop bits (1 / 1.5 / 2)", "ストップビット長（1 / 1.5 / 2）"),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_eol,
            tr(
                "Line ending appended on send. Auto uses the detected RX line ending.",
                "送信時に末尾へ付加する改行コード\nAuto 時は受信データから自動検出した改行コード（括弧内表示）を使用します",
            ),
        );
        add_combo_tooltip(
            tip,
            hwnd,
            self.cmb_display,
            tr(
                "Display mode: Text (lossy UTF-8 + control dots) / HEX (hex dump + ASCII) / BIN (8-bit binary)",
                "受信データの表示形式を切り替えます\nテキスト: 文字化け・制御文字も含めてそのまま表示\nHEX: 16進ダンプ + ASCII プレビュー\nBIN: 2進ビット列表示（独自バイナリパケット解析向け）",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.chk_autoscroll,
            tr(
                "Auto-scroll to bottom on new data. Automatically disables on scroll up and re-enables at bottom.",
                "新しい受信時に自動で最下部へスクロールします\n上へスクロールすると自動OFF、一番下までスクロールすると自動ONになります",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.chk_timestamp,
            tr(
                "Show local time (HH:MM:SS.mmm) on each line. Log files always include full date & time.",
                "各受信行の先頭にローカル時刻（HH:MM:SS.mmm）を表示します\n※ログファイルには常に日付+時刻が記録されます",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.chk_wrap,
            tr(
                "Wrap long lines to fit viewport width. When OFF, use horizontal scroll (Shift + Wheel).",
                "長い行を画面幅で折り返して表示します\nOFF 時は横スクロール表示（Shift + ホイールで横スクロール可能）",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_clear,
            tr(
                "Clear log display and truncate log file for this port",
                "このポートの表示ログとログファイル内容をクリアします",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.edit_send,
            tr(
                "Type text to send (Enter: Send / Up-Down: History / Ctrl+Wheel or Ctrl+±/0: Zoom)",
                "デバイスへ送信する文字列を入力します（Enter で送信 / ↑↓ キーで送信履歴 / Ctrl+ホイール・Ctrl+±/0 で拡大縮小）",
            ),
        );
        add_tooltip(
            tip,
            hwnd,
            self.btn_send,
            tr("Send input text with selected line ending", "入力した文字列に改行コードを付けてデバイスへ送信します"),
        );
    }

    unsafe fn update_auto_combo_item0(&mut self, cmb: HWND, new_label: &str) {
        // Do not touch items while the user currently has the dropdown list open
        if SendMessageW(cmb, CB_GETDROPPEDSTATE, 0, 0) != 0 {
            return;
        }
        let cur_sel = SendMessageW(cmb, CB_GETCURSEL, 0, 0);
        self.updating_ui = true;
        SendMessageW(cmb, CB_DELETESTRING, 0, 0);
        let w = to_wide(new_label);
        SendMessageW(cmb, CB_INSERTSTRING, 0, w.as_ptr() as LPARAM);
        if cur_sel >= 0 {
            SendMessageW(cmb, CB_SETCURSEL, cur_sel as WPARAM, 0);
        }
        self.updating_ui = false;
    }

    unsafe fn sync_controls_from_config(&mut self) {
        self.updating_ui = true;
        let s = self.config.serial;

        if s.auto_detect {
            SendMessageW(self.cmb_baud, CB_SETCURSEL, 0, 0);
        } else {
            let baud_str = s.baud.to_string();
            if let Some(idx) = BAUD_FIXED_ITEMS.iter().position(|&x| x == baud_str) {
                SendMessageW(self.cmb_baud, CB_SETCURSEL, idx + 1, 0);
            } else {
                SendMessageW(self.cmb_baud, CB_SETCURSEL, usize::MAX, 0);
                set_window_text(self.cmb_baud, &baud_str);
            }
        }

        let db_idx = match s.data_bits {
            8 => 0,
            7 => 1,
            6 => 2,
            _ => 3,
        };
        SendMessageW(self.cmb_databits, CB_SETCURSEL, db_idx, 0);

        let par_idx = match s.parity {
            Parity::None => 0,
            Parity::Even => 1,
            Parity::Odd => 2,
            Parity::Mark => 3,
            Parity::Space => 4,
        };
        SendMessageW(self.cmb_parity, CB_SETCURSEL, par_idx, 0);

        let sb_idx = match s.stop_bits {
            StopBits::One => 0,
            StopBits::OnePointFive => 1,
            StopBits::Two => 2,
        };
        SendMessageW(self.cmb_stopbits, CB_SETCURSEL, sb_idx, 0);

        let eol_idx = match s.tx_eol {
            LineEnding::Auto => 0,
            LineEnding::CrLf => 1,
            LineEnding::Lf => 2,
            LineEnding::Cr => 3,
            LineEnding::None => 4,
        };
        SendMessageW(self.cmb_eol, CB_SETCURSEL, eol_idx, 0);

        let disp_idx = match self.config.display_mode {
            DisplayMode::Text => 0,
            DisplayMode::Hex => 1,
            DisplayMode::Bin => 2,
        };
        SendMessageW(self.cmb_display, CB_SETCURSEL, disp_idx, 0);

        SendMessageW(
            self.chk_autoscroll,
            BM_SETCHECK,
            if self.config.auto_scroll { 1 } else { 0 },
            0,
        );
        SendMessageW(
            self.chk_timestamp,
            BM_SETCHECK,
            if self.config.show_timestamps { 1 } else { 0 },
            0,
        );
        SendMessageW(
            self.chk_wrap,
            BM_SETCHECK,
            if self.config.word_wrap { 1 } else { 0 },
            0,
        );
        self.updating_ui = false;
    }

    pub unsafe fn populate_port_combo(&mut self, ports: &[PortInfo]) {
        let mut new_ports: Vec<String> = Vec::new();
        let mut labels: Vec<String> = Vec::new();

        new_ports.push(String::new());
        labels.push(tr("(Select COM Port)", "(COMポート選択)").to_string());

        for p in ports {
            new_ports.push(p.name.clone());
            labels.push(p.display_label());
        }

        if !self.config.port.is_empty() && !new_ports.iter().any(|x| x == &self.config.port) {
            new_ports.push(self.config.port.clone());
            labels.push(format!(
                "{} ({})",
                self.config.port,
                tr("Offline", "未接続")
            ));
        }

        if new_ports == self.combo_ports {
            return;
        }
        if SendMessageW(self.cmb_port, CB_GETDROPPEDSTATE, 0, 0) != 0 {
            return;
        }

        self.updating_ui = true;
        SendMessageW(self.cmb_port, CB_RESETCONTENT, 0, 0);
        for lbl in &labels {
            let w = to_wide(lbl);
            SendMessageW(self.cmb_port, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        self.combo_ports = new_ports;

        let sel = self
            .combo_ports
            .iter()
            .position(|p| p == &self.config.port)
            .unwrap_or(0);
        SendMessageW(self.cmb_port, CB_SETCURSEL, sel, 0);
        self.updating_ui = false;
    }

    unsafe fn attach_port(&mut self, port: String, auto_connect: bool) {
        if let Some(prev) = self.session.take() {
            self.manager.release_pane_ref(&prev.port_name);
        }
        self.config.port = port.clone();
        self.config.connected = auto_connect;
        self.last_seq = 0;
        logview_clear(self.log_view);

        if port.is_empty() {
            self.session = None;
            self.last_state = ConnectionState::Released;
            set_window_text(self.btn_connect, tr("Connect", "接続"));
            EnableWindow(self.btn_kill, 0);
            let txt = format!(" ○ {}", tr("No port selected", "ポート未選択"));
            self.update_status_banner(&txt, ConnectionState::Released);
            self.notify_parent_changed();
            return;
        }

        self.manager.acquire_pane_ref(&port);
        let sess = self
            .manager
            .get_or_create_session(&port, self.config.serial, auto_connect);
        sess.update_config(self.config.serial);
        if auto_connect {
            sess.connect();
        }
        self.session = Some(sess);
        self.poll_session();
        self.notify_parent_changed();
    }

    unsafe fn handle_command(&mut self, id: isize, notif: u32) {
        if self.updating_ui {
            return;
        }
        match id {
            ID_CMB_PORT if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_port, CB_GETCURSEL, 0, 0);
                if idx >= 0 && (idx as usize) < self.combo_ports.len() {
                    let chosen = self.combo_ports[idx as usize].clone();
                    if chosen != self.config.port {
                        self.attach_port(chosen, true);
                    }
                }
            }
            ID_BTN_CONNECT => {
                if self.config.port.is_empty() {
                    return;
                }
                let is_released = matches!(self.last_state, ConnectionState::Released);
                if is_released {
                    self.config.connected = true;
                    if let Some(ref s) = self.session {
                        s.connect();
                    } else {
                        self.attach_port(self.config.port.clone(), true);
                    }
                } else {
                    self.config.connected = false;
                    if let Some(ref s) = self.session {
                        s.disconnect();
                    }
                }
                self.notify_parent_changed();
            }
            ID_BTN_AUTO => {
                self.config.serial.auto_detect = true;
                self.sync_controls_from_config();
                if let Some(ref s) = self.session {
                    s.trigger_autodetect();
                } else if !self.config.port.is_empty() {
                    self.attach_port(self.config.port.clone(), true);
                }
                self.notify_parent_changed();
            }
            ID_BTN_KILL => {
                if let Some(ref s) = self.session {
                    s.kill_owner_and_connect();
                }
            }
            ID_BTN_POPOUT => {
                let parent = GetParent(self.hwnd);
                if !parent.is_null() {
                    PostMessageW(parent, WM_PANE_POPOUT, self.hwnd as WPARAM, 0);
                }
            }
            ID_CMB_BAUD if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_baud, CB_GETCURSEL, 0, 0);
                if idx == 0 {
                    self.config.serial.auto_detect = true;
                    if let Some(ref s) = self.session {
                        s.trigger_autodetect();
                    }
                    self.notify_parent_changed();
                } else if idx > 0 && ((idx - 1) as usize) < BAUD_FIXED_ITEMS.len() {
                    if let Ok(baud) = BAUD_FIXED_ITEMS[(idx - 1) as usize].parse::<u32>() {
                        self.config.serial.auto_detect = false;
                        self.config.serial.baud = baud;
                        self.apply_serial_config();
                    }
                }
            }
            ID_CMB_BAUD if notif == CBN_EDITCHANGE => {
                let text = get_window_text(self.cmb_baud);
                let trimmed = text.trim();
                if trimmed.to_ascii_lowercase().starts_with("auto") {
                    self.config.serial.auto_detect = true;
                    if let Some(ref s) = self.session {
                        s.trigger_autodetect();
                    }
                } else if let Ok(baud) = trimmed.parse::<u32>() {
                    if (110..=4_000_000).contains(&baud) {
                        self.config.serial.auto_detect = false;
                        self.config.serial.baud = baud;
                        self.apply_serial_config();
                    }
                }
            }
            ID_CMB_DATABITS if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_databits, CB_GETCURSEL, 0, 0);
                self.config.serial.data_bits = match idx {
                    0 => 8,
                    1 => 7,
                    2 => 6,
                    _ => 5,
                };
                self.config.serial.auto_detect = false;
                self.apply_serial_config();
            }
            ID_CMB_PARITY if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_parity, CB_GETCURSEL, 0, 0);
                self.config.serial.parity = match idx {
                    1 => Parity::Even,
                    2 => Parity::Odd,
                    3 => Parity::Mark,
                    4 => Parity::Space,
                    _ => Parity::None,
                };
                self.config.serial.auto_detect = false;
                self.apply_serial_config();
            }
            ID_CMB_STOPBITS if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_stopbits, CB_GETCURSEL, 0, 0);
                self.config.serial.stop_bits = match idx {
                    1 => StopBits::OnePointFive,
                    2 => StopBits::Two,
                    _ => StopBits::One,
                };
                self.config.serial.auto_detect = false;
                self.apply_serial_config();
            }
            ID_CMB_EOL if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_eol, CB_GETCURSEL, 0, 0);
                self.config.serial.tx_eol = match idx {
                    1 => LineEnding::CrLf,
                    2 => LineEnding::Lf,
                    3 => LineEnding::Cr,
                    4 => LineEnding::None,
                    _ => LineEnding::Auto,
                };
                self.apply_serial_config();
            }
            ID_CMB_DISPLAY if notif == CBN_SELCHANGE => {
                let idx = SendMessageW(self.cmb_display, CB_GETCURSEL, 0, 0);
                self.config.display_mode = match idx {
                    1 => DisplayMode::Hex,
                    2 => DisplayMode::Bin,
                    _ => DisplayMode::Text,
                };
                logview_set_display_mode(self.log_view, self.config.display_mode);
                self.notify_parent_changed();
            }
            ID_CHK_AUTOSCROLL => {
                self.config.auto_scroll = SendMessageW(self.chk_autoscroll, BM_GETCHECK, 0, 0) != 0;
                logview_set_auto_scroll(self.log_view, self.config.auto_scroll);
                self.notify_parent_changed();
            }
            ID_CHK_TIMESTAMP => {
                self.config.show_timestamps =
                    SendMessageW(self.chk_timestamp, BM_GETCHECK, 0, 0) != 0;
                logview_set_show_timestamps(self.log_view, self.config.show_timestamps);
                self.notify_parent_changed();
            }
            ID_CHK_WRAP => {
                self.config.word_wrap = SendMessageW(self.chk_wrap, BM_GETCHECK, 0, 0) != 0;
                logview_set_word_wrap(self.log_view, self.config.word_wrap);
                self.notify_parent_changed();
            }
            ID_BTN_CLEAR => {
                if let Some(ref s) = self.session {
                    s.clear_logs();
                }
                logview_clear(self.log_view);
            }
            ID_BTN_SEND => {
                self.send_current_input();
                SetFocus(self.edit_send);
            }
            _ => {}
        }
    }

    unsafe fn apply_serial_config(&mut self) {
        self.last_applied_serial = self.config.serial;
        if let Some(ref s) = self.session {
            s.update_config(self.config.serial);
        }
        self.notify_parent_changed();
    }

    unsafe fn send_current_input(&mut self) {
        let text = get_window_text(self.edit_send);
        if let Some(ref s) = self.session {
            s.send_text(&text, None);
            if !text.is_empty() && self.send_history.last().map(|x| x.as_str()) != Some(&text) {
                self.send_history.push(text);
                if self.send_history.len() > 100 {
                    self.send_history.remove(0);
                }
            }
            self.history_idx = None;
            set_window_text(self.edit_send, "");
        }
    }

    unsafe fn navigate_history(&mut self, up: bool) {
        if self.send_history.is_empty() {
            return;
        }
        let new_idx = match (self.history_idx, up) {
            (None, true) => Some(self.send_history.len() - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => {
                if i + 1 < self.send_history.len() {
                    Some(i + 1)
                } else {
                    None
                }
            }
        };
        self.history_idx = new_idx;
        if let Some(i) = new_idx {
            let s = &self.send_history[i];
            set_window_text(self.edit_send, s);
            let len = s.encode_utf16().count();
            SendMessageW(self.edit_send, EM_SETSEL, len, len as LPARAM);
        } else {
            set_window_text(self.edit_send, "");
        }
    }

    unsafe fn poll_session(&mut self) {
        let Some(sess) = self.session.clone() else {
            return;
        };

        let snap = sess.snapshot();

        // Sync UI controls if Auto-detect updated framing/baud
        if snap.config != self.last_applied_serial {
            self.config.serial = snap.config;
            self.last_applied_serial = snap.config;
            self.sync_controls_from_config();
        }

        // Update dynamic Auto labels ("Auto (115200)", "Auto (LF)") so current value is always visible!
        let desired_auto_baud = match &snap.state {
            ConnectionState::AutoDetecting { probing_baud } => {
                format!("Auto ({}...)", probing_baud)
            }
            _ => format!("Auto ({})", snap.config.baud),
        };
        if desired_auto_baud != self.last_auto_baud_label {
            self.last_auto_baud_label = desired_auto_baud.clone();
            self.update_auto_combo_item0(self.cmb_baud, &desired_auto_baud);
        }

        let desired_auto_eol = if snap.detected_eol != LineEnding::None {
            format!("Auto ({})", snap.detected_eol.label())
        } else {
            tr("Auto (wait)", "Auto (待機)").to_string()
        };
        if desired_auto_eol != self.last_auto_eol_label {
            self.last_auto_eol_label = desired_auto_eol.clone();
            self.update_auto_combo_item0(self.cmb_eol, &desired_auto_eol);
        }

        // Build informative status strip
        let hw = self
            .manager
            .find_port_info(&snap.port)
            .map(|p| p.hardware_label)
            .unwrap_or_default();
        let eol_str = if snap.detected_eol != LineEnding::None {
            format!(" {}", snap.detected_eol.label())
        } else {
            String::new()
        };
        let auto_tag = if snap.config.auto_detect {
            if snap.auto_locked {
                tr(" [Auto]", " [Auto確定]")
            } else {
                " [Auto]"
            }
        } else {
            ""
        };
        let hw_suffix = if hw.is_empty() {
            String::new()
        } else {
            format!("  |  {}", hw)
        };

        let status_text = match &snap.state {
            ConnectionState::Connected => format!(
                " ● {}  {}  {} {}{}{}{}",
                tr("Connected", "接続中"),
                snap.port,
                snap.config.baud,
                snap.config.framing_label(),
                eol_str,
                auto_tag,
                hw_suffix
            ),
            ConnectionState::Connecting => format!(
                " ● {}  {}{}",
                tr("Connecting...", "接続中..."),
                snap.port,
                hw_suffix
            ),
            ConnectionState::AutoDetecting { probing_baud } => match lang() {
                Lang::En => format!(" ● Detecting...  {} (probing {} bps)", snap.port, probing_baud),
                Lang::Ja => format!(" ● 自動検出中...  {} ({} bps 確認中)", snap.port, probing_baud),
            },
            ConnectionState::Occupied { owner } => match owner {
                Some(o) => format!(
                    " ● {}  {}  [{} (PID {})]",
                    tr("Occupied", "他プロセス占有中"),
                    snap.port,
                    o.name,
                    o.pid
                ),
                None => format!(
                    " ● {}  {}  ({})",
                    tr("Occupied", "他プロセス占有中"),
                    snap.port,
                    tr("identifying process...", "プロセス検索中...")
                ),
            },
            ConnectionState::Disconnected { reason } => {
                format!(
                    " ● {}  {}  ({})",
                    tr("Disconnected", "切断中"),
                    snap.port,
                    reason
                )
            }
            ConnectionState::Released => format!(
                " ○ {}  {}{}",
                tr("Released", "切断中 (解放)"),
                snap.port,
                hw_suffix
            ),
        };

        let occupied = matches!(snap.state, ConnectionState::Occupied { .. });
        let released = matches!(snap.state, ConnectionState::Released);
        if self.last_state != snap.state {
            EnableWindow(self.btn_kill, if occupied { 1 } else { 0 });
            set_window_text(
                self.btn_connect,
                if released {
                    tr("Connect", "接続")
                } else {
                    tr("Disc.", "切断")
                },
            );
        }
        self.update_status_banner(&status_text, snap.state);

        // Fetch and append any new log entries directly to custom virtual LogView
        if snap.latest_seq > self.last_seq {
            let new_entries = sess.read_logs(self.last_seq, 0);
            if !new_entries.is_empty() {
                if let Some(last) = new_entries.last() {
                    self.last_seq = last.seq;
                }
                logview_append_entries(self.log_view, &new_entries);
            }
        }
    }

    unsafe fn update_status_banner(&mut self, text: &str, new_state: ConnectionState) {
        let state_changed = self.last_state != new_state;
        let text_changed = self.last_status_text != text;
        if state_changed || text_changed {
            self.last_state = new_state;
            self.last_status_text = text.to_string();
            set_window_text(self.lbl_status, text);
            InvalidateRect(self.lbl_status, null(), 1);
        }
    }

    unsafe fn notify_parent_changed(&self) {
        let parent = GetParent(self.hwnd);
        if !parent.is_null() {
            PostMessageW(parent, WM_PANE_CONFIG_CHANGED, self.hwnd as WPARAM, 0);
        }
    }

    unsafe fn layout(&self) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let w = (rc.right - rc.left).max(100);
        let h = (rc.bottom - rc.top).max(120);
        let dpi = self.theme.dpi;
        let s = |v: i32| dpi_scale(v, dpi);

        let pad = s(4);
        let status_h = s(22);
        let row1_h = s(24);
        let row2_h = s(22);
        let send_h = s(24);
        let drop_h = s(240);

        // 0. Status strip
        MoveWindow(self.lbl_status, 0, 0, w, status_h, 1);

        // 1. Row 1: Port combo + Connect + Auto + Kill (+ Popout when not dedicated)
        let y1 = status_h + pad;
        let btn_conn_w = s(58);
        let btn_auto_w = s(68);
        let btn_kill_w = s(74);
        let btn_pop_w = if self.dedicated { 0 } else { s(44) };
        let right_fixed =
            btn_conn_w + btn_auto_w + btn_kill_w + btn_pop_w + pad * (if self.dedicated { 4 } else { 5 });
        let port_w = (w - right_fixed - pad).max(s(90));

        let mut x = pad;
        MoveWindow(self.cmb_port, x, y1, port_w, drop_h, 1);
        x += port_w + pad;
        MoveWindow(self.btn_connect, x, y1, btn_conn_w, row1_h, 1);
        x += btn_conn_w + pad;
        MoveWindow(self.btn_auto, x, y1, btn_auto_w, row1_h, 1);
        x += btn_auto_w + pad;
        MoveWindow(self.btn_kill, x, y1, btn_kill_w, row1_h, 1);
        if !self.dedicated {
            x += btn_kill_w + pad;
            MoveWindow(self.btn_popout, x, y1, btn_pop_w, row1_h, 1);
        }

        // 2. Row 2: Baud + DataBits + Parity + StopBits + EOL + DisplayMode + AutoScroll + Time + Wrap + Clear
        let y2 = y1 + row1_h + pad;
        let mut x2 = pad;
        let w_baud = s(112);
        let w_db = s(46);
        let w_par = s(36);
        let w_sb = s(40);
        let w_eol = s(92);
        let w_disp = s(76);
        let w_as = s(114);
        let w_ts = s(52);
        let w_wrap = s(76);
        let w_clr = s(50);

        MoveWindow(self.cmb_baud, x2, y2, w_baud, drop_h, 1);
        x2 += w_baud + s(3);
        MoveWindow(self.cmb_databits, x2, y2, w_db, drop_h, 1);
        x2 += w_db + s(3);
        MoveWindow(self.cmb_parity, x2, y2, w_par, drop_h, 1);
        x2 += w_par + s(3);
        MoveWindow(self.cmb_stopbits, x2, y2, w_sb, drop_h, 1);
        x2 += w_sb + s(3);
        MoveWindow(self.cmb_eol, x2, y2, w_eol, drop_h, 1);
        x2 += w_eol + s(3);
        MoveWindow(self.cmb_display, x2, y2, w_disp, drop_h, 1);
        x2 += w_disp + s(5);

        // If pane is narrow (e.g. 2x2 on 1080p), wrap checkboxes onto a clean Row 2b so nothing clips!
        let need_row2b = x2 + w_as + w_ts + w_wrap + w_clr + s(16) > w;
        let y_log_top = if need_row2b {
            let y2b = y2 + row2_h + s(3);
            let mut xb = pad;
            MoveWindow(self.chk_autoscroll, xb, y2b, w_as, row2_h, 1);
            xb += w_as + s(4);
            MoveWindow(self.chk_timestamp, xb, y2b, w_ts, row2_h, 1);
            xb += w_ts + s(4);
            MoveWindow(self.chk_wrap, xb, y2b, w_wrap, row2_h, 1);
            let clr_x = (w - pad - w_clr).max(xb + w_wrap + s(4));
            MoveWindow(self.btn_clear, clr_x, y2b, w_clr, row2_h, 1);
            y2b + row2_h + pad
        } else {
            MoveWindow(self.chk_autoscroll, x2, y2, w_as, row2_h, 1);
            x2 += w_as + s(4);
            MoveWindow(self.chk_timestamp, x2, y2, w_ts, row2_h, 1);
            x2 += w_ts + s(4);
            MoveWindow(self.chk_wrap, x2, y2, w_wrap, row2_h, 1);
            let clr_x = (w - pad - w_clr).max(x2 + w_wrap + s(4));
            MoveWindow(self.btn_clear, clr_x, y2, w_clr, row2_h, 1);
            y2 + row2_h + pad
        };

        // 3. Bottom Send Row
        let y_send = h - pad - send_h;
        let send_btn_w = s(56);
        let send_edit_w = (w - pad * 3 - send_btn_w).max(s(60));
        MoveWindow(self.edit_send, pad, y_send, send_edit_w, send_h, 1);
        MoveWindow(
            self.btn_send,
            pad + send_edit_w + pad,
            y_send,
            send_btn_w,
            send_h,
            1,
        );

        // 4. Center Custom Virtual Log View
        let log_h = (y_send - pad - y_log_top).max(s(40));
        MoveWindow(
            self.log_view,
            pad,
            y_log_top,
            (w - pad * 2).max(s(60)),
            log_h,
            1,
        );
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

unsafe fn populate_combo(cmb: HWND, items: &[&str]) {
    for &item in items {
        let w = to_wide(item);
        SendMessageW(cmb, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
    }
}
