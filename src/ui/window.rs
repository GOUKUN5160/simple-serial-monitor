use crate::config::{
    clamp_rect_to_screen, save_config, AppConfig, WindowStateConfig, DEFAULT_API_PORT,
};
use crate::manager::MonitorManager;
use crate::types::{LayoutMode, NotifyMode, PaneConfig, WindowRect};
use crate::ui::notify::{init_tray_icon, notify_new_devices, remove_tray_icon};
use crate::ui::pane::{
    create_pane, pane_apply_theme, pane_get_config, pane_refresh_ports, pane_set_visible,
    WM_PANE_CONFIG_CHANGED, WM_PANE_POPOUT,
};
use crate::ui::tooltip::{add_combo_tooltip, add_tooltip, create_tooltip_window};
use crate::ui::{dpi_scale, rgb, set_window_text, to_wide, window_dpi, UiTheme, IDI_APP_ICON};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, ScreenToClient,
    COLOR_BTNFACE, HFONT, PAINTSTRUCT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowPlacement, IsWindow, KillTimer, LoadCursorW, LoadImageW,
    MoveWindow, PostQuitMessage, RegisterClassExW, SendMessageW, SetCursor, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, BS_PUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA,
    HICON, HTCLIENT, IDC_ARROW, IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IMAGE_ICON, LR_DEFAULTCOLOR,
    LR_SHARED, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_MAXIMIZE, SW_SHOW, SW_SHOWNORMAL, WINDOWPLACEMENT, WM_CAPTURECHANGED, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_DEVICECHANGE, WM_DPICHANGED, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_SETCURSOR, WM_SETFONT, WM_SIZE,
    WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

const MAIN_CLASS_NAME: &str = "SsmMainWindowClass";

const SS_CENTERIMAGE: u32 = 0x0200;
const SS_RIGHT: u32 = 0x0002;
const SS_NOTIFY: u32 = 0x0100;

const ID_BTN_1X1: isize = 201;
const ID_BTN_1X2: isize = 202;
const ID_BTN_2X1: isize = 203;
const ID_BTN_2X2: isize = 204;
const ID_BTN_NEW_WIN: isize = 205;
const ID_BTN_REFRESH: isize = 206;
const ID_CMB_NOTIFY: isize = 207;
const ID_BTN_OPEN_LOGS: isize = 208;
const ID_LBL_API: isize = 209;

const TIMER_REFRESH_ID: usize = 10;

const NOTIFY_ITEMS: &[&str] = &["通知:OFF", "通知:アプリ内", "通知:Windows", "通知:両方"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragAxis {
    None,
    Horiz,
    Vert,
    Both,
}

// Thread-local GUI window registry (Win32 message loop is single-threaded).
thread_local! {
    static OPEN_WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    static SHUTTING_DOWN: RefCell<bool> = const { RefCell::new(false) };
    static GLOBAL_NOTIFY_MODE: RefCell<NotifyMode> = const { RefCell::new(NotifyMode::InApp) };
    static PRIMARY_HWND: RefCell<HWND> = const { RefCell::new(null_mut()) };
    static TRAY_READY: RefCell<bool> = const { RefCell::new(false) };
}

struct WindowCreateParams {
    is_primary: bool,
    dedicated: bool,
    manager: Arc<MonitorManager>,
    layout: LayoutMode,
    split_x: f32,
    split_y: f32,
    panes: Vec<PaneConfig>,
    api_port: Option<u16>,
}

struct MonitorWindowState {
    hwnd: HWND,
    hinst: HINSTANCE,
    is_primary: bool,
    dedicated: bool,
    manager: Arc<MonitorManager>,
    theme: Arc<UiTheme>,
    api_port: u16,
    layout: LayoutMode,
    split_x: f32,
    split_y: f32,
    drag_axis: DragAxis,
    panes: Vec<HWND>,

    tooltip_hwnd: HWND,
    btn_1x1: HWND,
    btn_1x2: HWND,
    btn_2x1: HWND,
    btn_2x2: HWND,
    btn_new_win: HWND,
    btn_refresh: HWND,
    cmb_notify: HWND,
    lbl_api: HWND,
    btn_open_logs: HWND,
}

struct GridMetrics {
    gx: i32,
    gy: i32,
    gw: i32,
    gh: i32,
    gap: i32,
    cw: i32,
    ch: i32,
}

pub unsafe fn register_window_classes(hinst: HINSTANCE) {
    let cursor = LoadCursorW(null_mut(), IDC_ARROW);
    let icon_big = LoadImageW(
        hinst,
        IDI_APP_ICON as *const u16,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXICON),
        GetSystemMetrics(SM_CYICON),
        LR_DEFAULTCOLOR | LR_SHARED,
    ) as HICON;
    let icon_sm = LoadImageW(
        hinst,
        IDI_APP_ICON as *const u16,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR | LR_SHARED,
    ) as HICON;

    let main_cls = to_wide(MAIN_CLASS_NAME);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(monitor_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: icon_big,
        hCursor: cursor,
        hbrBackground: (COLOR_BTNFACE + 1) as _,
        lpszMenuName: null(),
        lpszClassName: main_cls.as_ptr(),
        hIconSm: icon_sm,
    };
    RegisterClassExW(&wc);
}

