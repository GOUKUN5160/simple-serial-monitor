use crate::json::{obj, parse, Json};
use crate::manager::{normalize_port, MonitorManager};
use crate::types::{LineEnding, Parity, SerialConfig, StopBits};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub fn start_agent_server(manager: Arc<MonitorManager>, port: u16) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port)).ok()?;
    let actual_port = listener.local_addr().ok()?.port();

    let _ = thread::Builder::new()
        .name("agent-http".to_string())
        .spawn(move || {
            for incoming in listener.incoming() {
                if let Ok(stream) = incoming {
                    let mgr = Arc::clone(&manager);
                    let _ = thread::spawn(move || handle_client(stream, mgr));
                }
            }
        });

    Some(actual_port)
}

fn handle_client(mut stream: TcpStream, manager: Arc<MonitorManager>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(_) => return,
    };

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let full_target = parts[1];

    // Read headers
    let mut content_length: usize = 0;
    let mut content_type = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            if kl == "content-length" {
                content_length = v.trim().parse().unwrap_or(0).min(1024 * 1024);
            } else if kl == "content-type" {
                content_type = v.trim().to_ascii_lowercase();
            }
        }
    }

    // Read body if present
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body_str = String::from_utf8_lossy(&body).to_string();

    let (path, query) = match full_target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (full_target, ""),
    };
    let params = parse_query(query);

    // Route requests
    if method == "OPTIONS" {
        write_http(&mut stream, "204 No Content", "text/plain", "");
        return;
    }

    if method == "GET" && (path == "/" || path == "/api/status") {
        let resp = obj(vec![
            ("app", Json::Str("SimpleSerialMonitor".into())),
            ("version", Json::Str(env!("CARGO_PKG_VERSION").into())),
            (
                "api_url",
                Json::Str(format!("http://127.0.0.1:{}", manager.api_port())),
            ),
            (
                "log_dir",
                Json::Str(manager.log_dir().to_string_lossy().to_string()),
            ),
            ("ports", manager.ports_status_json()),
        ]);
        write_http(
            &mut stream,
            "200 OK",
            "application/json; charset=utf-8",
            &resp.to_json_string(),
        );
        return;
    }

    if method == "GET" && path == "/api/ports" {
        if get_param(&params, "refresh").as_deref() == Some("1") {
            manager.refresh_ports();
        }
        let resp = obj(vec![("ports", manager.ports_status_json())]);
        write_http(
            &mut stream,
            "200 OK",
            "application/json; charset=utf-8",
            &resp.to_json_string(),
        );
        return;
    }

    if method == "GET" {
        if let Some(port_raw) = path.strip_prefix("/api/logs/") {
            let port = normalize_port(&url_decode(port_raw));
            let tail: usize = get_param(&params, "tail")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200);
            let since_seq: u64 = get_param(&params, "since_seq")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let fmt = get_param(&params, "format").unwrap_or_else(|| "json".into());

            let session = match manager.get_session(&port) {
                Some(s) => s,
                None if get_param(&params, "auto_open").as_deref() == Some("1") => {
                    manager.get_or_create_session(&port, SerialConfig::default(), true)
                }
                None => {
                    let resp = obj(vec![
                        ("port", Json::Str(port)),
                        ("state", Json::Str("unmonitored".into())),
                        ("latest_seq", Json::Num(0.0)),
                        ("entries", Json::Arr(vec![])),
                    ]);
                    write_http(
                        &mut stream,
                        "200 OK",
                        "application/json; charset=utf-8",
                        &resp.to_json_string(),
                    );
                    return;
                }
            };

            let snap = session.snapshot();
            let entries = session.read_logs(since_seq, tail);

            if fmt == "text" {
                let mut out = String::new();
                for e in &entries {
                    out.push('[');
                    out.push_str(&e.datetime_str);
                    out.push_str("] ");
                    out.push_str(&e.text);
                    out.push('\n');
                }
                write_http(&mut stream, "200 OK", "text/plain; charset=utf-8", &out);
            } else {
                let resp = obj(vec![
                    ("port", Json::Str(snap.port)),
                    ("state", Json::Str(snap.state.code().to_string())),
                    (
                        "owner",
                        snap.state
                            .owner()
                            .map(|o| o.to_json())
                            .unwrap_or(Json::Null),
                    ),
                    ("config", snap.config.to_json()),
                    ("detected_eol", Json::Str(snap.detected_eol.as_str().into())),
                    ("latest_seq", Json::Num(snap.latest_seq as f64)),
                    ("log_file", Json::Str(snap.log_file)),
                    (
                        "entries",
                        Json::Arr(entries.iter().map(|e| e.to_json()).collect()),
                    ),
                ]);
                write_http(
                    &mut stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    &resp.to_json_string(),
                );
            }
            return;
        }

        if let Some(port_raw) = path.strip_prefix("/api/stream/") {
            let port = normalize_port(&url_decode(port_raw));
            let mut since_seq: u64 = get_param(&params, "since_seq")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let fmt = get_param(&params, "format").unwrap_or_else(|| "sse".into());
            let session = manager.get_or_create_session(&port, SerialConfig::default(), true);

            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            let ctype = if fmt == "ndjson" {
                "application/x-ndjson; charset=utf-8"
            } else {
                "text/event-stream; charset=utf-8"
            };
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
                ctype
            );
            if stream.write_all(hdr.as_bytes()).is_err() {
                return;
            }
            let _ = stream.flush();

            // Stream existing tail if since_seq == 0 -> start from current latest - 50
            if since_seq == 0 {
                let initial = session.read_logs(0, 50);
                for entry in initial {
                    since_seq = entry.seq;
                    if !write_stream_event(&mut stream, &fmt, &entry.to_json().to_json_string()) {
                        return;
                    }
                }
            }

            loop {
                let batch = session.wait_logs_since(since_seq, Duration::from_secs(15));
                if batch.is_empty() {
                    // Heartbeat comment for SSE to detect closed sockets
                    if fmt == "sse" {
                        if stream.write_all(b": keepalive\n\n").is_err() || stream.flush().is_err() {
                            return;
                        }
                    }
                    continue;
                }
                for entry in batch {
                    since_seq = entry.seq;
                    if !write_stream_event(&mut stream, &fmt, &entry.to_json().to_json_string()) {
                        return;
                    }
                }
            }
        }
    }

    if method == "POST" {
        if let Some(port_raw) = path.strip_prefix("/api/send/") {
            let port = normalize_port(&url_decode(port_raw));
            let session = manager.get_or_create_session(&port, SerialConfig::default(), true);
            let (data, eol_override) = if content_type.contains("json")
                || body_str.trim_start().starts_with('{')
            {
                if let Ok(j) = parse(&body_str) {
                    let d = j
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let e = j
                        .get("eol")
                        .and_then(|v| v.as_str())
                        .map(LineEnding::from_str);
                    (d, e)
                } else {
                    (body_str, None)
                }
            } else {
                (body_str, get_param(&params, "eol").map(|s| LineEnding::from_str(&s)))
            };

            session.send_text(&data, eol_override);
            let resp = obj(vec![
                ("ok", Json::Bool(true)),
                ("port", Json::Str(port)),
                ("sent", Json::Str(data)),
            ]);
            write_http(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                &resp.to_json_string(),
            );
            return;
        }

        if let Some(rest) = path.strip_prefix("/api/ports/") {
            if let Some((port_raw, action)) = rest.split_once('/') {
                let port = normalize_port(&url_decode(port_raw));
                let result = match action {
                    "connect" => {
                        let mut cfg = manager
                            .get_session(&port)
                            .map(|s| s.snapshot().config)
                            .unwrap_or_default();
                        if !body_str.trim().is_empty() {
                            if let Ok(j) = parse(&body_str) {
                                if let Some(b) = j.get("baud").and_then(|v| v.as_u64()) {
                                    cfg.baud = b as u32;
                                    cfg.auto_detect = false;
                                }
                                if let Some(a) = j.get("auto_detect").and_then(|v| v.as_bool()) {
                                    cfg.auto_detect = a;
                                }
                                if let Some(d) = j.get("data_bits").and_then(|v| v.as_u64()) {
                                    cfg.data_bits = d.clamp(5, 8) as u8;
                                }
                                if let Some(p) = j.get("parity").and_then(|v| v.as_str()) {
                                    cfg.parity = Parity::from_str(p);
                                }
                                if let Some(s) = j.get("stop_bits").and_then(|v| v.as_str()) {
                                    cfg.stop_bits = StopBits::from_str(s);
                                }
                                if let Some(e) = j.get("tx_eol").and_then(|v| v.as_str()) {
                                    cfg.tx_eol = LineEnding::from_str(e);
                                }
                            }
                        }
                        let sess = manager.get_or_create_session(&port, cfg, true);
                        sess.update_config(cfg);
                        sess.connect();
                        Ok("connected")
                    }
                    "disconnect" | "release" => {
                        if let Some(sess) = manager.get_session(&port) {
                            sess.disconnect();
                        }
                        Ok("disconnected")
                    }
                    "autodetect" => {
                        let sess = manager.get_or_create_session(&port, SerialConfig::default(), true);
                        sess.trigger_autodetect();
                        Ok("autodetecting")
                    }
                    "clear" => {
                        if let Some(sess) = manager.get_session(&port) {
                            sess.clear_logs();
                        }
                        Ok("cleared")
                    }
                    "kill-owner" => {
                        let sess = manager.get_or_create_session(&port, SerialConfig::default(), true);
                        sess.kill_owner_and_connect();
                        Ok("kill_owner_requested")
                    }
                    _ => Err("unknown action"),
                };

                manager.write_status_file();
                match result {
                    Ok(msg) => {
                        let resp = obj(vec![
                            ("ok", Json::Bool(true)),
                            ("port", Json::Str(port)),
                            ("action", Json::Str(msg.into())),
                        ]);
                        write_http(
                            &mut stream,
                            "200 OK",
                            "application/json; charset=utf-8",
                            &resp.to_json_string(),
                        );
                    }
                    Err(e) => {
                        let resp = obj(vec![("ok", Json::Bool(false)), ("error", Json::Str(e.into()))]);
                        write_http(
                            &mut stream,
                            "400 Bad Request",
                            "application/json; charset=utf-8",
                            &resp.to_json_string(),
                        );
                    }
                }
                return;
            }
        }
    }

    let err = obj(vec![("error", Json::Str("Not Found".into()))]);
    write_http(
        &mut stream,
        "404 Not Found",
        "application/json; charset=utf-8",
        &err.to_json_string(),
    );
}

fn write_stream_event(stream: &mut TcpStream, fmt: &str, json_line: &str) -> bool {
    let chunk = if fmt == "ndjson" {
        format!("{}\n", json_line)
    } else {
        format!("data: {}\n\n", json_line)
    };
    stream.write_all(chunk.as_bytes()).is_ok() && stream.flush().is_ok()
}

fn write_http(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n{}",
        status,
        content_type,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(kv), String::new()),
        })
        .collect()
}

fn get_param(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
