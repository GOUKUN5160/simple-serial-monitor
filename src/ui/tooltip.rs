use crate::ui::to_wide;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::UI::Controls::{
    GetComboBoxInfo, COMBOBOXINFO, TTF_IDISHWND, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_SETDELAYTIME,
    TTM_SETMAXTIPWIDTH, TTDT_AUTOPOP, TTDT_INITIAL, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, CW_USEDEFAULT, WS_EX_TOPMOST, WS_POPUP,
};

pub unsafe fn create_tooltip_window(owner: HWND, hinst: HINSTANCE) -> HWND {
    let cls = to_wide("tooltips_class32");
    let tip = CreateWindowExW(
        WS_EX_TOPMOST,
        cls.as_ptr(),
        null(),
        WS_POPUP | TTS_NOPREFIX | TTS_ALWAYSTIP,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        owner,
        null_mut(),
        hinst,
        null(),
    );
    if !tip.is_null() {
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, 340);
        SendMessageW(tip, TTM_SETDELAYTIME, TTDT_INITIAL as WPARAM, 350);
        SendMessageW(tip, TTM_SETDELAYTIME, TTDT_AUTOPOP as WPARAM, 12000);
    }
    tip
}

pub unsafe fn add_tooltip(tip: HWND, owner: HWND, ctrl: HWND, text: &str) {
    if tip.is_null() || ctrl.is_null() {
        return;
    }
    let mut wide = to_wide(text);
    let ti = TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: owner,
        uId: ctrl as usize,
        rect: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        hinst: null_mut(),
        lpszText: wide.as_mut_ptr(),
        lParam: 0,
        lpReserved: null_mut(),
    };
    SendMessageW(tip, TTM_ADDTOOLW, 0, &ti as *const TTTOOLINFOW as LPARAM);
}

/// Registers a tooltip on a ComboBox AND its inner edit child (for CBS_DROPDOWN).
pub unsafe fn add_combo_tooltip(tip: HWND, owner: HWND, cmb: HWND, text: &str) {
    add_tooltip(tip, owner, cmb, text);
    let mut cbi: COMBOBOXINFO = std::mem::zeroed();
    cbi.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
    if GetComboBoxInfo(cmb, &mut cbi) != 0 && !cbi.hwndItem.is_null() && cbi.hwndItem != cmb {
        add_tooltip(tip, owner, cbi.hwndItem, text);
    }
}