pub unsafe fn create_initial_windows(
    hinst: HINSTANCE,
    manager: Arc<MonitorManager>,
    config: AppConfig,
    api_port: Option<u16>,
) -> HWND {
    GLOBAL_NOTIFY_MODE.with(|m| *m.borrow_mut() = config.notify_mode);

    let mut main_panes = config.main_panes;
    while main_panes.len() < 4 {
        main_panes.push(PaneConfig::default());
    }
    if main_panes[0].port.is_empty() && config.extra_windows.is_empty() {
        let ports = manager.list_ports();
        let chosen = ports
            .iter()
            .find(|p| p.vid.is_some())
            .or_else(|| ports.first());
        if let Some(first) = chosen {
            main_panes[0].port = first.name.clone();
            main_panes[0].connected = true;
        }
    }

    let main_hwnd = spawn_monitor_window(
        hinst,
        true,
        false,
        Arc::clone(&manager),
        config.main_rect,
        config.layout,
        config.main_split_x,
        config.main_split_y,
        main_panes,
        api_port,
    );

    for ew in config.extra_windows {
        let _ = spawn_monitor_window(
            hinst,
            false,
            ew.dedicated,
            Arc::clone(&manager),
            ew.rect,
            if ew.dedicated {
                LayoutMode::Grid1x1
            } else {
                ew.layout
            },
            ew.split_x,
            ew.split_y,
            ew.panes,
            api_port,
        );
    }

    main_hwnd
}

#[allow(clippy::too_many_arguments)]
unsafe fn spawn_monitor_window(
    hinst: HINSTANCE,
    is_primary: bool,
    dedicated: bool,
    manager: Arc<MonitorManager>,
    raw_rect: WindowRect,
    layout: LayoutMode,
    split_x: f32,
    split_y: f32,
    mut panes: Vec<PaneConfig>,
    api_port: Option<u16>,
) -> HWND {
    while panes.len() < 4 {
        panes.push(PaneConfig::default());
    }
    let rect = clamp_rect_to_screen(&raw_rect);
    let maximized = rect.maximized;

    let title_str = build_window_title(is_primary, dedicated, layout, &panes, &manager);
    let params = Box::new(WindowCreateParams {
        is_primary,
        dedicated,
        manager,
        layout: if dedicated {
            LayoutMode::Grid1x1
        } else {
            layout
        },
        split_x: split_x.clamp(0.15, 0.85),
        split_y: split_y.clamp(0.15, 0.85),
        panes,
        api_port,
    });

    let cls = to_wide(MAIN_CLASS_NAME);
    let title = to_wide(&title_str);
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        null_mut(),
        null_mut(),
        hinst,
        Box::into_raw(params) as *const _,
    );
    if !hwnd.is_null() {
        OPEN_WINDOWS.with(|list| list.borrow_mut().push(hwnd));
        if is_primary {
            PRIMARY_HWND.with(|p| *p.borrow_mut() = hwnd);
            let ok = init_tray_icon(hwnd, hinst);
            TRAY_READY.with(|t| *t.borrow_mut() = ok);
        }
        ShowWindow(hwnd, if maximized { SW_MAXIMIZE } else { SW_SHOWNORMAL });
    }
    hwnd
}

fn build_window_title(
    is_primary: bool,
    dedicated: bool,
    layout: LayoutMode,
    panes: &[PaneConfig],
    manager: &MonitorManager,
) -> String {
    if is_primary {
        return "Simple Serial Monitor".to_string();
    }
    if dedicated {
        let port = panes.first().map(|p| p.port.as_str()).unwrap_or("");
        if port.is_empty() {
            return "Simple Serial Monitor (専用ウィンドウ)".to_string();
        }
        if let Some(info) = manager.find_port_info(port) {
            if !info.hardware_label.is_empty() {
                return format!("{} ({}) - Simple Serial Monitor", port, info.hardware_label);
            }
        }
        return format!("{} - Simple Serial Monitor", port);
    }
    let vis = layout.pane_count();
    let active: Vec<&str> = panes
        .iter()
        .take(vis)
        .filter(|p| !p.port.is_empty())
        .map(|p| p.port.as_str())
        .collect();
    if active.is_empty() {
        "Simple Serial Monitor (サブウィンドウ)".to_string()
    } else {
        format!("{} - Simple Serial Monitor", active.join(" / "))
    }
}

