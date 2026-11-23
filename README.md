# Simple Serial Monitor (`ssm`)

**English** | [日本語](README.ja.md)

An ultra-lightweight, native Win32 serial monitor built in Rust for embedded development with **ESP32**, **Raspberry Pi**, **Arduino**, and custom UART/binary devices.

Designed to be fast, minimal, and truly portable — no Electron, no WebView2, no .NET, and no Visual C++ Redistributable required. Just unzip and run (~500 KB standalone executable).

---

## Features

- **Auto-Detection & Manual Override**
  - Automatically detects baud rate (`115200`, `9600`, `74880`, `57600`, `38400`, `230400`, `460800`, `921600`, etc.), data bits (`8/7/6/5`), parity (`N/E/O/M/S`), stop bits (`1/1.5/2`), and line ending (`CRLF`/`LF`/`CR`).
  - Displays the currently active/detected value directly inside the dropdowns when set to Auto (e.g., `Auto (115200)`, `Auto (LF)`).
  - Supports manual overrides and arbitrary custom baud rates at any time.
- **Multi-Port Simultaneous Monitoring (Split Grid + Multi-Window)**
  - Switch between **1-pane**, **2-pane (horizontal / vertical)**, and **4-pane (2×2)** layouts in one click.
  - Drag the borders between split panes to resize them freely; click the layout button again to reset to equal sizes.
  - Open additional multi-pane windows (`+ 別ウィンドウ`) or pop out a dedicated single-COM-port window (`別窓`).
- **Automatic COM Port Enumeration & Hardware Names**
  - Detects USB hot-plugging automatically and identifies common hardware (ESP32, Raspberry Pi Pico/RP2xxx, Arduino, CP210x, CH340/CH341/CH9102, FTDI, STM32 ST-LINK, J-Link, etc.).
  - Configurable new-device notifications: **Off / In-App Popup / Windows Notification / Both**.
- **Port Occupation Detection & Process Termination**
  - When a COM port is locked by another program, displays the occupying process name and PID and lets you terminate it and reconnect in one click.
- **Clear Status Indicators & Auto-Reconnect**
  - Color-coded status bar (Connected / Auto-detecting / Disconnected & waiting / Occupied / Released) and automatic reconnection after USB unplug/replug or MCU reboot.
- **Binary & Custom Packet Support (`Text` / `HEX` / `BIN`)**
  - Always displays incoming bytes — even raw binary streams or non-UTF-8 packets without newlines.
  - Instantly switch buffered and live output between **Text** (lossy UTF-8 + visible control markers), **HEX** (hex dump + ASCII preview), and **BIN** (8-bit binary).
- **Rich, Minimal Virtual Log View**
  - Visually distinguishes system events (Connect, Disconnect, Auto-detect, Error) from device output and TX lines using centered tinted banners and distinct colors.
  - Smart auto-scroll: automatically pauses when you scroll up and resumes when you scroll back to the bottom.
  - Toggle line wrapping vs. horizontal scrolling (`Shift + Wheel` supported).
  - Zoom in/out/reset (`Ctrl + Wheel`, `Ctrl + +/-`, `Ctrl + 0`).
  - Hover tooltips on all controls and Per-Monitor V2 DPI scaling across mixed-resolution monitors.
- **Portable Layout & Session Persistence**
  - Saves window positions, split ratios, multi-window layouts, and per-port settings in `ssm_config.json` next to `ssm.exe` and restores them on startup.
- **Simultaneous Coding-Agent Interface (Antigravity / Codex / Claude Code)**
  - Lets AI coding agents read live serial output, send commands, and temporarily release/reconnect COM ports for firmware flashing while you watch the GUI — with zero port contention.

---

## Shortcuts & Controls

| Action | Shortcut / Operation |
| --- | --- |
| Send command | `Enter` in send box |
| Command history | `↑` / `↓` in send box |
| Zoom in / out / reset | `Ctrl + Wheel`, `Ctrl + +` / `-`, `Ctrl + 0` |
| Horizontal scroll (when Wrap is OFF) | `Shift + Wheel` or tilt/horizontal wheel |
| Copy / Select all | Drag to select + `Ctrl + C` / `Ctrl + A` |
| Resize / Reset split panes | Drag divider between panes / Click layout button again |

---

## Coding Agent Interface

While `ssm.exe` is running, coding agents and scripts can access serial ports concurrently with the GUI through any of 4 interfaces:

### 1. MCP Server (`ssm-cli.exe mcp`) — Recommended for Antigravity / Codex / Claude Code

Register `ssm-cli.exe` as a stdio MCP server:

```json
{
  "mcpServers": {
    "simple-serial-monitor": {
      "command": "C:\\path\\to\\SimpleSerialMonitor\\ssm-cli.exe",
      "args": ["mcp"]
    }
  }
}
```

Available tools:
- `list_serial_ports` — List active COM ports, hardware names, connection states, baud/framing, occupying processes, and log file paths.
- `read_serial_log` — Read buffered or incremental (`since_seq`) serial logs without port conflicts.
- `send_serial_data` — Send text/commands to a COM port.
- `control_serial_port` — Run `connect`, `disconnect` (release port before flashing firmware), `autodetect`, `clear`, or `kill-owner`.

### 2. CLI (`ssm-cli.exe`)

```powershell
# List ports and live status
.\ssm-cli.exe list

# Read recent logs or follow in real time alongside the GUI
.\ssm-cli.exe read COM9 --tail 50
.\ssm-cli.exe read COM9 --follow

# Send a command
.\ssm-cli.exe send COM9 "help"

# Release port before flashing, then reconnect
.\ssm-cli.exe disconnect COM9
# ... esptool / arduino-cli / cargo flash ...
.\ssm-cli.exe connect COM9
```

### 3. Local HTTP REST & SSE API (`http://127.0.0.1:27182`)

- `GET /api/status` — Server status and summary of all ports
- `GET /api/ports?refresh=1` — Active COM ports list (JSON)
- `GET /api/logs/COM9?tail=100&since_seq=0&format=json|text` — Read buffered logs
- `GET /api/stream/COM9?format=sse|ndjson` — Real-time log stream
- `POST /api/send/COM9` — Send data (`{"data":"...", "eol":"auto"}` or plain text)
- `POST /api/ports/COM9/connect` | `disconnect` | `autodetect` | `clear` | `kill-owner`

### 4. Real-Time Log Files (`logs/`)

Date-stamped UTF-8 logs (`[YYYY-MM-DD HH:MM:SS.mmm]`) are flushed in real time to `logs/<PORT>.log` (opened with full read/write sharing) alongside `logs/status.json`.

---

## Building from Source

Requires Rust (`x86_64-pc-windows-msvc`) on Windows:

```powershell
cargo build --release
```

To build the portable ZIP archive:

```powershell
powershell -ExecutionPolicy Bypass -File .\package_portable.ps1
```

---

<sub>**Note:** This entire project (code, icon, and documentation) was created with AI (Google Gemini). Please review and use at your own discretion.</sub>
