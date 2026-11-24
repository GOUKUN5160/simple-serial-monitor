use crate::types::PortInfo;

#[cfg(windows)]
mod platform {
    use super::{build_hardware_label, is_cdc_device, port_sort_key};
    use crate::types::PortInfo;
    use std::collections::BTreeMap;
    use std::ptr::{null, null_mut};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
        SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey, DIGCF_DEVICEINTERFACE,
        DIGCF_PRESENT, DICS_FLAG_GLOBAL, DIREG_DEV, HDEVINFO, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME,
        SPDRP_HARDWAREID, SPDRP_MFG, SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_SZ,
    };

    // GUID_DEVCLASS_PORTS = {4D36E978-E325-11CE-BFC1-08002BE10318}
    const GUID_DEVCLASS_PORTS: GUID = GUID {
        data1: 0x4D36E978,
        data2: 0xE325,
        data3: 0x11CE,
        data4: [0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18],
    };

    // GUID_DEVINTERFACE_COMPORT = {86E0D1E0-8089-11D0-9CE4-08003E301F73}
    const GUID_DEVINTERFACE_COMPORT: GUID = GUID {
        data1: 0x86E0D1E0,
        data2: 0x8089,
        data3: 0x11D0,
        data4: [0x9C, 0xE4, 0x08, 0x00, 0x3E, 0x30, 0x1F, 0x73],
    };

    pub fn enumerate_ports() -> Vec<PortInfo> {
        let mut map: BTreeMap<String, PortInfo> = BTreeMap::new();

        // 1. SetupAPI by GUID_DEVCLASS_PORTS (most USB-UART, Arduino, ESP32, Pico, Bluetooth SPP)
        enum_setupapi(&GUID_DEVCLASS_PORTS, DIGCF_PRESENT, &mut map);
        // 2. SetupAPI by GUID_DEVINTERFACE_COMPORT (additional virtual / multi-function CDC ports)
        enum_setupapi(
            &GUID_DEVINTERFACE_COMPORT,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            &mut map,
        );
        // 3. Registry fallback HKLM\HARDWARE\DEVICEMAP\SERIALCOMM (catches com0com & custom virtual ports)
        enum_registry_serialcomm(&mut map);

        let mut ports: Vec<PortInfo> = map.into_values().collect();
        ports.sort_by_key(|p| port_sort_key(&p.name));
        ports
    }

    fn enum_setupapi(guid: &GUID, flags: u32, map: &mut BTreeMap<String, PortInfo>) {
        unsafe {
            let hdev = SetupDiGetClassDevsW(guid, null(), null_mut(), flags);
            if hdev == INVALID_HANDLE_VALUE as HDEVINFO || hdev == 0 {
                return;
            }

            let mut index = 0u32;
            loop {
                let mut dev_info: SP_DEVINFO_DATA = std::mem::zeroed();
                dev_info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(hdev, index, &mut dev_info) == 0 {
                    break;
                }
                index += 1;

                let port_name = match read_port_name(hdev, &dev_info) {
                    Some(p) if p.to_ascii_uppercase().starts_with("COM") => p.to_ascii_uppercase(),
                    _ => continue,
                };

                let friendly = read_dev_prop_string(hdev, &dev_info, SPDRP_FRIENDLYNAME)
                    .or_else(|| read_dev_prop_string(hdev, &dev_info, SPDRP_DEVICEDESC))
                    .unwrap_or_default();
                let manufacturer =
                    read_dev_prop_string(hdev, &dev_info, SPDRP_MFG).unwrap_or_default();
                let hwid =
                    read_dev_prop_string(hdev, &dev_info, SPDRP_HARDWAREID).unwrap_or_default();

                let (vid, pid) = parse_vid_pid(&hwid);
                let is_usb_cdc = is_cdc_device(vid, pid, &friendly, &hwid);
                let hardware_label =
                    build_hardware_label(&port_name, &friendly, &manufacturer, vid, pid);

                map.entry(port_name.clone()).or_insert(PortInfo {
                    name: port_name,
                    friendly_name: friendly,
                    hardware_label,
                    manufacturer,
                    vid,
                    pid,
                    is_usb_cdc,
                });
            }

            SetupDiDestroyDeviceInfoList(hdev);
        }
    }

    unsafe fn read_port_name(hdev: HDEVINFO, dev_info: &SP_DEVINFO_DATA) -> Option<String> {
        let hkey: HKEY =
            SetupDiOpenDevRegKey(hdev, dev_info, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ);
        if hkey.is_null() || hkey == INVALID_HANDLE_VALUE {
            return None;
        }
        let key_name: Vec<u16> = "PortName\0".encode_utf16().collect();
        let mut buf = [0u16; 64];
        let mut buf_bytes = (buf.len() * 2) as u32;
        let mut reg_type = 0u32;
        let status = RegQueryValueExW(
            hkey,
            key_name.as_ptr(),
            null(),
            &mut reg_type,
            buf.as_mut_ptr() as *mut u8,
            &mut buf_bytes,
        );
        RegCloseKey(hkey);
        if status == ERROR_SUCCESS && reg_type == REG_SZ {
            let len = (buf_bytes as usize / 2).min(buf.len());
            let s = utf16_slice_to_string(&buf[..len]);
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    unsafe fn read_dev_prop_string(
        hdev: HDEVINFO,
        dev_info: &SP_DEVINFO_DATA,
        prop: u32,
    ) -> Option<String> {
        let mut buf = [0u16; 512];
        let mut req_size = 0u32;
        let ok = SetupDiGetDeviceRegistryPropertyW(
            hdev,
            dev_info,
            prop,
            null_mut(),
            buf.as_mut_ptr() as *mut u8,
            (buf.len() * 2) as u32,
            &mut req_size,
        );
        if ok != 0 {
            let s = utf16_slice_to_string(&buf);
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    fn enum_registry_serialcomm(map: &mut BTreeMap<String, PortInfo>) {
        unsafe {
            let subkey: Vec<u16> = "HARDWARE\\DEVICEMAP\\SERIALCOMM\0".encode_utf16().collect();
            let mut hkey: HKEY = null_mut();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, subkey.as_ptr(), 0, KEY_READ, &mut hkey)
                != ERROR_SUCCESS
            {
                return;
            }
            let mut index = 0u32;
            loop {
                let mut name_buf = [0u16; 256];
                let mut name_len = name_buf.len() as u32;
                let mut data_buf = [0u16; 128];
                let mut data_bytes = (data_buf.len() * 2) as u32;
                let mut reg_type = 0u32;
                let st = RegEnumValueW(
                    hkey,
                    index,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    null(),
                    &mut reg_type,
                    data_buf.as_mut_ptr() as *mut u8,
                    &mut data_bytes,
                );
                if st != ERROR_SUCCESS {
                    break;
                }
                index += 1;
                if reg_type == REG_SZ {
                    let port = utf16_slice_to_string(
                        &data_buf[..(data_bytes as usize / 2).min(data_buf.len())],
                    )
                    .to_ascii_uppercase();
                    if port.starts_with("COM") {
                        let dev_path = utf16_slice_to_string(
                            &name_buf[..(name_len as usize).min(name_buf.len())],
                        );
                        map.entry(port.clone()).or_insert_with(|| PortInfo {
                            name: port,
                            friendly_name: dev_path.clone(),
                            hardware_label: if dev_path.contains("VCP") || dev_path.contains("CNC") {
                                "Virtual Serial Port".to_string()
                            } else {
                                dev_path
                            },
                            manufacturer: String::new(),
                            vid: None,
                            pid: None,
                            is_usb_cdc: false,
                        });
                    }
                }
            }
            RegCloseKey(hkey);
        }
    }

    fn utf16_slice_to_string(slice: &[u16]) -> String {
        let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
        String::from_utf16_lossy(&slice[..end]).trim().to_string()
    }

    fn parse_vid_pid(hwid: &str) -> (Option<u16>, Option<u16>) {
        let upper = hwid.to_ascii_uppercase();
        let vid = extract_hex4(&upper, "VID_");
        let pid = extract_hex4(&upper, "PID_");
        (vid, pid)
    }

    fn extract_hex4(s: &str, prefix: &str) -> Option<u16> {
        let idx = s.find(prefix)?;
        let rest = &s[idx + prefix.len()..];
        if rest.len() >= 4 {
            u16::from_str_radix(&rest[..4], 16).ok()
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{build_hardware_label, is_cdc_device, port_sort_key};
    use crate::types::PortInfo;
    use std::collections::BTreeMap;
    use std::ffi::{c_char, c_void, CStr};

    type IoObjectT = u32;
    type IoIteratorT = u32;
    type IoRegistryEntryT = u32;
    type KernReturnT = i32;
    type CfTypeRef = *const c_void;
    type CfStringRef = *const c_void;
    type CfDictionaryRef = *const c_void;
    type CfAllocatorRef = *const c_void;
    type CfTypeId = usize;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
    const K_IO_SERVICE_PLANE: &[u8] = b"IOService\0";

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CfDictionaryRef;
        fn IOServiceGetMatchingServices(
            main_port: u32,
            matching: CfDictionaryRef,
            existing: *mut IoIteratorT,
        ) -> KernReturnT;
        fn IOIteratorNext(iterator: IoIteratorT) -> IoObjectT;
        fn IOObjectRelease(object: IoObjectT) -> KernReturnT;
        fn IORegistryEntryCreateCFProperty(
            entry: IoRegistryEntryT,
            key: CfStringRef,
            allocator: CfAllocatorRef,
            options: u32,
        ) -> CfTypeRef;
        fn IORegistryEntryGetParentEntry(
            entry: IoRegistryEntryT,
            plane: *const c_char,
            parent: *mut IoRegistryEntryT,
        ) -> KernReturnT;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: CfAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CfStringRef;
        fn CFStringGetCString(
            the_string: CfStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFStringGetTypeID() -> CfTypeId;
        fn CFNumberGetTypeID() -> CfTypeId;
        fn CFNumberGetValue(number: CfTypeRef, the_type: i32, value_ptr: *mut c_void) -> u8;
        fn CFGetTypeID(cf: CfTypeRef) -> CfTypeId;
        fn CFRelease(cf: CfTypeRef);
    }

    unsafe fn cf_str(s: &[u8]) -> CfStringRef {
        CFStringCreateWithCString(
            std::ptr::null(),
            s.as_ptr() as *const c_char,
            K_CF_STRING_ENCODING_UTF8,
        )
    }

    unsafe fn read_entry_string(entry: IoRegistryEntryT, key_cstr: &[u8]) -> Option<String> {
        let key = cf_str(key_cstr);
        if key.is_null() {
            return None;
        }
        let val = IORegistryEntryCreateCFProperty(entry, key, std::ptr::null(), 0);
        CFRelease(key);
        if val.is_null() {
            return None;
        }
        let mut out = None;
        if CFGetTypeID(val) == CFStringGetTypeID() {
            let mut buf = [0i8; 512];
            if CFStringGetCString(val, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8) != 0 {
                if let Ok(s) = CStr::from_ptr(buf.as_ptr()).to_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        out = Some(trimmed.to_string());
                    }
                }
            }
        }
        CFRelease(val);
        out
    }

    unsafe fn read_entry_u16(entry: IoRegistryEntryT, key_cstr: &[u8]) -> Option<u16> {
        let key = cf_str(key_cstr);
        if key.is_null() {
            return None;
        }
        let val = IORegistryEntryCreateCFProperty(entry, key, std::ptr::null(), 0);
        CFRelease(key);
        if val.is_null() {
            return None;
        }
        let mut out = None;
        if CFGetTypeID(val) == CFNumberGetTypeID() {
            let mut num: i32 = 0;
            if CFNumberGetValue(val, K_CF_NUMBER_SINT32_TYPE, &mut num as *mut i32 as *mut c_void) != 0
                && (0..=0xFFFF).contains(&num)
            {
                out = Some(num as u16);
            }
        }
        CFRelease(val);
        out
    }

    pub fn enumerate_ports() -> Vec<PortInfo> {
        let mut map: BTreeMap<String, PortInfo> = BTreeMap::new();

        unsafe {
            let matching = IOServiceMatching(b"IOSerialBSDClient\0".as_ptr() as *const c_char);
            if !matching.is_null() {
                let mut iter: IoIteratorT = 0;
                if IOServiceGetMatchingServices(0, matching, &mut iter) == 0 {
                    loop {
                        let service = IOIteratorNext(iter);
                        if service == 0 {
                            break;
                        }
                        if let Some(callout) = read_entry_string(service, b"IOCalloutDevice\0") {
                            let tty_base = read_entry_string(service, b"IOTTYBaseName\0").unwrap_or_default();
                            let mut vid = None;
                            let mut pid = None;
                            let mut product = String::new();
                            let mut vendor = String::new();

                            let mut cur = service;
                            let mut owned_cur = false;
                            for _ in 0..12 {
                                if vid.is_none() {
                                    vid = read_entry_u16(cur, b"idVendor\0");
                                }
                                if pid.is_none() {
                                    pid = read_entry_u16(cur, b"idProduct\0");
                                }
                                if product.is_empty() {
                                    if let Some(p) = read_entry_string(cur, b"USB Product Name\0")
                                        .or_else(|| read_entry_string(cur, b"kUSBProductString\0"))
                                        .or_else(|| read_entry_string(cur, b"Product Name\0"))
                                    {
                                        product = p;
                                    }
                                }
                                if vendor.is_empty() {
                                    if let Some(v) = read_entry_string(cur, b"USB Vendor Name\0")
                                        .or_else(|| read_entry_string(cur, b"kUSBVendorString\0"))
                                        .or_else(|| read_entry_string(cur, b"Vendor Name\0"))
                                    {
                                        vendor = v;
                                    }
                                }
                                if vid.is_some() && pid.is_some() && !product.is_empty() && !vendor.is_empty() {
                                    break;
                                }
                                let mut parent: IoRegistryEntryT = 0;
                                if IORegistryEntryGetParentEntry(
                                    cur,
                                    K_IO_SERVICE_PLANE.as_ptr() as *const c_char,
                                    &mut parent,
                                ) != 0
                                    || parent == 0
                                {
                                    break;
                                }
                                if owned_cur {
                                    IOObjectRelease(cur);
                                }
                                cur = parent;
                                owned_cur = true;
                            }
                            if owned_cur {
                                IOObjectRelease(cur);
                            }

                            let friendly = if !product.is_empty() {
                                product.clone()
                            } else if !tty_base.is_empty() {
                                tty_base.clone()
                            } else {
                                callout
                                    .strip_prefix("/dev/cu.")
                                    .unwrap_or(&callout)
                                    .to_string()
                            };
                            let is_usb_cdc = is_cdc_device(vid, pid, &friendly, &callout);
                            let hardware_label =
                                build_hardware_label(&callout, &friendly, &vendor, vid, pid);

                            map.entry(callout.clone()).or_insert(PortInfo {
                                name: callout,
                                friendly_name: friendly,
                                hardware_label,
                                manufacturer: vendor,
                                vid,
                                pid,
                                is_usb_cdc,
                            });
                        }
                        IOObjectRelease(service);
                    }
                    IOObjectRelease(iter);
                }
            }
        }

        // Fallback: scan /dev/cu.* for virtual serial ports (e.g. socat, pseudo-terminals)
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                if let Ok(fname) = entry.file_name().into_string() {
                    if fname.starts_with("cu.") {
                        let full = format!("/dev/{}", fname);
                        map.entry(full.clone()).or_insert_with(|| {
                            let short = fname.trim_start_matches("cu.").to_string();
                            let is_usb_cdc = is_cdc_device(None, None, &short, &full);
                            PortInfo {
                                name: full,
                                friendly_name: short.clone(),
                                hardware_label: short,
                                manufacturer: String::new(),
                                vid: None,
                                pid: None,
                                is_usb_cdc,
                            }
                        });
                    }
                }
            }
        }

        let mut ports: Vec<PortInfo> = map.into_values().collect();
        ports.sort_by_key(|p| port_sort_key(&p.name));
        ports
    }
}

