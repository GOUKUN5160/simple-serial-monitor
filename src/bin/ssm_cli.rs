use simple_serial_monitor::config::DEFAULT_API_PORT;
use simple_serial_monitor::json::{obj, parse, Json};
use simple_serial_monitor::manager::normalize_port;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn url_encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

fn app_bin_name() -> &'static str {
    if cfg!(windows) {
        "ssm.exe"
    } else {
        "ssm"
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut api_port: u16 = std::env::var("SSM_API_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_API_PORT);

    // Extract optional global --api-port <N>
    if let Some(pos) = args.iter().position(|a| a == "--api-port") {
        if pos + 1 < args.len() {
            if let Ok(p) = args[pos + 1].parse() {
                api_port = p;
            }
            args.drain(pos..=pos + 1);
        }
    }

    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print_usage();
        return;
    }

    let cmd = args[0].as_str();
    let rest = &args[1..];

    let exit_code = match cmd {
        "list" | "ports" => cmd_list(api_port, rest),
        "read" | "logs" => cmd_read(api_port, rest),
        "send" => cmd_send(api_port, rest),
        "connect" => cmd_connect(api_port, rest),
        "disconnect" | "release" => cmd_simple_action(api_port, rest, "disconnect"),
        "autodetect" => cmd_simple_action(api_port, rest, "autodetect"),
        "clear" => cmd_simple_action(api_port, rest, "clear"),
        "kill-owner" => cmd_simple_action(api_port, rest, "kill-owner"),
        "status" => cmd_status(api_port),
        "mcp" => {
            run_mcp_server(api_port);
            0
        }
        _ => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
            1
        }
    };

    std::process::exit(exit_code);
}

fn print_usage() {
    println!(
        "Simple Serial Monitor CLI & MCP Server (ssm-cli)

Usage:
  ssm-cli [--api-port 27182] <command> [options]

Commands:
  list [--json] [--refresh]                   List active COM ports & monitor status
  read <PORT> [--tail N] [--since-seq N] [--follow] [--json]
                                              Read serial output simultaneously with GUI
  send <PORT> <TEXT> [--eol auto|crlf|lf|cr|none]
                                              Send data to a COM port
  connect <PORT> [--baud N] [--auto]          Connect/open a COM port in the monitor
  disconnect <PORT>                           Release COM port (e.g. before flashing firmware)
  autodetect <PORT>                           Trigger baud/framing auto-detection
  clear <PORT>                                Clear log buffer and log file
  kill-owner <PORT>                           Terminate process holding the COM port & connect
  status                                      Print full JSON status
  mcp                                         Start stdio MCP server (for Antigravity / Codex)"
    );
}

