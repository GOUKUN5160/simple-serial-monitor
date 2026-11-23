use crate::types::ProcessOwner;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE, STATUS_INFO_LENGTH_MISMATCH,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileType, QueryDosDeviceW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TYPE_CHAR, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
const OBJECT_NAME_INFORMATION: u32 = 1;

#[repr(C)]
struct SystemHandleTableEntryInfoEx {
    object: *mut c_void,
    unique_process_id: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

#[repr(C)]
struct SystemHandleInformationExHeader {
    number_of_handles: usize,
    reserved: usize,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(
        system_information_class: u32,
        system_information: *mut c_void,
        system_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

    fn NtQueryObject(
        handle: HANDLE,
        object_information_class: u32,
        object_information: *mut c_void,
        object_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

/// Find the process currently holding a handle to the specified COM port (e.g. "COM3").
pub fn find_port_owner(port_name: &str) -> Option<ProcessOwner> {
    let target_nt_path = query_dos_device_nt_path(port_name)?;
    let target_lower = target_nt_path.to_ascii_lowercase();

    unsafe {
        // Open NUL briefly so we can identify the exact ObjectTypeIndex for File objects on this Windows build.
        let nul_wide: Vec<u16> = "\\\\.\\NUL\0".encode_utf16().collect();
        let probe_handle = CreateFileW(
            nul_wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        );
        if probe_handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let buf = query_extended_handles();
        let my_pid = GetCurrentProcessId() as usize;
        let probe_val = probe_handle as usize;

        let result = buf.and_then(|data| {
            if data.len() < std::mem::size_of::<SystemHandleInformationExHeader>() {
                return None;
            }
            let header = &*(data.as_ptr() as *const SystemHandleInformationExHeader);
            let entries_ptr = data
                .as_ptr()
                .add(std::mem::size_of::<SystemHandleInformationExHeader>())
                as *const SystemHandleTableEntryInfoEx;
            let max_entries = (data.len() - std::mem::size_of::<SystemHandleInformationExHeader>())
                / std::mem::size_of::<SystemHandleTableEntryInfoEx>();
            let count = header.number_of_handles.min(max_entries);
            let entries = std::slice::from_raw_parts(entries_ptr, count);

            // Identify File ObjectTypeIndex
            let file_type_idx = entries
                .iter()
                .find(|e| e.unique_process_id == my_pid && e.handle_value == probe_val)
                .map(|e| e.object_type_index)?;

            // Group File handles by remote PID
            let mut by_pid: HashMap<u32, Vec<usize>> = HashMap::new();
            for e in entries {
                if e.object_type_index == file_type_idx
                    && e.unique_process_id != my_pid
                    && e.unique_process_id > 4
                {
                    by_pid
                        .entry(e.unique_process_id as u32)
                        .or_default()
                        .push(e.handle_value);
                }
            }

            let cur_proc = GetCurrentProcess();
            for (pid, handles) in by_pid {
                let hproc = OpenProcess(PROCESS_DUP_HANDLE, 0, pid);
                if hproc.is_null() {
                    continue;
                }
                let mut matched = false;
                for hval in handles {
                    let mut dup: HANDLE = null_mut();
                    let ok = DuplicateHandle(
                        hproc,
                        hval as HANDLE,
                        cur_proc,
                        &mut dup,
                        0,
                        0,
                        0,
                    );
                    if ok == 0 || dup.is_null() {
                        continue;
                    }
                    // Fast filter: COM ports are always FILE_TYPE_CHAR (avoids pipes completely!)
                    let ftype = GetFileType(dup);
                    if ftype == FILE_TYPE_CHAR {
                        if let Some(obj_name) = query_object_name_safe(dup) {
                            let lower = obj_name.to_ascii_lowercase();
                            if lower == target_lower || lower.ends_with(&target_lower) {
                                matched = true;
                            }
                        }
                    }
                    CloseHandle(dup);
                    if matched {
                        break;
                    }
                }
                CloseHandle(hproc);
                if matched {
                    return Some(build_process_owner(pid));
                }
            }
            None
        });

        CloseHandle(probe_handle);
        result
    }
}

/// Terminate the process by PID and wait briefly for handle cleanup.
pub fn terminate_process(pid: u32) -> Result<(), String> {
    if pid <= 4 || pid == unsafe { GetCurrentProcessId() } {
        return Err("システムプロセスまたは自プロセスは終了できません".into());
    }
    unsafe {
        let hproc = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if hproc.is_null() {
            return Err(format!(
                "プロセス (PID {}) を開けませんでした (権限不足の可能性があります)",
                pid
            ));
        }
        let ok = TerminateProcess(hproc, 1);
        if ok == 0 {
            CloseHandle(hproc);
            return Err(format!("プロセス (PID {}) の終了に失敗しました", pid));
        }
        // Wait up to 1500ms for OS to release all handles including COM port
        WaitForSingleObject(hproc, 1500);
        CloseHandle(hproc);
        Ok(())
    }
}

fn query_dos_device_nt_path(port_name: &str) -> Option<String> {
    let clean = port_name.trim().trim_start_matches("\\\\.\\");
    let wide: Vec<u16> = clean.encode_utf16().chain(Some(0)).collect();
    let mut buf = [0u16; 512];
    let len = unsafe { QueryDosDeviceW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
    let s = String::from_utf16_lossy(&buf[..end]);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

unsafe fn query_extended_handles() -> Option<Vec<u8>> {
    let mut size: usize = 1024 * 1024; // 1 MB initial
    for _ in 0..8 {
        let mut buf = vec![0u8; size];
        let mut ret_len: u32 = 0;
        let status = NtQuerySystemInformation(
            SYSTEM_EXTENDED_HANDLE_INFORMATION,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut ret_len,
        );
        if status >= 0 {
            return Some(buf);
        }
        if status == STATUS_INFO_LENGTH_MISMATCH {
            size = (ret_len as usize).max(size * 2) + 65536;
            if size > 128 * 1024 * 1024 {
                return None;
            }
        } else {
            return None;
        }
    }
    None
}

fn query_object_name_safe(handle: HANDLE) -> Option<String> {
    // Querying FILE_TYPE_CHAR handles is normally instantaneous,
    // but run with a short guard thread only if needed? Actually spawning a thread per char handle
    // is very cheap since each process typically has only 0-3 FILE_TYPE_CHAR handles (console/COM).
    let raw = handle as usize;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let h = raw as HANDLE;
        let mut buf = vec![0u8; 2048];
        let mut ret_len = 0u32;
        let status = unsafe {
            NtQueryObject(
                h,
                OBJECT_NAME_INFORMATION,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut ret_len,
            )
        };
        if status >= 0 {
            let us = unsafe { &*(buf.as_ptr() as *const UnicodeString) };
            if us.length > 0 && !us.buffer.is_null() {
                let slice = unsafe {
                    std::slice::from_raw_parts(us.buffer, (us.length / 2) as usize)
                };
                let _ = tx.send(Some(String::from_utf16_lossy(slice)));
                return;
            }
        }
        let _ = tx.send(None);
    });
    rx.recv_timeout(Duration::from_millis(80)).ok().flatten()
}

fn build_process_owner(pid: u32) -> ProcessOwner {
    let mut exe_path = String::new();
    unsafe {
        let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !hproc.is_null() {
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(hproc, 0, buf.as_mut_ptr(), &mut len) != 0 {
                exe_path = String::from_utf16_lossy(&buf[..len as usize]);
            }
            CloseHandle(hproc);
        }
    }
    let name = exe_path
        .rsplit(['\\', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("PID {}", pid));

    ProcessOwner {
        pid,
        name,
        exe_path,
    }
}