unsafe extern "system" fn monitor_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        let params = Box::from_raw(cs.lpCreateParams as *mut WindowCreateParams);
        let dpi = window_dpi(hwnd);
        let theme = Arc::new(UiTheme::new(dpi));
        let f_ui = theme.font_ui;

        let mut btn_1x1 = null_mut();
        let mut btn_1x2 = null_mut();
        let mut btn_2x1 = null_mut();
        let mut btn_2x2 = null_mut();
        let mut btn_new_win = null_mut();
        let mut btn_refresh = null_mut();
        let mut cmb_notify = null_mut();
        let mut lbl_api = null_mut();
        let mut btn_open_logs = null_mut();
        let mut tooltip_hwnd = null_mut();

        if !params.dedicated {
            btn_1x1 = create_tb_btn(hwnd, cs.hInstance, "1画面", ID_BTN_1X1, f_ui);
            btn_1x2 = create_tb_btn(hwnd, cs.hInstance, "2画面横", ID_BTN_1X2, f_ui);
            btn_2x1 = create_tb_btn(hwnd, cs.hInstance, "2画面縦", ID_BTN_2X1, f_ui);
            btn_2x2 = create_tb_btn(hwnd, cs.hInstance, "4画面", ID_BTN_2X2, f_ui);
            btn_new_win =
                create_tb_btn(hwnd, cs.hInstance, "+ 別ウィンドウ", ID_BTN_NEW_WIN, f_ui);
            btn_refresh = create_tb_btn(hwnd, cs.hInstance, "ポート再検索", ID_BTN_REFRESH, f_ui);

            if params.is_primary {
                let cmb_cls = to_wide("COMBOBOX");
                cmb_notify = CreateWindowExW(
                    0,
                    cmb_cls.as_ptr(),
                    null(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
                    0,
                    0,
                    10,
                    10,
                    hwnd,
                    ID_CMB_NOTIFY as _,
                    cs.hInstance,
                    null(),
                );
                SendMessageW(cmb_notify, WM_SETFONT, f_ui as WPARAM, 1);
                for &item in NOTIFY_ITEMS {
                    let w = to_wide(item);
                    SendMessageW(cmb_notify, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
                }
                let nm = GLOBAL_NOTIFY_MODE.with(|m| *m.borrow());
                let idx = match nm {
                    NotifyMode::Off => 0,
                    NotifyMode::InApp => 1,
                    NotifyMode::Windows => 2,
                    NotifyMode::Both => 3,
                };
                SendMessageW(cmb_notify, CB_SETCURSEL, idx, 0);
            }

            btn_open_logs =
                create_tb_btn(hwnd, cs.hInstance, "ログフォルダ", ID_BTN_OPEN_LOGS, f_ui);

            let api_text = match params.api_port {
                Some(p) => format!("Agent API: 127.0.0.1:{}", p),
                None => "Agent API: 停止".to_string(),
            };
            let lbl_cls = to_wide("STATIC");
            let lbl_txt = to_wide(&api_text);
            lbl_api = CreateWindowExW(
                0,
                lbl_cls.as_ptr(),
                lbl_txt.as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_CENTERIMAGE | SS_RIGHT | SS_NOTIFY,
                0,
                0,
                10,
                10,
                hwnd,
                ID_LBL_API as _,
                cs.hInstance,
                null(),
            );
            SendMessageW(lbl_api, WM_SETFONT, f_ui as WPARAM, 1);

            tooltip_hwnd = create_tooltip_window(hwnd, cs.hInstance);
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_1x1,
                "1 画面表示に切り替えます（分割比率もリセット）",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_1x2,
                "左右 2 画面表示に切り替えます\n画面の間をドラッグでサイズ変更でき、再度押すと均等幅にリセットされます",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_2x1,
                "上下 2 画面表示に切り替えます\n画面の間をドラッグでサイズ変更でき、再度押すと均等高さにリセットされます",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_2x2,
                "2×2 の 4 画面表示に切り替えます\n縦横の境界や中央交点をドラッグでサイズ変更でき、再度押すと均等にリセットされます",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_new_win,
                "複数画面分割に対応した新しいウィンドウを開きます",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_refresh,
                "アクティブな COM ポートとハードウェア名を手動で再スキャンします",
            );
            if !cmb_notify.is_null() {
                add_combo_tooltip(
                    tooltip_hwnd,
                    hwnd,
                    cmb_notify,
                    "新しい COM デバイス接続時の通知方法を設定します\nOFF / アプリ内ポップアップ / Windows通知 / 両方",
                );
            }
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                btn_open_logs,
                "日付付きリアルタイムログファイルが保存されている logs フォルダを開きます",
            );
            add_tooltip(
                tooltip_hwnd,
                hwnd,
                lbl_api,
                "Antigravity / Codex / ssm-cli から同時アクセスできるローカル HTTP/SSE API アドレス",
            );
        }

        let pane_count = if params.dedicated { 1 } else { 4 };
        let mut panes = Vec::with_capacity(pane_count);
        for pc in params.panes.into_iter().take(pane_count) {
            let ph = create_pane(
                hwnd,
                cs.hInstance,
                Arc::clone(&params.manager),
                pc,
                params.dedicated,
                Arc::clone(&theme),
            );
            panes.push(ph);
        }

        let state = Box::new(MonitorWindowState {
            hwnd,
            hinst: cs.hInstance,
            is_primary: params.is_primary,
            dedicated: params.dedicated,
            manager: params.manager,
            theme,
            api_port: params.api_port.unwrap_or(DEFAULT_API_PORT),
            layout: params.layout,
            split_x: params.split_x,
            split_y: params.split_y,
            drag_axis: DragAxis::None,
            panes,
            tooltip_hwnd,
            btn_1x1,
            btn_1x2,
            btn_2x1,
            btn_2x2,
            btn_new_win,
            btn_refresh,
            cmb_notify,
            lbl_api,
            btn_open_logs,
        });

        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        (*ptr).update_layout();
        if (*ptr).is_primary {
            SetTimer(hwnd, TIMER_REFRESH_ID, 2000, None);
        }
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MonitorWindowState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *ptr;

    match msg {
        WM_SIZE => {
            state.update_layout();
            0
        }
        WM_PAINT => {
            state.paint_splitters();
            0
        }
        WM_SETCURSOR => {
            let hit = (lparam & 0xFFFF) as u32;
            if hit == HTCLIENT && !state.dedicated {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                ScreenToClient(hwnd, &mut pt);
                let axis = if state.drag_axis != DragAxis::None {
                    state.drag_axis
                } else {
                    state.hit_test_splitter(pt.x, pt.y)
                };
                match axis {
                    DragAxis::Horiz => {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZEWE));
                        return 1;
                    }
                    DragAxis::Vert => {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZENS));
                        return 1;
                    }
                    DragAxis::Both => {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZEALL));
                        return 1;
                    }
                    DragAxis::None => {}
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            if !state.dedicated {
                let x = (lparam & 0xFFFF) as u16 as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                let axis = state.hit_test_splitter(x, y);
                if axis != DragAxis::None {
                    state.drag_axis = axis;
                    SetCapture(hwnd);
                    return 0;
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEMOVE => {
            if state.drag_axis != DragAxis::None {
                let x = (lparam & 0xFFFF) as u16 as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                state.apply_splitter_drag(x, y);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONUP => {
            if state.drag_axis != DragAxis::None {
                state.drag_axis = DragAxis::None;
                ReleaseCapture();
                save_all_windows_config();
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CAPTURECHANGED => {
            state.drag_axis = DragAxis::None;
            0
        }
        WM_DPICHANGED => {
            let new_dpi = (wparam & 0xFFFF) as u32;
            let suggested = &*(lparam as *const RECT);
            state.handle_dpi_changed(new_dpi, suggested);
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as isize;
            let notif = ((wparam >> 16) & 0xFFFF) as u32;
            match id {
                ID_BTN_1X1 => state.set_layout_and_reset_split(LayoutMode::Grid1x1),
                ID_BTN_1X2 => state.set_layout_and_reset_split(LayoutMode::Grid1x2),
                ID_BTN_2X1 => state.set_layout_and_reset_split(LayoutMode::Grid2x1),
                ID_BTN_2X2 => state.set_layout_and_reset_split(LayoutMode::Grid2x2),
                ID_BTN_NEW_WIN => {
                    state.open_new_multi_window();
                    save_all_windows_config();
                }
                ID_BTN_REFRESH => {
                    refresh_all_windows_ports(&state.manager, state.hinst, false);
                }
                ID_CMB_NOTIFY if notif == CBN_SELCHANGE => {
                    let idx = SendMessageW(state.cmb_notify, CB_GETCURSEL, 0, 0);
                    let mode = match idx {
                        0 => NotifyMode::Off,
                        2 => NotifyMode::Windows,
                        3 => NotifyMode::Both,
                        _ => NotifyMode::InApp,
                    };
                    GLOBAL_NOTIFY_MODE.with(|m| *m.borrow_mut() = mode);
                    save_all_windows_config();
                }
                ID_BTN_OPEN_LOGS => {
                    let dir = to_wide(&state.manager.log_dir().to_string_lossy());
                    let op = to_wide("open");
                    ShellExecuteW(hwnd, op.as_ptr(), dir.as_ptr(), null(), null(), SW_SHOW);
                }
                _ => {}
            }
            0
        }
        WM_PANE_POPOUT => {
            // 「別窓」 opens a dedicated single-COM-port window (no split controls!)
            let pane_hwnd = wparam as HWND;
            if let Some(cfg) = pane_get_config(pane_hwnd) {
                state.open_dedicated_port_window(cfg);
                save_all_windows_config();
            }
            0
        }
        WM_PANE_CONFIG_CHANGED => {
            state.update_title();
            save_all_windows_config();
            0
        }
        WM_DEVICECHANGE => {
            refresh_all_windows_ports(&state.manager, state.hinst, true);
            0
        }
        WM_TIMER => {
            if wparam == TIMER_REFRESH_ID {
                refresh_all_windows_ports(&state.manager, state.hinst, true);
            }
            0
        }
        WM_CLOSE => {
            let remaining = OPEN_WINDOWS.with(|list| list.borrow().len());
            if state.is_primary || remaining <= 1 {
                save_all_windows_config();
                SHUTTING_DOWN.with(|f| *f.borrow_mut() = true);
                let others: Vec<HWND> = OPEN_WINDOWS.with(|list| {
                    list.borrow()
                        .iter()
                        .copied()
                        .filter(|&h| h != hwnd && IsWindow(h) != 0)
                        .collect()
                });
                for oh in others {
                    DestroyWindow(oh);
                }
                DestroyWindow(hwnd);
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            if state.is_primary {
                KillTimer(hwnd, TIMER_REFRESH_ID);
                if TRAY_READY.with(|t| *t.borrow()) {
                    remove_tray_icon(hwnd);
                    TRAY_READY.with(|t| *t.borrow_mut() = false);
                }
            }
            if !state.tooltip_hwnd.is_null() {
                DestroyWindow(state.tooltip_hwnd);
                state.tooltip_hwnd = null_mut();
            }
            OPEN_WINDOWS.with(|list| list.borrow_mut().retain(|&h| h != hwnd));
            let shutting_down = SHUTTING_DOWN.with(|f| *f.borrow());
            let left = OPEN_WINDOWS.with(|list| list.borrow().len());
            if left == 0 {
                PostQuitMessage(0);
            } else if !shutting_down {
                save_all_windows_config();
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

impl MonitorWindowState {
    unsafe fn set_layout_and_reset_split(&mut self, layout: LayoutMode) {
        self.layout = layout;
        // Clicking the layout button always resets dragged splitter ratios back to 50% / 50%!
        self.split_x = 0.5;
        self.split_y = 0.5;
        self.update_layout();
        self.update_title();
        InvalidateRect(self.hwnd, null(), 1);
        save_all_windows_config();
    }

    unsafe fn handle_dpi_changed(&mut self, new_dpi: u32, suggested: &RECT) {
        let dpi = if new_dpi == 0 { 96 } else { new_dpi };
        let new_theme = Arc::new(UiTheme::new(dpi));
        let f_ui = new_theme.font_ui as WPARAM;

        if !self.dedicated {
            for ctrl in [
                self.btn_1x1,
                self.btn_1x2,
                self.btn_2x1,
                self.btn_2x2,
                self.btn_new_win,
                self.btn_refresh,
                self.cmb_notify,
                self.lbl_api,
                self.btn_open_logs,
            ] {
                if !ctrl.is_null() {
                    SendMessageW(ctrl, WM_SETFONT, f_ui, 1);
                }
            }
        }

        self.theme = Arc::clone(&new_theme);
        for &ph in &self.panes {
            pane_apply_theme(ph, Arc::clone(&new_theme));
        }

        SetWindowPos(
            self.hwnd,
            null_mut(),
            suggested.left,
            suggested.top,
            (suggested.right - suggested.left).max(380),
            (suggested.bottom - suggested.top).max(280),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        self.update_layout();
        InvalidateRect(self.hwnd, null(), 1);
    }

    unsafe fn open_new_multi_window(&self) {
        let count = OPEN_WINDOWS.with(|list| list.borrow().len()) as i32;
        let offset = count * 36;
        let rect = WindowRect {
            x: 140 + offset,
            y: 110 + offset,
            width: 960,
            height: 660,
            maximized: false,
        };

        let mut pc = PaneConfig::default();
        let used = collect_all_used_ports();
        let all_ports = self.manager.list_ports();
        let chosen = all_ports
            .iter()
            .find(|p| p.vid.is_some() && !used.contains(&p.name))
            .or_else(|| all_ports.iter().find(|p| !used.contains(&p.name)));
        if let Some(p) = chosen {
            pc.port = p.name.clone();
        }

        let mut panes = vec![pc];
        while panes.len() < 4 {
            panes.push(PaneConfig::default());
        }

        spawn_monitor_window(
            self.hinst,
            false,
            false,
            Arc::clone(&self.manager),
            rect,
            LayoutMode::Grid1x1,
            0.5,
            0.5,
            panes,
            Some(self.api_port),
        );
    }

    unsafe fn open_dedicated_port_window(&self, pane_config: PaneConfig) {
        let count = OPEN_WINDOWS.with(|list| list.borrow().len()) as i32;
        let offset = count * 36;
        let rect = WindowRect {
            x: 160 + offset,
            y: 120 + offset,
            width: 720,
            height: 500,
            maximized: false,
        };
        let panes = vec![pane_config];
        spawn_monitor_window(
            self.hinst,
            false,
            true,
            Arc::clone(&self.manager),
            rect,
            LayoutMode::Grid1x1,
            0.5,
            0.5,
            panes,
            Some(self.api_port),
        );
    }

    unsafe fn update_title(&self) {
        let panes: Vec<PaneConfig> = self
            .panes
            .iter()
            .map(|&ph| pane_get_config(ph).unwrap_or_default())
            .collect();
        let title = build_window_title(
            self.is_primary,
            self.dedicated,
            self.layout,
            &panes,
            &self.manager,
        );
        set_window_text(self.hwnd, &title);
    }

    unsafe fn compute_grid_metrics(&self) -> GridMetrics {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let w = (rc.right - rc.left).max(200);
        let h = (rc.bottom - rc.top).max(200);
        let dpi = self.theme.dpi;
        let s = |v: i32| dpi_scale(v, dpi);

        if self.dedicated {
            return GridMetrics {
                gx: 0,
                gy: 0,
                gw: w,
                gh: h,
                gap: 0,
                cw: w,
                ch: h,
            };
        }

        let pad = s(5);
        let tb_h = s(24);
        let gx = pad;
        let gy = pad + tb_h + pad;
        let gw = (w - pad * 2).max(100);
        let gh = (h - gy - pad).max(100);
        let gap = s(7);

        let min_pane = s(120);
        let max_cw = (gw - gap - min_pane).max(min_pane);
        let cw = (((gw - gap) as f32 * self.split_x).round() as i32).clamp(min_pane, max_cw);

        let max_ch = (gh - gap - min_pane).max(min_pane);
        let ch = (((gh - gap) as f32 * self.split_y).round() as i32).clamp(min_pane, max_ch);

        GridMetrics {
            gx,
            gy,
            gw,
            gh,
            gap,
            cw,
            ch,
        }
    }

    unsafe fn hit_test_splitter(&self, x: i32, y: i32) -> DragAxis {
        if self.dedicated || self.layout == LayoutMode::Grid1x1 {
            return DragAxis::None;
        }
        let m = self.compute_grid_metrics();
        if x < m.gx || x > m.gx + m.gw || y < m.gy || y > m.gy + m.gh {
            return DragAxis::None;
        }
        let tol = dpi_scale(2, self.theme.dpi);
        let vx0 = m.gx + m.cw - tol;
        let vx1 = m.gx + m.cw + m.gap + tol;
        let hy0 = m.gy + m.ch - tol;
        let hy1 = m.gy + m.ch + m.gap + tol;

        let on_v = x >= vx0 && x <= vx1;
        let on_h = y >= hy0 && y <= hy1;

        match self.layout {
            LayoutMode::Grid1x2 if on_v => DragAxis::Horiz,
            LayoutMode::Grid2x1 if on_h => DragAxis::Vert,
            LayoutMode::Grid2x2 => match (on_v, on_h) {
                (true, true) => DragAxis::Both,
                (true, false) => DragAxis::Horiz,
                (false, true) => DragAxis::Vert,
                (false, false) => DragAxis::None,
            },
            _ => DragAxis::None,
        }
    }

    unsafe fn apply_splitter_drag(&mut self, x: i32, y: i32) {
        let m = self.compute_grid_metrics();
        let avail_w = (m.gw - m.gap).max(1) as f32;
        let avail_h = (m.gh - m.gap).max(1) as f32;
        if matches!(self.drag_axis, DragAxis::Horiz | DragAxis::Both) {
            let rx = ((x - m.gx - m.gap / 2) as f32 / avail_w).clamp(0.15, 0.85);
            self.split_x = rx;
        }
        if matches!(self.drag_axis, DragAxis::Vert | DragAxis::Both) {
            let ry = ((y - m.gy - m.gap / 2) as f32 / avail_h).clamp(0.15, 0.85);
            self.split_y = ry;
        }
        self.update_layout();
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn paint_splitters(&self) {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(self.hwnd, &mut ps);
        if hdc.is_null() {
            return;
        }
        if !self.dedicated && self.layout != LayoutMode::Grid1x1 {
            let m = self.compute_grid_metrics();
            let bg = CreateSolidBrush(rgb(232, 235, 240));
            let grip = CreateSolidBrush(rgb(165, 173, 184));
            let s = |v: i32| dpi_scale(v, self.theme.dpi);

            if matches!(self.layout, LayoutMode::Grid1x2 | LayoutMode::Grid2x2) {
                let vrc = RECT {
                    left: m.gx + m.cw,
                    top: m.gy,
                    right: m.gx + m.cw + m.gap,
                    bottom: m.gy + m.gh,
                };
                FillRect(hdc, &vrc, bg);
                let cx = vrc.left + m.gap / 2;
                let cy = (vrc.top + vrc.bottom) / 2;
                let grc = RECT {
                    left: cx - 1,
                    top: cy - s(18),
                    right: cx + 2,
                    bottom: cy + s(18),
                };
                FillRect(hdc, &grc, grip);
            }
            if matches!(self.layout, LayoutMode::Grid2x1 | LayoutMode::Grid2x2) {
                let hrc = RECT {
                    left: m.gx,
                    top: m.gy + m.ch,
                    right: m.gx + m.gw,
                    bottom: m.gy + m.ch + m.gap,
                };
                FillRect(hdc, &hrc, bg);
                let cx = (hrc.left + hrc.right) / 2;
                let cy = hrc.top + m.gap / 2;
                let grc = RECT {
                    left: cx - s(18),
                    top: cy - 1,
                    right: cx + s(18),
                    bottom: cy + 2,
                };
                FillRect(hdc, &grc, grip);
            }

            DeleteObject(bg);
            DeleteObject(grip);
        }
        EndPaint(self.hwnd, &ps);
    }

    unsafe fn update_layout(&self) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let w = (rc.right - rc.left).max(200);
        let dpi = self.theme.dpi;
        let s = |v: i32| dpi_scale(v, dpi);

        if self.dedicated {
            if let Some(&ph) = self.panes.first() {
                pane_set_visible(ph, true);
                MoveWindow(ph, 0, 0, w, (rc.bottom - rc.top).max(200), 1);
            }
            return;
        }

        let pad = s(5);
        let tb_h = s(24);
        let drop_h = s(180);
        let y = pad;

        set_window_text(
            self.btn_1x1,
            if self.layout == LayoutMode::Grid1x1 {
                "[1画面]"
            } else {
                "1画面"
            },
        );
        set_window_text(
            self.btn_1x2,
            if self.layout == LayoutMode::Grid1x2 {
                "[2画面横]"
            } else {
                "2画面横"
            },
        );
        set_window_text(
            self.btn_2x1,
            if self.layout == LayoutMode::Grid2x1 {
                "[2画面縦]"
            } else {
                "2画面縦"
            },
        );
        set_window_text(
            self.btn_2x2,
            if self.layout == LayoutMode::Grid2x2 {
                "[4画面]"
            } else {
                "4画面"
            },
        );

        let mut x = pad;
        let place = |btn: HWND, x: &mut i32, bw: i32| {
            MoveWindow(btn, *x, y, bw, tb_h, 1);
            *x += bw + s(4);
        };
        place(self.btn_1x1, &mut x, s(62));
        place(self.btn_1x2, &mut x, s(76));
        place(self.btn_2x1, &mut x, s(76));
        place(self.btn_2x2, &mut x, s(62));
        x += s(5);
        place(self.btn_new_win, &mut x, s(106));
        place(self.btn_refresh, &mut x, s(94));

        if !self.cmb_notify.is_null() {
            x += s(4);
            let nw = s(106);
            MoveWindow(self.cmb_notify, x, y, nw, drop_h, 1);
            x += nw + s(4);
        }

        let open_w = s(86);
        let open_x = (w - pad - open_w).max(x + s(6));
        MoveWindow(self.btn_open_logs, open_x, y, open_w, tb_h, 1);

        let api_x = x + s(6);
        let api_w = open_x - s(8) - api_x;
        if api_w >= s(130) {
            ShowWindow(self.lbl_api, SW_SHOW);
            MoveWindow(self.lbl_api, api_x, y, api_w, tb_h, 1);
        } else {
            ShowWindow(self.lbl_api, SW_HIDE);
        }

        // Layout grid area using dynamic split_x / split_y
        let m = self.compute_grid_metrics();
        let vis_count = self.layout.pane_count();
        for (i, &ph) in self.panes.iter().enumerate() {
            pane_set_visible(ph, i < vis_count);
        }

        match self.layout {
            LayoutMode::Grid1x1 => {
                MoveWindow(self.panes[0], m.gx, m.gy, m.gw, m.gh, 1);
            }
            LayoutMode::Grid1x2 => {
                let rw = (m.gw - m.cw - m.gap).max(40);
                MoveWindow(self.panes[0], m.gx, m.gy, m.cw, m.gh, 1);
                MoveWindow(self.panes[1], m.gx + m.cw + m.gap, m.gy, rw, m.gh, 1);
            }
            LayoutMode::Grid2x1 => {
                let bh = (m.gh - m.ch - m.gap).max(40);
                MoveWindow(self.panes[0], m.gx, m.gy, m.gw, m.ch, 1);
                MoveWindow(self.panes[1], m.gx, m.gy + m.ch + m.gap, m.gw, bh, 1);
            }
            LayoutMode::Grid2x2 => {
                let rw = (m.gw - m.cw - m.gap).max(40);
                let bh = (m.gh - m.ch - m.gap).max(40);
                MoveWindow(self.panes[0], m.gx, m.gy, m.cw, m.ch, 1);
                MoveWindow(self.panes[1], m.gx + m.cw + m.gap, m.gy, rw, m.ch, 1);
                MoveWindow(self.panes[2], m.gx, m.gy + m.ch + m.gap, m.cw, bh, 1);
                MoveWindow(
                    self.panes[3],
                    m.gx + m.cw + m.gap,
                    m.gy + m.ch + m.gap,
                    rw,
                    bh,
                    1,
                );
            }
        }
    }

    unsafe fn capture_state(&self) -> WindowStateConfig {
        let mut panes: Vec<PaneConfig> = self
            .panes
            .iter()
            .map(|&ph| pane_get_config(ph).unwrap_or_default())
            .collect();
        while panes.len() < 4 {
            panes.push(PaneConfig::default());
        }
        WindowStateConfig {
            rect: capture_window_rect(self.hwnd),
            layout: self.layout,
            split_x: self.split_x,
            split_y: self.split_y,
            dedicated: self.dedicated,
            panes,
        }
    }
}

unsafe fn refresh_all_windows_ports(
    manager: &Arc<MonitorManager>,
    hinst: HINSTANCE,
    trigger_notify: bool,
) {
    let (ports, newly_appeared) = manager.refresh_ports();
    let wins: Vec<HWND> = OPEN_WINDOWS.with(|list| list.borrow().clone());
    for wh in wins {
        if IsWindow(wh) != 0 {
            let ptr = GetWindowLongPtrW(wh, GWLP_USERDATA) as *mut MonitorWindowState;
            if !ptr.is_null() {
                for &ph in &(*ptr).panes {
                    pane_refresh_ports(ph, &ports);
                }
            }
        }
    }
    if trigger_notify && !newly_appeared.is_empty() {
        let mode = GLOBAL_NOTIFY_MODE.with(|m| *m.borrow());
        let owner = PRIMARY_HWND.with(|p| *p.borrow());
        let tray_ok = TRAY_READY.with(|t| *t.borrow());
        notify_new_devices(owner, hinst, mode, tray_ok, &newly_appeared);
    }
}

unsafe fn collect_all_used_ports() -> Vec<String> {
    let mut used = Vec::new();
    let wins: Vec<HWND> = OPEN_WINDOWS.with(|list| list.borrow().clone());
    for wh in wins {
        if IsWindow(wh) != 0 {
            let ptr = GetWindowLongPtrW(wh, GWLP_USERDATA) as *mut MonitorWindowState;
            if !ptr.is_null() {
                let st = &*ptr;
                for &ph in st.panes.iter().take(st.layout.pane_count()) {
                    if let Some(c) = pane_get_config(ph) {
                        if !c.port.is_empty() {
                            used.push(c.port);
                        }
                    }
                }
            }
        }
    }
    used
}

unsafe fn save_all_windows_config() {
    let wins: Vec<HWND> = OPEN_WINDOWS.with(|list| list.borrow().clone());
    let mut primary_cfg: Option<(u16, WindowStateConfig)> = None;
    let mut extras: Vec<WindowStateConfig> = Vec::new();

    for wh in wins {
        if IsWindow(wh) != 0 {
            let ptr = GetWindowLongPtrW(wh, GWLP_USERDATA) as *mut MonitorWindowState;
            if !ptr.is_null() {
                let st = &*ptr;
                let snap = st.capture_state();
                if st.is_primary && primary_cfg.is_none() {
                    primary_cfg = Some((st.api_port, snap));
                } else {
                    extras.push(snap);
                }
            }
        }
    }

    if let Some((api_port, main)) = primary_cfg {
        let notify_mode = GLOBAL_NOTIFY_MODE.with(|m| *m.borrow());
        let cfg = AppConfig {
            api_port,
            notify_mode,
            main_rect: main.rect,
            layout: main.layout,
            main_split_x: main.split_x,
            main_split_y: main.split_y,
            main_panes: main.panes,
            extra_windows: extras,
        };
        save_config(&cfg);
    }
}

unsafe fn capture_window_rect(hwnd: HWND) -> WindowRect {
    let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(hwnd, &mut wp) != 0 {
        let r = wp.rcNormalPosition;
        return WindowRect {
            x: r.left,
            y: r.top,
            width: (r.right - r.left).max(380),
            height: (r.bottom - r.top).max(280),
            maximized: wp.showCmd == SW_MAXIMIZE as u32,
        };
    }
    WindowRect::default()
}

unsafe fn create_tb_btn(
    parent: HWND,
    hinst: HINSTANCE,
    text: &str,
    id: isize,
    font: HFONT,
) -> HWND {
    let cls = to_wide("BUTTON");
    let txt = to_wide(text);
    let h = CreateWindowExW(
        0,
        cls.as_ptr(),
        txt.as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        0,
        0,
        10,
        10,
        parent,
        id as _,
        hinst,
        null(),
    );
    if !h.is_null() {
        SendMessageW(h, WM_SETFONT, font as WPARAM, 1);
    }
    h
}
