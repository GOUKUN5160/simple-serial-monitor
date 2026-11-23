use crate::json::{obj, Json};
use crate::ports::{enumerate_ports, port_sort_key};
use crate::serial::{PortSession, PortSnapshot};
use crate::types::{PortInfo, SerialConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct MonitorManager {
    ports: Mutex<Vec<PortInfo>>,
    sessions: Mutex<HashMap<String, Arc<PortSession>>>,
    pane_refs: Mutex<HashMap<String, usize>>,
    log_dir: PathBuf,
    api_port: u16,
}

impl MonitorManager {
    pub fn new(log_dir: PathBuf, api_port: u16) -> Arc<Self> {
        let initial_ports = enumerate_ports();
        let mgr = Arc::new(Self {
            ports: Mutex::new(initial_ports),
            sessions: Mutex::new(HashMap::new()),
            pane_refs: Mutex::new(HashMap::new()),
            log_dir,
            api_port,
        });
        mgr.write_status_file();
        mgr
    }

    pub fn log_dir(&self) -> &PathBuf {
        &self.log_dir
    }

    pub fn api_port(&self) -> u16 {
        self.api_port
    }

    /// Refresh active COM ports and return `(all_ports, newly_appeared_ports)`.
    pub fn refresh_ports(&self) -> (Vec<PortInfo>, Vec<PortInfo>) {
        let latest = enumerate_ports();
        let newly_appeared = {
            let mut g = self.ports.lock().unwrap();
            let prev_names: Vec<String> = g.iter().map(|p| p.name.clone()).collect();
            let added: Vec<PortInfo> = latest
                .iter()
                .filter(|p| !prev_names.iter().any(|old| old.eq_ignore_ascii_case(&p.name)))
                .cloned()
                .collect();
            *g = latest.clone();
            added
        };
        // Update is_usb_cdc flags & notify existing sessions of hardware changes
        let sessions: Vec<Arc<PortSession>> = {
            let g = self.sessions.lock().unwrap();
            g.values().cloned().collect()
        };
        for sess in sessions {
            if let Some(info) = latest
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(&sess.port_name))
            {
                sess.set_is_usb_cdc(info.is_usb_cdc);
            }
            sess.notify_device_changed();
        }
        self.write_status_file();
        (latest, newly_appeared)
    }

    pub fn list_ports(&self) -> Vec<PortInfo> {
        self.ports.lock().unwrap().clone()
    }

    pub fn find_port_info(&self, port_name: &str) -> Option<PortInfo> {
        let upper = normalize_port(port_name);
        self.ports
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == upper)
            .cloned()
    }

    pub fn get_session(&self, port_name: &str) -> Option<Arc<PortSession>> {
        let upper = normalize_port(port_name);
        self.sessions.lock().unwrap().get(&upper).cloned()
    }

    pub fn acquire_pane_ref(&self, port_name: &str) {
        let upper = normalize_port(port_name);
        if upper.is_empty() {
            return;
        }
        let mut refs = self.pane_refs.lock().unwrap();
        *refs.entry(upper).or_insert(0) += 1;
    }

    /// Decrement pane reference count for a port; when no panes watch it, release the OS COM handle.
    pub fn release_pane_ref(&self, port_name: &str) {
        let upper = normalize_port(port_name);
        if upper.is_empty() {
            return;
        }
        let should_disconnect = {
            let mut refs = self.pane_refs.lock().unwrap();
            if let Some(cnt) = refs.get_mut(&upper) {
                *cnt = cnt.saturating_sub(1);
                if *cnt == 0 {
                    refs.remove(&upper);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if should_disconnect {
            if let Some(sess) = self.get_session(&upper) {
                sess.disconnect();
            }
            self.write_status_file();
        }
    }

    pub fn get_or_create_session(
        &self,
        port_name: &str,
        initial_cfg: SerialConfig,
        auto_connect: bool,
    ) -> Arc<PortSession> {
        let upper = normalize_port(port_name);
        let is_cdc = self
            .find_port_info(&upper)
            .map(|p| p.is_usb_cdc)
            .unwrap_or(false);

        let mut g = self.sessions.lock().unwrap();
        if let Some(existing) = g.get(&upper) {
            return Arc::clone(existing);
        }
        let sess = PortSession::start(
            upper.clone(),
            initial_cfg,
            is_cdc,
            auto_connect,
            self.log_dir.clone(),
        );
        g.insert(upper, Arc::clone(&sess));
        drop(g);
        self.write_status_file();
        sess
    }

    /// Build combined JSON list of all enumerated ports + any active session ports (even if unplugged).
    pub fn ports_status_json(&self) -> Json {
        let enumerated = self.list_ports();
        let sessions: Vec<(String, PortSnapshot)> = {
            let g = self.sessions.lock().unwrap();
            g.iter().map(|(k, v)| (k.clone(), v.snapshot())).collect()
        };

        let mut names: Vec<String> = enumerated.iter().map(|p| p.name.clone()).collect();
        for (name, _) in &sessions {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.sort_by_key(|n| port_sort_key(n));

        let items: Vec<Json> = names
            .into_iter()
            .map(|name| {
                let info = enumerated.iter().find(|p| p.name == name);
                let snap = sessions.iter().find(|(k, _)| k == &name).map(|(_, s)| s);

                let (state_str, owner_json, config_json, detected_eol, latest_seq, log_file) =
                    if let Some(s) = snap {
                        (
                            s.state.code().to_string(),
                            s.state
                                .owner()
                                .map(|o| o.to_json())
                                .unwrap_or(Json::Null),
                            s.config.to_json(),
                            s.detected_eol.as_str().to_string(),
                            s.latest_seq,
                            s.log_file.clone(),
                        )
                    } else {
                        (
                            "unmonitored".to_string(),
                            Json::Null,
                            Json::Null,
                            "none".to_string(),
                            0,
                            String::new(),
                        )
                    };

                let mut fields = vec![
                    ("port", Json::Str(name.clone())),
                    ("present", Json::Bool(info.is_some())),
                    (
                        "hardware_name",
                        Json::Str(
                            info.map(|i| i.hardware_label.clone())
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "friendly_name",
                        Json::Str(
                            info.map(|i| i.friendly_name.clone())
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "manufacturer",
                        Json::Str(
                            info.map(|i| i.manufacturer.clone())
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "vid",
                        info.and_then(|i| i.vid)
                            .map(|v| Json::Str(format!("{:04X}", v)))
                            .unwrap_or(Json::Null),
                    ),
                    (
                        "pid",
                        info.and_then(|i| i.pid)
                            .map(|v| Json::Str(format!("{:04X}", v)))
                            .unwrap_or(Json::Null),
                    ),
                    ("state", Json::Str(state_str)),
                    ("owner", owner_json),
                    ("config", config_json),
                    ("detected_eol", Json::Str(detected_eol)),
                    ("latest_seq", Json::Num(latest_seq as f64)),
                    ("log_file", Json::Str(log_file)),
                ];
                if let Some(s) = snap {
                    if let crate::types::ConnectionState::AutoDetecting { probing_baud } = s.state {
                        fields.push(("probing_baud", Json::Num(probing_baud as f64)));
                    }
                }
                obj(fields)
            })
            .collect();

        Json::Arr(items)
    }

    pub fn write_status_file(&self) {
        let status = obj(vec![
            ("app", Json::Str("SimpleSerialMonitor".to_string())),
            ("version", Json::Str(env!("CARGO_PKG_VERSION").to_string())),
            ("api_url", Json::Str(format!("http://127.0.0.1:{}", self.api_port))),
            ("log_dir", Json::Str(self.log_dir.to_string_lossy().to_string())),
            ("ports", self.ports_status_json()),
        ]);
        let path = self.log_dir.join("status.json");
        let tmp = self.log_dir.join("status.json.tmp");
        if std::fs::write(&tmp, status.to_json_pretty()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn shutdown_all(&self) {
        let sessions: Vec<Arc<PortSession>> = {
            let g = self.sessions.lock().unwrap();
            g.values().cloned().collect()
        };
        for s in sessions {
            s.shutdown();
        }
    }
}

pub fn normalize_port(name: &str) -> String {
    name.trim()
        .trim_start_matches("\\\\.\\")
        .to_ascii_uppercase()
}
