use crate::config::{load_config, save_config, AppConfig};
use crate::i18n::{set_lang, Lang};
use crate::json::{obj, parse, Json};
use crate::manager::MonitorManager;
use crate::types::{ConnectionState, DisplayMode, SerialConfig};
use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::{Arc, OnceLock};

static GLOBAL_MANAGER: OnceLock<Arc<MonitorManager>> = OnceLock::new();

extern "C" {
    fn ssm_macos_run_gui(initial_config_json: *const c_char, actual_api_port: c_int);
}

pub fn load_initial_config() -> AppConfig {
    load_config()
}

pub fn run_gui(manager: Arc<MonitorManager>, initial_config: AppConfig, api_port: Option<u16>) {
    let _ = GLOBAL_MANAGER.set(manager.clone());
    let cfg_json = initial_config.to_json().to_json_string();
    let c_cfg = CString::new(cfg_json).unwrap_or_else(|_| CString::new("{}").unwrap());
    let port_val: c_int = api_port.map(|p| p as c_int).unwrap_or(-1);
    unsafe {
        ssm_macos_run_gui(c_cfg.as_ptr(), port_val);
    }
    manager.shutdown_all();
}

fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr).to_str().unwrap_or("") }
}

fn into_c_string(s: String) -> *mut c_char {
    // Strip interior NUL bytes safely
    let clean = if s.as_bytes().contains(&0) {
        s.replace('\0', "")
    } else {
        s
    };
    CString::new(clean)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn ssm_free_str(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_get_ports_json(refresh: c_int) -> *mut c_char {
    let Some(mgr) = GLOBAL_MANAGER.get() else {
        return into_c_string(r#"{"ports":[],"added":[]}"#.to_string());
    };
    let (ports, added) = if refresh != 0 {
        mgr.refresh_ports()
    } else {
        (mgr.list_ports(), Vec::new())
    };
    let resp = obj(vec![
        (
            "ports",
            Json::Arr(ports.iter().map(|p| p.to_json()).collect()),
        ),
        (
            "added",
            Json::Arr(added.iter().map(|p| p.to_json()).collect()),
        ),
    ]);
    into_c_string(resp.to_json_string())
}

#[no_mangle]
pub extern "C" fn ssm_attach_pane(
    port: *const c_char,
    serial_cfg_json: *const c_char,
    auto_connect: c_int,
) {
    let Some(mgr) = GLOBAL_MANAGER.get() else {
        return;
    };
    let port_str = cstr_to_str(port);
    if port_str.is_empty() {
        return;
    }
    let cfg = parse(cstr_to_str(serial_cfg_json))
        .map(|j| SerialConfig::from_json(&j))
        .unwrap_or_default();
    let sess = mgr.get_or_create_session(port_str, cfg, auto_connect != 0);
    if auto_connect != 0 {
        let snap = sess.snapshot();
        if matches!(snap.state, ConnectionState::Released) {
            sess.connect();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_acquire_pane_ref(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        mgr.acquire_pane_ref(cstr_to_str(port));
    }
}

#[no_mangle]
pub extern "C" fn ssm_release_pane_ref(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        mgr.release_pane_ref(cstr_to_str(port));
    }
}

#[no_mangle]
pub extern "C" fn ssm_poll_pane_json(
    port: *const c_char,
    since_seq: u64,
    tail: usize,
) -> *mut c_char {
    let Some(mgr) = GLOBAL_MANAGER.get() else {
        return into_c_string("{}".to_string());
    };
    let port_str = cstr_to_str(port);
    let Some(sess) = mgr.get_session(port_str) else {
        return into_c_string(r#"{"has_session":false,"entries":[]}"#.to_string());
    };

    let snap = sess.snapshot();
    let entries = sess.read_logs(since_seq, tail);

    let mut state_fields = vec![
        ("code", Json::Str(snap.state.code().to_string())),
    ];
    match &snap.state {
        ConnectionState::AutoDetecting { probing_baud } => {
            state_fields.push(("probing_baud", Json::Num(*probing_baud as f64)));
        }
        ConnectionState::Disconnected { reason } => {
            state_fields.push(("reason", Json::Str(reason.clone())));
        }
        ConnectionState::Occupied { owner: Some(o) } => {
            state_fields.push(("owner", o.to_json()));
        }
        _ => {}
    }

    let entries_json: Vec<Json> = entries
        .iter()
        .map(|e| {
            obj(vec![
                ("seq", Json::Num(e.seq as f64)),
                ("time", Json::Str(e.time_str.clone())),
                ("datetime", Json::Str(e.datetime_str.clone())),
                ("kind", Json::Str(e.kind.as_str().to_string())),
                ("is_sys", Json::Bool(e.kind.is_system())),
                ("text", Json::Str(e.format_for_mode(DisplayMode::Text))),
                ("hex", Json::Str(e.format_for_mode(DisplayMode::Hex))),
                ("bin", Json::Str(e.format_for_mode(DisplayMode::Bin))),
            ])
        })
        .collect();

    let hw_label = mgr
        .find_port_info(port_str)
        .map(|p| p.hardware_label)
        .unwrap_or_default();

    let resp = obj(vec![
        ("has_session", Json::Bool(true)),
        ("port", Json::Str(snap.port)),
        ("hardware_label", Json::Str(hw_label)),
        ("state", obj(state_fields)),
        ("config", snap.config.to_json()),
        ("detected_eol", Json::Str(snap.detected_eol.as_str().to_string())),
        ("auto_locked", Json::Bool(snap.auto_locked)),
        ("latest_seq", Json::Num(snap.latest_seq as f64)),
        ("entries", Json::Arr(entries_json)),
    ]);

    into_c_string(resp.to_json_string())
}

#[no_mangle]
pub extern "C" fn ssm_connect(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.connect();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_disconnect(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.disconnect();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_autodetect(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.trigger_autodetect();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_kill_owner(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.kill_owner_and_connect();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_clear_logs(port: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.clear_logs();
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_update_serial_config(port: *const c_char, serial_cfg_json: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            if let Ok(j) = parse(cstr_to_str(serial_cfg_json)) {
                sess.update_config(SerialConfig::from_json(&j));
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_send_text(port: *const c_char, text: *const c_char) {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        let p = cstr_to_str(port);
        if let Some(sess) = mgr.get_session(p) {
            sess.send_text(cstr_to_str(text), None);
        }
    }
}

#[no_mangle]
pub extern "C" fn ssm_set_lang(lang_code: *const c_char) {
    if let Some(l) = Lang::from_str(cstr_to_str(lang_code)) {
        set_lang(l);
    }
}

#[no_mangle]
pub extern "C" fn ssm_save_config_json(config_json: *const c_char) {
    if let Ok(j) = parse(cstr_to_str(config_json)) {
        let cfg = AppConfig::from_json(&j);
        save_config(&cfg);
    }
}

#[no_mangle]
pub extern "C" fn ssm_get_log_dir() -> *mut c_char {
    let dir = GLOBAL_MANAGER
        .get()
        .map(|m| m.log_dir().to_string_lossy().to_string())
        .unwrap_or_default();
    into_c_string(dir)
}

#[no_mangle]
pub extern "C" fn ssm_shutdown_all() {
    if let Some(mgr) = GLOBAL_MANAGER.get() {
        mgr.shutdown_all();
    }
}