pub fn enumerate_ports() -> Vec<PortInfo> {
    platform::enumerate_ports()
}

pub fn port_sort_key(name: &str) -> (u32, String) {
    let upper = name.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("COM") {
        if let Ok(n) = rest.parse::<u32>() {
            return (n, upper);
        }
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("usbmodem")
        || lower.contains("usbserial")
        || lower.contains("slab_usbtouart")
        || lower.contains("wchusbserial")
    {
        return (0, lower);
    }
    if lower.contains("bluetooth") || lower.contains("debug-console") || lower.contains("wlan") {
        return (2, lower);
    }
    (1, lower)
}

fn is_cdc_device(vid: Option<u16>, pid: Option<u16>, friendly: &str, hwid: &str) -> bool {
    // Native USB CDC boards often require DTR=ON to send output (e.g. Raspberry Pi Pico, ESP32-S3 USB-JTAG, Arduino Leonardo/R4)
    if let Some(v) = vid {
        if matches!(v, 0x2E8A | 0x2341 | 0x2A03 | 0x239A | 0x1B4F | 0x16C0) {
            return true;
        }
        if v == 0x303A && matches!(pid, Some(0x1001 | 0x0002 | 0x0009)) {
            return true;
        }
    }
    let fl = friendly.to_ascii_lowercase();
    let hl = hwid.to_ascii_lowercase();
    fl.contains("usb serial device")
        || hl.contains("usbmodem")
        || hl.contains("usb\\vid_") && fl.contains("シリアル デバイス")
}

fn build_hardware_label(
    port: &str,
    friendly: &str,
    manufacturer: &str,
    vid: Option<u16>,
    pid: Option<u16>,
) -> String {
    // Check well-known embedded hardware VID:PID first for super informative names
    if let (Some(v), Some(p)) = (vid, pid) {
        if let Some(known) = known_hardware_name(v, p) {
            return known.to_string();
        }
    }
    // Strip trailing " (COMx)" from friendly name to keep labels compact
    let suffix = format!(" ({})", port);
    let mut cleaned = friendly.trim().to_string();
    if cleaned.to_ascii_uppercase().ends_with(&suffix) {
        let new_len = cleaned.len().saturating_sub(suffix.len());
        cleaned.truncate(new_len);
    }
    if !cleaned.is_empty() {
        return cleaned;
    }
    manufacturer.trim().to_string()
}

fn known_hardware_name(vid: u16, pid: u16) -> Option<&'static str> {
    match (vid, pid) {
        // Espressif
        (0x303A, 0x1001) => Some("ESP32 USB JTAG/Serial"),
        (0x303A, 0x0002) => Some("ESP32-S2 USB CDC"),
        (0x303A, 0x0009) => Some("ESP32-S3 USB CDC"),
        (0x303A, _) => Some("Espressif ESP32"),
        // Raspberry Pi
        (0x2E8A, 0x0005) => Some("Raspberry Pi Pico (MicroPython)"),
        (0x2E8A, 0x000A) => Some("Raspberry Pi Pico (SDK/CDC)"),
        (0x2E8A, 0x000C) => Some("Raspberry Pi Debug Probe"),
        (0x2E8A, 0x000F) => Some("Raspberry Pi Pico 2 (RP2350)"),
        (0x2E8A, _) => Some("Raspberry Pi RP2xxx"),
        // Arduino
        (0x2341, 0x0043) | (0x2A03, 0x0043) => Some("Arduino Uno"),
        (0x2341, 0x0042) | (0x2A03, 0x0042) => Some("Arduino Mega 2560"),
        (0x2341, 0x8036) | (0x2341, 0x0036) => Some("Arduino Leonardo"),
        (0x2341, 0x8037) | (0x2341, 0x0037) => Some("Arduino Micro"),
        (0x2341, 0x0069) | (0x2341, 0x1002) => Some("Arduino Uno R4"),
        (0x2341, 0x0058) => Some("Arduino Nano Every"),
        (0x2341, _) | (0x2A03, _) => Some("Arduino Board"),
        // Common USB-UART Bridges
        (0x10C4, 0xEA60) => Some("CP210x USB-UART Bridge"),
        (0x10C4, 0xEA70) => Some("CP2105 Dual USB-UART"),
        (0x10C4, _) => Some("Silicon Labs UART Bridge"),
        (0x1A86, 0x7523) => Some("CH340 USB-Serial"),
        (0x1A86, 0x5523) => Some("CH341 USB-Serial"),
        (0x1A86, 0x55D4) => Some("CH9102 USB-Serial"),
        (0x1A86, 0x55D3) => Some("CH343 USB-Serial"),
        (0x0403, 0x6001) => Some("FTDI FT232R USB-UART"),
        (0x0403, 0x6010) => Some("FTDI FT2232 Dual UART"),
        (0x0403, 0x6011) => Some("FTDI FT4232 Quad UART"),
        (0x0403, 0x6015) => Some("FTDI FT231X USB-UART"),
        (0x0403, _) => Some("FTDI USB-UART"),
        (0x067B, 0x2303) | (0x067B, 0x23A3) => Some("Prolific PL2303 USB-Serial"),
        // Debug probes & other MCUs
        (0x0483, 0x374B) | (0x0483, 0x374E) | (0x0483, 0x374F) => Some("STM32 ST-LINK VCP"),
        (0x0483, 0x5740) => Some("STM32 Virtual COM Port"),
        (0x1366, _) => Some("SEGGER J-Link CDC"),
        (0x16C0, 0x0483) => Some("Teensy USB Serial"),
        (0x0D28, 0x0204) => Some("BBC micro:bit / DAPLink"),
        (0x239A, _) => Some("Adafruit Board"),
        (0x1B4F, _) => Some("SparkFun Board"),
        _ => None,
    }
}