fn http_request(
    api_port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", api_port)).map_err(|e| {
        format!(
            "{} (127.0.0.1:{}) に接続できません: {} (先に {} を起動してください)",
            app_bin_name(), api_port, e, app_bin_name()
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let payload = body.unwrap_or("");
    let req = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        method,
        path,
        api_port,
        payload.len(),
        payload
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write error: {}", e))?;

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| format!("read error: {}", e))?;

    let (head, resp_body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid HTTP response".to_string())?;
    let status_code = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(500);
    Ok((status_code, resp_body.to_string()))
}

fn cmd_status(api_port: u16) -> i32 {
    match http_request(api_port, "GET", "/api/status", None) {
        Ok((_, body)) => {
            println!("{}", body);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_list(api_port: u16, args: &[String]) -> i32 {
    let json_mode = args.iter().any(|a| a == "--json");
    let refresh = args.iter().any(|a| a == "--refresh");
    let path = if refresh {
        "/api/ports?refresh=1"
    } else {
        "/api/ports"
    };
    match http_request(api_port, "GET", path, None) {
        Ok((_, body)) => {
            if json_mode {
                println!("{}", body);
                return 0;
            }
            if let Ok(j) = parse(&body) {
                if let Some(arr) = j.get("ports").and_then(|v| v.as_arr()) {
                    if arr.is_empty() {
                        println!("アクティブなCOMポートはありません。");
                        return 0;
                    }
                    for item in arr {
                        let port = item.get("port").and_then(|v| v.as_str()).unwrap_or("?");
                        let hw = item
                            .get("hardware_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let state = item.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                        let baud = item
                            .get("config")
                            .and_then(|c| c.get("baud"))
                            .and_then(|v| v.as_u64())
                            .map(|b| format!("{}bps", b))
                            .unwrap_or_default();
                        let owner = item
                            .get("owner")
                            .and_then(|o| {
                                let name = o.get("name")?.as_str()?;
                                let pid = o.get("pid")?.as_u64()?;
                                Some(format!("owner={} (PID {})", name, pid))
                            })
                            .unwrap_or_default();
                        println!("{:<24} | {:<13} | {:<10} | {} {}", port, state, baud, hw, owner);
                    }
                    return 0;
                }
            }
            println!("{}", body);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_read(api_port: u16, args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: ssm-cli read <PORT> [--tail N] [--since-seq N] [--follow] [--json]");
        return 1;
    }
    let port = normalize_port(&args[0]);
    let port_enc = url_encode_segment(&port);
    let mut tail: usize = 100;
    let mut since_seq: u64 = 0;
    let mut follow = false;
    let mut json_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--tail" if i + 1 < args.len() => {
                tail = args[i + 1].parse().unwrap_or(100);
                i += 2;
            }
            "--since-seq" if i + 1 < args.len() => {
                since_seq = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--follow" | "-f" => {
                follow = true;
                i += 1;
            }
            "--json" => {
                json_mode = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    if follow {
        return stream_follow(api_port, &port_enc, since_seq, json_mode);
    }

    let fmt = if json_mode { "json" } else { "text" };
    let path = format!(
        "/api/logs/{}?tail={}&since_seq={}&format={}&auto_open=1",
        port_enc, tail, since_seq, fmt
    );
    match http_request(api_port, "GET", &path, None) {
        Ok((_, body)) => {
            print!("{}", body);
            if !body.ends_with('\n') && !body.is_empty() {
                println!();
            }
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn stream_follow(api_port: u16, port: &str, since_seq: u64, json_mode: bool) -> i32 {
    let mut stream = match TcpStream::connect(("127.0.0.1", api_port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error connecting to ssm.exe: {}", e);
            return 1;
        }
    };
    let req = format!(
        "GET /api/stream/{}?since_seq={}&format=ndjson HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: keep-alive\r\n\r\n",
        port, since_seq, api_port
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return 1;
    }

    let mut reader = BufReader::new(stream);
    // Skip HTTP response headers
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return 1;
        }
        if line.trim().is_empty() {
            break;
        }
    }

    let mut stdout = io::stdout();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if json_mode {
            let _ = writeln!(stdout, "{}", trimmed);
        } else if let Ok(j) = parse(trimmed) {
            let t = j
                .get("datetime")
                .or_else(|| j.get("time"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let txt = j.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let _ = writeln!(stdout, "[{}] {}", t, txt);
        } else {
            let _ = writeln!(stdout, "{}", trimmed);
        }
        let _ = stdout.flush();
    }
    0
}

fn cmd_send(api_port: u16, args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("Usage: ssm-cli send <PORT> <TEXT> [--eol auto|crlf|lf|cr|none]");
        return 1;
    }
    let port = url_encode_segment(&normalize_port(&args[0]));
    let data = &args[1];
    let mut eol = "auto";
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--eol" && i + 1 < args.len() {
            eol = &args[i + 1];
            i += 2;
        } else {
            i += 1;
        }
    }
    let body = obj(vec![
        ("data", Json::Str(data.clone())),
        ("eol", Json::Str(eol.to_string())),
    ])
    .to_json_string();

    match http_request(api_port, "POST", &format!("/api/send/{}", port), Some(&body)) {
        Ok((_, resp)) => {
            println!("{}", resp);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_connect(api_port: u16, args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: ssm-cli connect <PORT> [--baud N] [--auto]");
        return 1;
    }
    let port = url_encode_segment(&normalize_port(&args[0]));
    let mut fields = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--baud" && i + 1 < args.len() {
            if let Ok(b) = args[i + 1].parse::<u32>() {
                fields.push(("baud", Json::Num(b as f64)));
            }
            i += 2;
        } else if args[i] == "--auto" {
            fields.push(("auto_detect", Json::Bool(true)));
            i += 1;
        } else {
            i += 1;
        }
    }
    let body = obj(fields).to_json_string();
    match http_request(
        api_port,
        "POST",
        &format!("/api/ports/{}/connect", port),
        Some(&body),
    ) {
        Ok((_, resp)) => {
            println!("{}", resp);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_simple_action(api_port: u16, args: &[String], action: &str) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: ssm-cli {} <PORT>", action);
        return 1;
    }
    let port = url_encode_segment(&normalize_port(&args[0]));
    match http_request(
        api_port,
        "POST",
        &format!("/api/ports/{}/{}", port, action),
        Some("{}"),
    ) {
        Ok((_, resp)) => {
            println!("{}", resp);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

// ============================================================================
// Lightweight stdio MCP Server (JSON-RPC 2.0) for Antigravity / Codex / Claude
// ============================================================================

fn run_mcp_server(api_port: u16) {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line_res in stdin.lock().lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req = match parse(trimmed) {
            Ok(j) => j,
            Err(_) => continue,
        };

        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // JSON-RPC notifications (no id) require no response
        let Some(req_id) = id else {
            continue;
        };

        let resp = match method {
            "initialize" => mcp_ok(
                req_id,
                obj(vec![
                    ("protocolVersion", Json::Str("2024-11-05".into())),
                    (
                        "capabilities",
                        obj(vec![("tools", obj(vec![]))]),
                    ),
                    (
                        "serverInfo",
                        obj(vec![
                            ("name", Json::Str("simple-serial-monitor".into())),
                            ("version", Json::Str(env!("CARGO_PKG_VERSION").into())),
                        ]),
                    ),
                ]),
            ),
            "ping" => mcp_ok(req_id, obj(vec![])),
            "tools/list" => mcp_ok(req_id, obj(vec![("tools", mcp_tools_schema())])),
            "tools/call" => {
                let params = req.get("params");
                let name = params
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = params.and_then(|p| p.get("arguments"));
                let (is_err, text) = handle_mcp_tool(api_port, name, args);
                mcp_ok(
                    req_id,
                    obj(vec![
                        (
                            "content",
                            Json::Arr(vec![obj(vec![
                                ("type", Json::Str("text".into())),
                                ("text", Json::Str(text)),
                            ])]),
                        ),
                        ("isError", Json::Bool(is_err)),
                    ]),
                )
            }
            _ => obj(vec![
                ("jsonrpc", Json::Str("2.0".into())),
                ("id", req_id),
                (
                    "error",
                    obj(vec![
                        ("code", Json::Num(-32601.0)),
                        ("message", Json::Str(format!("Method not found: {}", method))),
                    ]),
                ),
            ]),
        };

        let _ = writeln!(stdout, "{}", resp.to_json_string());
        let _ = stdout.flush();
    }
}

fn mcp_ok(id: Json, result: Json) -> Json {
    obj(vec![
        ("jsonrpc", Json::Str("2.0".into())),
        ("id", id),
        ("result", result),
    ])
}

fn mcp_tools_schema() -> Json {
    Json::Arr(vec![
        obj(vec![
            ("name", Json::Str("list_serial_ports".into())),
            (
                "description",
                Json::Str("List active COM ports, hardware names (ESP32/Pico/Arduino/CH340/CP210x), connection status, baud/framing, occupying process, and log file paths.".into()),
            ),
            (
                "inputSchema",
                obj(vec![
                    ("type", Json::Str("object".into())),
                    (
                        "properties",
                        obj(vec![(
                            "refresh",
                            obj(vec![("type", Json::Str("boolean".into()))]),
                        )]),
                    ),
                ]),
            ),
        ]),
        obj(vec![
            ("name", Json::Str("read_serial_log".into())),
            (
                "description",
                Json::Str("Read live serial monitor logs from a COM port simultaneously with the human GUI without port conflict. Pass since_seq from the previous latest_seq for incremental polling.".into()),
            ),
            (
                "inputSchema",
                obj(vec![
                    ("type", Json::Str("object".into())),
                    (
                        "properties",
                        obj(vec![
                            ("port", obj(vec![("type", Json::Str("string".into()))])),
                            ("tail", obj(vec![("type", Json::Str("integer".into()))])),
                            ("since_seq", obj(vec![("type", Json::Str("integer".into()))])),
                        ]),
                    ),
                    ("required", Json::Arr(vec![Json::Str("port".into())])),
                ]),
            ),
        ]),
        obj(vec![
            ("name", Json::Str("send_serial_data".into())),
            (
                "description",
                Json::Str("Send text/command to a COM port via Simple Serial Monitor.".into()),
            ),
            (
                "inputSchema",
                obj(vec![
                    ("type", Json::Str("object".into())),
                    (
                        "properties",
                        obj(vec![
                            ("port", obj(vec![("type", Json::Str("string".into()))])),
                            ("data", obj(vec![("type", Json::Str("string".into()))])),
                            ("eol", obj(vec![("type", Json::Str("string".into()))])),
                        ]),
                    ),
                    (
                        "required",
                        Json::Arr(vec![Json::Str("port".into()), Json::Str("data".into())]),
                    ),
                ]),
            ),
        ]),
        obj(vec![
            ("name", Json::Str("control_serial_port".into())),
            (
                "description",
                Json::Str("Control a COM port in Simple Serial Monitor: connect, disconnect (release port before flashing firmware), autodetect, clear, or kill-owner.".into()),
            ),
            (
                "inputSchema",
                obj(vec![
                    ("type", Json::Str("object".into())),
                    (
                        "properties",
                        obj(vec![
                            ("port", obj(vec![("type", Json::Str("string".into()))])),
                            ("action", obj(vec![("type", Json::Str("string".into()))])),
                            ("baud", obj(vec![("type", Json::Str("integer".into()))])),
                        ]),
                    ),
                    (
                        "required",
                        Json::Arr(vec![Json::Str("port".into()), Json::Str("action".into())]),
                    ),
                ]),
            ),
        ]),
    ])
}

fn handle_mcp_tool(api_port: u16, name: &str, args: Option<&Json>) -> (bool, String) {
    match name {
        "list_serial_ports" => {
            let refresh = args
                .and_then(|a| a.get("refresh"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let path = if refresh {
                "/api/ports?refresh=1"
            } else {
                "/api/ports"
            };
            match http_request(api_port, "GET", path, None) {
                Ok((_, b)) => (false, b),
                Err(e) => (true, e),
            }
        }
        "read_serial_log" => {
            let port = match args.and_then(|a| a.get("port")).and_then(|v| v.as_str()) {
                Some(p) => url_encode_segment(&normalize_port(p)),
                None => return (true, "missing required parameter: port".into()),
            };
            let tail = args
                .and_then(|a| a.get("tail"))
                .and_then(|v| v.as_u64())
                .unwrap_or(100);
            let since_seq = args
                .and_then(|a| a.get("since_seq"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let path = format!(
                "/api/logs/{}?tail={}&since_seq={}&format=json&auto_open=1",
                port, tail, since_seq
            );
            match http_request(api_port, "GET", &path, None) {
                Ok((_, b)) => (false, b),
                Err(e) => (true, e),
            }
        }
        "send_serial_data" => {
            let port = match args.and_then(|a| a.get("port")).and_then(|v| v.as_str()) {
                Some(p) => url_encode_segment(&normalize_port(p)),
                None => return (true, "missing required parameter: port".into()),
            };
            let data = args
                .and_then(|a| a.get("data"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let eol = args
                .and_then(|a| a.get("eol"))
                .and_then(|v| v.as_str())
                .unwrap_or("auto");
            let body = obj(vec![
                ("data", Json::Str(data.to_string())),
                ("eol", Json::Str(eol.to_string())),
            ])
            .to_json_string();
            match http_request(api_port, "POST", &format!("/api/send/{}", port), Some(&body)) {
                Ok((_, b)) => (false, b),
                Err(e) => (true, e),
            }
        }
        "control_serial_port" => {
            let port = match args.and_then(|a| a.get("port")).and_then(|v| v.as_str()) {
                Some(p) => url_encode_segment(&normalize_port(p)),
                None => return (true, "missing required parameter: port".into()),
            };
            let action = match args.and_then(|a| a.get("action")).and_then(|v| v.as_str()) {
                Some(a) => a,
                None => return (true, "missing required parameter: action".into()),
            };
            let mut fields = Vec::new();
            if let Some(baud) = args.and_then(|a| a.get("baud")).and_then(|v| v.as_u64()) {
                fields.push(("baud", Json::Num(baud as f64)));
            }
            let body = obj(fields).to_json_string();
            match http_request(
                api_port,
                "POST",
                &format!("/api/ports/{}/{}", port, action),
                Some(&body),
            ) {
                Ok((_, b)) => (false, b),
                Err(e) => (true, e),
            }
        }
        _ => (true, format!("Unknown tool: {}", name)),
    }
}
