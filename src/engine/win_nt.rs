//! Low-overhead OS telemetry wrappers.
//! - Windows: unsafe Win32 / NT native APIs (`NtQuerySystemInformation`, PEB reads, named pipes).
//! - macOS / Unix: `ps`, `lsof`, `sysctl`, `vm_stat`, and Unix domain socket queries.

use crate::model::ProcessKey;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Raw per-process record parsed from OS enumeration.
#[derive(Debug, Clone)]
pub struct RawProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub session_id: u32,
    pub create_time_100ns: i64,
    pub cpu_time_100ns: i64,
    pub private_ram_bytes: u64,
    pub image_name: String,
}

/// Result of inspecting a newly seen process once (image path, CWD, cmdline, and env hints).
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct PebInspection {
    pub image_path: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub cmdline: Option<String>,
    pub peb_readable: bool,
    pub virtual_env: Option<String>,
    pub compose_project_env: Option<String>,
    /// Safe developer-oriented env vars captured in memory for faithful restarts (never logged or serialized).
    pub dev_env: Vec<(String, String)>,
}

/// Summary of a running Docker container parsed from Engine API `/containers/json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainerSummary {
    pub name: String,
    pub compose_project: Option<String>,
    pub ports: Vec<u16>,
    pub uptime_secs: u64,
}

#[derive(Deserialize)]
struct DockerApiContainer {
    #[serde(rename = "Id")]
    id: Option<String>,
    #[serde(rename = "Names", default)]
    names: Vec<String>,
    #[serde(rename = "Labels", default)]
    labels: HashMap<String, String>,
    #[serde(rename = "Ports", default)]
    ports: Vec<DockerApiPort>,
    #[serde(rename = "Created")]
    created: Option<i64>,
    #[serde(rename = "Status")]
    status: Option<String>,
}

#[derive(Deserialize)]
struct DockerApiPort {
    #[serde(rename = "PublicPort")]
    public_port: Option<u16>,
}

pub fn parse_docker_status_uptime_secs(status: &str) -> Option<u64> {
    let low = status.trim().to_ascii_lowercase();
    let rest = low.strip_prefix("up ")?.trim();
    if rest.starts_with("less than a second") || rest.starts_with("about a second") {
        return Some(1);
    }
    if rest.starts_with("about a minute") {
        return Some(60);
    }
    if rest.starts_with("about an hour") {
        return Some(3600);
    }
    let mut parts = rest.split_whitespace();
    let num: u64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?;
    let mult = if unit.starts_with("sec") {
        1
    } else if unit.starts_with("min") {
        60
    } else if unit.starts_with("hour") {
        3600
    } else if unit.starts_with("day") {
        86_400
    } else if unit.starts_with("week") {
        7 * 86_400
    } else if unit.starts_with("month") {
        30 * 86_400
    } else {
        return None;
    };
    Some(num.saturating_mul(mult))
}

/// Parses Docker Engine `/containers/json` response body using `serde_json`.
pub fn parse_docker_containers_json(body: &str) -> Vec<DockerContainerSummary> {
    let Ok(items) = serde_json::from_str::<Vec<DockerApiContainer>>(body) else {
        return Vec::new();
    };
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    items
        .into_iter()
        .filter_map(|c| {
            let id = c.id?;
            let compose_project = c
                .labels
                .get("com.docker.compose.project")
                .filter(|s| !s.is_empty())
                .cloned();
            let name = c
                .names
                .first()
                .map(|s| s.trim_start_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.chars().take(12).collect());

            let mut ports = Vec::new();
            for p in c.ports {
                if let Some(pub_p) = p.public_port {
                    if pub_p > 0 && !ports.contains(&pub_p) {
                        ports.push(pub_p);
                    }
                }
            }
            let uptime_secs = c
                .status
                .as_deref()
                .and_then(parse_docker_status_uptime_secs)
                .or_else(|| {
                    c.created
                        .filter(|&ts| ts > 0 && (ts as u64) <= now_secs)
                        .map(|ts| now_secs.saturating_sub(ts as u64))
                })
                .unwrap_or(0);
            Some(DockerContainerSummary {
                name,
                compose_project,
                ports,
                uptime_secs,
            })
        })
        .collect()
}

/// Decodes Docker Engine `/containers/{id}/logs` HTTP response body (supporting both 8-byte multiplexed frames and plain TTY text).
pub fn parse_docker_logs_payload(raw_http: &[u8], max_lines: usize) -> Vec<String> {
    let Some(sep_pos) = raw_http.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Vec::new();
    };
    let head = std::str::from_utf8(&raw_http[..sep_pos]).unwrap_or("");
    let status_line = head.lines().next().unwrap_or("");
    if !(status_line.starts_with("HTTP/1.") && status_line.contains(" 200 ")) {
        return Vec::new();
    }
    let body = &raw_http[sep_pos + 4..];
    let mut text = String::new();
    if body.len() >= 8 && matches!(body[0], 0..=2) && body[1..4] == [0, 0, 0] {
        let mut cursor = 0usize;
        while cursor + 8 <= body.len() {
            let frame_len = u32::from_be_bytes([
                body[cursor + 4],
                body[cursor + 5],
                body[cursor + 6],
                body[cursor + 7],
            ]) as usize;
            cursor += 8;
            let end = (cursor + frame_len).min(body.len());
            text.push_str(&String::from_utf8_lossy(&body[cursor..end]));
            cursor = end;
        }
    } else {
        text.push_str(&String::from_utf8_lossy(body));
    }
    let all_lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let skip = all_lines.len().saturating_sub(max_lines);
    all_lines.into_iter().skip(skip).collect()
}

fn parse_http_200_body(raw_http: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw_http).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status_line = head.lines().next()?;
    if !(status_line.starts_with("HTTP/1.") && status_line.contains(" 200 ")) {
        return None;
    }
    let is_chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().contains("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    if is_chunked {
        decode_http_chunked(body)
    } else {
        Some(body.to_string())
    }
}

fn decode_http_chunked(mut rest: &str) -> Option<String> {
    let mut out = String::with_capacity(rest.len());
    loop {
        let (size_line, after_size) = rest.split_once("\r\n")?;
        let hex = size_line.split(';').next().unwrap_or(size_line).trim();
        let len = usize::from_str_radix(hex, 16).ok()?;
        if len == 0 {
            break;
        }
        if after_size.len() < len {
            return None;
        }
        out.push_str(&after_size[..len]);
        rest = after_size[len..].strip_prefix("\r\n")?;
    }
    Some(out)
}

/// Parses a single `ps -axo pid=,ppid=,rss=,etime=,time=,comm=` output line cleanly,
/// preserving spaces in `comm` without mis-matching when `etime == cputime`.
#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn parse_ps_line(line: &str) -> Option<(u32, u32, u64, i64, i64, &str)> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut fields: [&str; 5] = [""; 5];
    for slot in &mut fields {
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < len && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return None;
        }
        *slot = &line[start..i];
    }
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let pid = fields[0].parse::<u32>().ok()?;
    let ppid = fields[1].parse::<u32>().unwrap_or(0);
    let rss_kb = fields[2].parse::<u64>().unwrap_or(0);
    let etime_100ns = parse_ps_duration_100ns(fields[3]).unwrap_or(0);
    let cpu_100ns = parse_ps_duration_100ns(fields[4]).unwrap_or(0);
    Some((pid, ppid, rss_kb, etime_100ns, cpu_100ns, line[i..].trim()))
}

/// Parses elapsed/CPU duration strings formatted as `[[dd-]hh:]mm:ss[.frac]` into seconds + fractional 100ns ticks.
#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn parse_ps_duration_100ns(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (days, rest) = if let Some((d, r)) = trimmed.split_once('-') {
        (d.trim().parse::<i64>().ok()?, r)
    } else {
        (0, trimmed)
    };
    let (hms, frac_100ns) = if let Some((main_part, frac_part)) = rest.split_once('.') {
        let mut ticks = 0i64;
        let mut scale = 1_000_000i64;
        for b in frac_part.bytes().take(7) {
            if !b.is_ascii_digit() {
                break;
            }
            ticks += ((b - b'0') as i64) * scale;
            scale /= 10;
        }
        (main_part, ticks)
    } else {
        (rest, 0)
    };

    let mut parts = hms.split(':');
    let a = parts.next()?.trim().parse::<i64>().ok()?;
    let b = parts.next()?.trim().parse::<i64>().ok()?;
    let total_secs = match parts.next() {
        Some(c_str) => {
            if parts.next().is_some() {
                return None;
            }
            let c = c_str.trim().parse::<i64>().ok()?;
            days * 86_400 + a * 3_600 + b * 60 + c
        }
        None => days * 86_400 + a * 60 + b,
    };
    Some(total_secs.saturating_mul(10_000_000).saturating_add(frac_100ns))
}

/// Parses `vm_stat` text output on macOS into `(used_bytes, free_plus_inactive_bytes)`.
#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn parse_vm_stat_used_bytes(text: &str) -> Option<u64> {
    let mut page_size = 4096u64;
    let mut active = 0u64;
    let mut wired = 0u64;
    let mut compressed = 0u64;
    let mut matched = false;

    for line in text.lines() {
        if let Some(pos) = line.find("page size of ") {
            let after = &line[pos + "page size of ".len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(sz) = digits.parse::<u64>() {
                if sz >= 4096 {
                    page_size = sz;
                }
            }
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let pages = val
            .trim()
            .trim_end_matches('.')
            .parse::<u64>()
            .unwrap_or(0);
        if key.contains("Pages active") {
            active = pages;
            matched = true;
        } else if key.contains("Pages wired down") {
            wired = pages;
            matched = true;
        } else if key.contains("Pages occupied by compressor") {
            compressed = pages;
            matched = true;
        }
    }

    if matched {
        Some(active.saturating_add(wired).saturating_add(compressed).saturating_mul(page_size))
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
mod platform_imp {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, REG_SZ, REG_VALUE_TYPE,
    };
    use windows::Win32::System::SystemInformation::{
        GetSystemInfo, GetSystemTimeAsFileTime, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, GetExitCodeProcess, GetProcessTimes, GetSystemTimes,
        IsProcessCritical, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
        PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_READ,
    };

    const SYSTEM_PROCESS_INFORMATION: u32 = 5;
    const PROCESS_BASIC_INFORMATION_CLASS: u32 = 0;
    const PROCESS_WOW64_INFORMATION_CLASS: u32 = 26;

    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004_u32 as i32;
    const STATUS_BUFFER_TOO_SMALL: i32 = 0xC0000023_u32 as i32;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(
            system_information_class: u32,
            system_information: *mut c_void,
            system_information_length: u32,
            return_length: *mut u32,
        ) -> i32;

        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UnicodeString64 {
        length: u16,
        maximum_length: u16,
        _pad: u32,
        buffer: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UnicodeString32 {
        length: u16,
        maximum_length: u16,
        buffer: u32,
    }

    #[cfg(target_pointer_width = "64")]
    #[repr(C)]
    struct SystemProcessInformation {
        next_entry_offset: u32,
        number_of_threads: u32,
        working_set_private_size: i64,
        hard_fault_count: u32,
        number_of_threads_high_watermark: u32,
        cycle_time: u64,
        create_time: i64,
        user_time: i64,
        kernel_time: i64,
        image_name: UnicodeString64,
        base_priority: i32,
        _pad_priority: u32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
        handle_count: u32,
        session_id: u32,
        unique_process_key: usize,
        peak_virtual_size: usize,
        virtual_size: usize,
        page_fault_count: u32,
        _pad_fault: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_page_count: usize,
        read_operation_count: i64,
        write_operation_count: i64,
        other_operation_count: i64,
        read_transfer_count: i64,
        write_transfer_count: i64,
        other_transfer_count: i64,
    }

    #[repr(C)]
    struct ProcessBasicInformation64 {
        exit_status: i32,
        _pad0: u32,
        peb_base_address: u64,
        affinity_mask: u64,
        base_priority: i32,
        _pad1: u32,
        unique_process_id: u64,
        inherited_from_unique_process_id: u64,
    }

    /// Owned Win32 `HANDLE` wrapper that closes automatically on drop.
    pub struct OwnedHandle(pub HANDLE);

    impl OwnedHandle {
        #[inline]
        pub fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    /// Returns the number of logical CPU cores on this machine (minimum 1).
    pub fn logical_core_count() -> u32 {
        // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
        unsafe {
            let mut info: SYSTEM_INFO = zeroed();
            GetSystemInfo(&mut info);
            info.dwNumberOfProcessors.max(1)
        }
    }

    /// Queries system-wide `(idle_100ns, kernel_100ns, user_100ns)`.
    /// Note: on Windows, `kernel_100ns` includes `idle_100ns`.
    pub fn query_system_times_100ns() -> Option<(u64, u64, u64)> {
        // SAFETY: Win32 time struct pointer is valid and aligned for the call.
        unsafe {
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                let to_u64 = |ft: FILETIME| ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
                Some((to_u64(idle), to_u64(kernel), to_u64(user)))
            } else {
                None
            }
        }
    }

    /// Queries physical memory `(used_bytes, total_bytes)`.
    pub fn query_system_memory() -> (u64, u64) {
        // SAFETY: Initialized MEMORYSTATUSEX struct with valid dwLength.
        unsafe {
            let mut mem = MEMORYSTATUSEX {
                dwLength: size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if GlobalMemoryStatusEx(&mut mem).is_ok() {
                let total = mem.ullTotalPhys;
                let used = total.saturating_sub(mem.ullAvailPhys);
                (used, total)
            } else {
                (0, 0)
            }
        }
    }

    /// Returns the current Windows file time in 100ns intervals since 1601-01-01 (matches `CreateTime`).
    pub fn current_system_time_100ns() -> i64 {
        // SAFETY: Win32 time struct pointer is valid and aligned for the call.
        let ft = unsafe { GetSystemTimeAsFileTime() };
        (((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)) as i64
    }

    /// Enumerates all OS processes into `out` reusing 8-byte aligned `scratch_buf`.
    #[cfg(target_pointer_width = "64")]
    pub fn enumerate_processes(scratch_buf: &mut Vec<u64>, out: &mut Vec<RawProcessEntry>) {
        out.clear();
        const INIT_U64S: usize = (768 * 1024) / 8;
        if scratch_buf.len() < INIT_U64S {
            scratch_buf.resize(INIT_U64S, 0);
        }

        let mut ret_len: u32 = 0;
        loop {
            let byte_len = (scratch_buf.len() * size_of::<u64>()) as u32;
            // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    scratch_buf.as_mut_ptr() as *mut c_void,
                    byte_len,
                    &mut ret_len,
                )
            };

            if status >= 0 {
                break;
            }
            if status == STATUS_INFO_LENGTH_MISMATCH || status == STATUS_BUFFER_TOO_SMALL {
                let needed_bytes = (ret_len as usize).max(byte_len as usize) + 64 * 1024;
                scratch_buf.resize(needed_bytes.div_ceil(size_of::<u64>()), 0);
                continue;
            }
            return;
        }

        let buf_bytes = scratch_buf.len() * size_of::<u64>();
        let valid_len = if (ret_len as usize) > 0 && (ret_len as usize) <= buf_bytes {
            ret_len as usize
        } else {
            buf_bytes
        };

        let base_ptr = scratch_buf.as_ptr() as *const u8;
        let base_addr = base_ptr as usize;
        let mut offset: usize = 0;

        while offset + size_of::<SystemProcessInformation>() <= valid_len {
            // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
            let entry = unsafe {
                std::ptr::read_unaligned(base_ptr.add(offset) as *const SystemProcessInformation)
            };
            let pid = entry.unique_process_id as u32;
            let parent_pid = entry.inherited_from_unique_process_id as u32;

            let image_name = if pid == 0 {
                "System Idle Process".to_string()
            } else {
                read_inline_unicode(&entry.image_name, base_addr, valid_len)
                    .unwrap_or_else(|| format!("pid-{pid}"))
            };

            let cpu_time_100ns = entry.kernel_time.saturating_add(entry.user_time);
            let private_ram_bytes = entry.working_set_private_size.max(0) as u64;

            out.push(RawProcessEntry {
                pid,
                parent_pid,
                session_id: entry.session_id,
                create_time_100ns: entry.create_time,
                cpu_time_100ns,
                private_ram_bytes,
                image_name,
            });

            if entry.next_entry_offset == 0 {
                break;
            }
            let next = offset.saturating_add(entry.next_entry_offset as usize);
            if next <= offset {
                break;
            }
            offset = next;
        }
    }

    #[cfg(target_pointer_width = "64")]
    fn read_inline_unicode(us: &UnicodeString64, base_addr: usize, valid_len: usize) -> Option<String> {
        let byte_len = us.length as usize;
        if byte_len == 0 || us.buffer == 0 || !byte_len.is_multiple_of(2) {
            return None;
        }
        let buf_addr = us.buffer as usize;
        let end_addr = base_addr.checked_add(valid_len)?;
        let str_end = buf_addr.checked_add(byte_len)?;
        if buf_addr < base_addr || str_end > end_addr {
            return None;
        }
        // SAFETY: Table pointer and dwNumEntries bounds were populated by the preceding Win32 IP Helper query.
        let bytes = unsafe { std::slice::from_raw_parts(buf_addr as *const u8, byte_len) };
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_ne_bytes(*c))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// Opens a process with `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ` and reads PEB fields.
    pub fn inspect_process_peb(pid: u32) -> PebInspection {
        let mut out = PebInspection::default();
        if pid <= 4 {
            return out;
        }

        let full_rights = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;
        let handle = match open_process_owned(full_rights, pid) {
            Some(h) => h,
            None => {
                if let Some(h_lim) = open_process_owned(PROCESS_QUERY_LIMITED_INFORMATION, pid) {
                    out.image_path = query_full_image_name(h_lim.raw());
                }
                return out;
            }
        };

        out.image_path = query_full_image_name(handle.raw());

        let mut wow64_peb: usize = 0;
        let mut ret_len: u32 = 0;
        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        let wow_status = unsafe {
            NtQueryInformationProcess(
                handle.raw(),
                PROCESS_WOW64_INFORMATION_CLASS,
                &mut wow64_peb as *mut usize as *mut c_void,
                size_of::<usize>() as u32,
                &mut ret_len,
            )
        };

        if wow_status >= 0 && wow64_peb != 0 {
            if read_peb_32(handle.raw(), wow64_peb as u32, &mut out) {
                out.peb_readable = true;
            }
            return out;
        }

        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        let mut pbi: ProcessBasicInformation64 = unsafe { zeroed() };
        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        let basic_status = unsafe {
            NtQueryInformationProcess(
                handle.raw(),
                PROCESS_BASIC_INFORMATION_CLASS,
                &mut pbi as *mut ProcessBasicInformation64 as *mut c_void,
                size_of::<ProcessBasicInformation64>() as u32,
                &mut ret_len,
            )
        };

        if basic_status >= 0
            && pbi.peb_base_address != 0
            && read_peb_64(handle.raw(), pbi.peb_base_address, &mut out)
        {
            out.peb_readable = true;
        }

        out
    }

    fn read_peb_64(handle: HANDLE, peb_addr: u64, out: &mut PebInspection) -> bool {
        let params_ptr = match read_remote_u64(handle, peb_addr + 0x20) {
            Some(p) if p != 0 => p,
            _ => return false,
        };

        if let Some(us) = read_remote_struct::<UnicodeString64>(handle, params_ptr + 0x38) {
            if let Some(s) = read_remote_utf16(handle, us.buffer, us.length as usize) {
                let trimmed = s.trim_end_matches('\0');
                if !trimmed.is_empty() {
                    out.working_dir = Some(PathBuf::from(trimmed));
                }
            }
        }

        if out.image_path.is_none() {
            if let Some(us) = read_remote_struct::<UnicodeString64>(handle, params_ptr + 0x60) {
                if let Some(s) = read_remote_utf16(handle, us.buffer, us.length as usize) {
                    let trimmed = s.trim_end_matches('\0');
                    if !trimmed.is_empty() {
                        out.image_path = Some(PathBuf::from(trimmed));
                    }
                }
            }
        }

        if let Some(us) = read_remote_struct::<UnicodeString64>(handle, params_ptr + 0x70) {
            if let Some(s) = read_remote_utf16(handle, us.buffer, us.length as usize) {
                let trimmed = s.trim_end_matches('\0');
                if !trimmed.is_empty() {
                    out.cmdline = Some(trimmed.to_string());
                }
            }
        }

        if let Some(env_ptr) = read_remote_u64(handle, params_ptr + 0x80) {
            if env_ptr != 0 {
                let env_size = read_remote_u64(handle, params_ptr + 0x3F0)
                    .unwrap_or(64 * 1024)
                    .min(64 * 1024) as usize;
                read_selected_env_vars(handle, env_ptr, env_size, out);
            }
        }

        out.working_dir.is_some() || out.cmdline.is_some()
    }

    fn read_peb_32(handle: HANDLE, peb32_addr: u32, out: &mut PebInspection) -> bool {
        let params_ptr = match read_remote_u32(handle, peb32_addr as u64 + 0x10) {
            Some(p) if p != 0 => p as u64,
            _ => return false,
        };

        if let Some(us) = read_remote_struct::<UnicodeString32>(handle, params_ptr + 0x24) {
            if let Some(s) = read_remote_utf16(handle, us.buffer as u64, us.length as usize) {
                let trimmed = s.trim_end_matches('\0');
                if !trimmed.is_empty() {
                    out.working_dir = Some(PathBuf::from(trimmed));
                }
            }
        }

        if out.image_path.is_none() {
            if let Some(us) = read_remote_struct::<UnicodeString32>(handle, params_ptr + 0x38) {
                if let Some(s) = read_remote_utf16(handle, us.buffer as u64, us.length as usize) {
                    let trimmed = s.trim_end_matches('\0');
                    if !trimmed.is_empty() {
                        out.image_path = Some(PathBuf::from(trimmed));
                    }
                }
            }
        }

        if let Some(us) = read_remote_struct::<UnicodeString32>(handle, params_ptr + 0x40) {
            if let Some(s) = read_remote_utf16(handle, us.buffer as u64, us.length as usize) {
                let trimmed = s.trim_end_matches('\0');
                if !trimmed.is_empty() {
                    out.cmdline = Some(trimmed.to_string());
                }
            }
        }

        if let Some(env_ptr) = read_remote_u32(handle, params_ptr + 0x48) {
            if env_ptr != 0 {
                read_selected_env_vars(handle, env_ptr as u64, 32 * 1024, out);
            }
        }

        out.working_dir.is_some() || out.cmdline.is_some()
    }

    fn read_selected_env_vars(
        handle: HANDLE,
        env_ptr: u64,
        max_bytes: usize,
        out: &mut PebInspection,
    ) {
        let capped = max_bytes.min(64 * 1024);
        if capped < 4 {
            return;
        }
        let mut buf = vec![0u16; capped / 2];
        let mut read_bytes = 0usize;
        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                env_ptr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                capped,
                Some(&mut read_bytes),
            )
        };
        if ok.is_err() && read_bytes < 4 {
            return;
        }
        let units = (read_bytes / 2).min(buf.len());
        let mut start = 0;
        while start < units {
            let rel_end = match buf[start..units].iter().position(|&c| c == 0) {
                Some(pos) => pos,
                None => break,
            };
            if rel_end == 0 {
                break;
            }
            let entry = String::from_utf16_lossy(&buf[start..start + rel_end]);
            if let Some((k, v)) = entry.split_once('=') {
                if !v.is_empty() {
                    if k.eq_ignore_ascii_case("VIRTUAL_ENV") {
                        out.virtual_env = Some(v.to_string());
                    } else if k.eq_ignore_ascii_case("COMPOSE_PROJECT_NAME") {
                        out.compose_project_env = Some(v.to_string());
                    }
                    if is_safe_restart_env_key(k) && out.dev_env.len() < 16 {
                        out.dev_env.push((k.to_string(), v.to_string()));
                    }
                }
            }
            start += rel_end + 1;
        }
    }

    fn read_remote_u64(handle: HANDLE, addr: u64) -> Option<u64> {
        read_remote_struct::<u64>(handle, addr)
    }

    fn read_remote_u32(handle: HANDLE, addr: u64) -> Option<u32> {
        read_remote_struct::<u32>(handle, addr)
    }

    fn read_remote_struct<T: Copy>(handle: HANDLE, addr: u64) -> Option<T> {
        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        unsafe {
            let mut val: T = zeroed();
            let mut read = 0usize;
            if ReadProcessMemory(
                handle,
                addr as *const c_void,
                &mut val as *mut T as *mut c_void,
                size_of::<T>(),
                Some(&mut read),
            )
            .is_ok()
                && read == size_of::<T>()
            {
                Some(val)
            } else {
                None
            }
        }
    }

    fn read_remote_utf16(handle: HANDLE, addr: u64, byte_len: usize) -> Option<String> {
        if addr == 0 || byte_len == 0 || !byte_len.is_multiple_of(2) {
            return None;
        }
        let capped_bytes = byte_len.min(32 * 1024);
        let mut buf = vec![0u16; capped_bytes / 2];
        let mut read = 0usize;
        // SAFETY: Process HANDLE and output buffers/lengths match the Win32/NT API ABI.
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                capped_bytes,
                Some(&mut read),
            )
        };
        if ok.is_err() || read < 2 {
            return None;
        }
        let units = (read / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..units]))
    }

    pub fn query_full_image_name(handle: HANDLE) -> Option<PathBuf> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: Valid Win32 handle/buffer sized for the query.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        if ok.is_ok() && len > 0 {
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
        } else {
            None
        }
    }

    pub fn open_process_owned(rights: PROCESS_ACCESS_RIGHTS, pid: u32) -> Option<OwnedHandle> {
        // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
        unsafe { OpenProcess(rights, false, pid).ok().map(OwnedHandle) }
    }

    pub fn get_process_create_time_100ns(handle: HANDLE) -> Option<i64> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
        unsafe {
            GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).ok()?;
        }
        Some((((creation.dwHighDateTime as u64) << 32) | (creation.dwLowDateTime as u64)) as i64)
    }

    pub fn is_process_alive_handle(handle: HANDLE) -> bool {
        // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
        unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT }
    }

    fn open_process_verified(rights: PROCESS_ACCESS_RIGHTS, key: ProcessKey) -> Option<OwnedHandle> {
        let h = open_process_owned(rights, key.pid)?;
        if key.create_time_100ns > 0 {
            let actual = get_process_create_time_100ns(h.raw())?;
            if actual != key.create_time_100ns {
                return None;
            }
        }
        Some(h)
    }

    pub fn open_process_for_wait(key: ProcessKey) -> Option<OwnedHandle> {
        open_process_verified(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, key)
    }

    pub fn open_process_for_terminate(key: ProcessKey) -> Option<OwnedHandle> {
        open_process_verified(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            key,
        )
    }

    pub fn is_process_critical_kernel(pid: u32) -> bool {
        let Some(h) = open_process_owned(PROCESS_QUERY_LIMITED_INFORMATION, pid) else {
            return false;
        };
        let mut critical = BOOL(0);
        // SAFETY: Current process pseudo-handle trim call with valid flags.
        unsafe {
            if IsProcessCritical(h.raw(), &mut critical).is_ok() {
                critical.as_bool()
            } else {
                false
            }
        }
    }

    pub fn current_pid() -> u32 {
        // SAFETY: Current process pseudo-handle trim call with valid flags.
        unsafe { GetCurrentProcessId() }
    }

    pub fn get_exit_code(handle: HANDLE) -> Option<i32> {
        let mut code = 0u32;
        // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
        unsafe {
            if GetExitCodeProcess(handle, &mut code).is_ok() {
                Some(code as i32)
            } else {
                None
            }
        }
    }

    /// Reads registered WSL2 distribution names from `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`.
    pub fn list_wsl_distros_from_registry() -> Vec<String> {
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\0"
            .encode_utf16()
            .collect();
        let mut root_key = HKEY::default();
        // SAFETY: Registry HKEY is opened/closed within scope with NUL-terminated UTF-16 names and exact byte buffers.
        let open_res = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                0,
                KEY_READ,
                &mut root_key,
            )
        };
        if open_res.is_err() {
            return Vec::new();
        }

        let mut distros = Vec::new();
        let val_name: Vec<u16> = "DistributionName\0".encode_utf16().collect();

        for idx in 0..32u32 {
            let mut name_buf = [0u16; 128];
            let mut name_len = name_buf.len() as u32;
            // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
            let enum_res = unsafe {
                RegEnumKeyExW(
                    root_key,
                    idx,
                    PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    PWSTR::null(),
                    None,
                    None,
                )
            };
            if enum_res.is_err() {
                break;
            }

            let mut child_key = HKEY::default();
            // SAFETY: Registry HKEY is opened/closed within scope with NUL-terminated UTF-16 names and exact byte buffers.
            let child_open = unsafe {
                RegOpenKeyExW(
                    root_key,
                    PCWSTR(name_buf.as_ptr()),
                    0,
                    KEY_READ,
                    &mut child_key,
                )
            };
            if child_open.is_err() {
                continue;
            }

            let mut data = [0u16; 128];
            let mut data_bytes = (data.len() * 2) as u32;
            let mut val_type = REG_VALUE_TYPE::default();
            // SAFETY: Registry HKEY is opened/closed within scope with NUL-terminated UTF-16 names and exact byte buffers.
            let q_res = unsafe {
                RegQueryValueExW(
                    child_key,
                    PCWSTR(val_name.as_ptr()),
                    None,
                    Some(&mut val_type),
                    Some(data.as_mut_ptr() as *mut u8),
                    Some(&mut data_bytes),
                )
            };
            // SAFETY: Registry HKEY is opened/closed within scope with NUL-terminated UTF-16 names and exact byte buffers.
            unsafe {
                let _ = RegCloseKey(child_key);
            }

            if q_res.is_ok() && val_type == REG_SZ && data_bytes >= 2 {
                let units = (data_bytes as usize / 2).min(data.len());
                let s = String::from_utf16_lossy(&data[..units])
                    .trim_end_matches('\0')
                    .to_string();
                if !s.is_empty() {
                    distros.push(s);
                }
            }
        }

        // SAFETY: Registry HKEY is opened/closed within scope with NUL-terminated UTF-16 names and exact byte buffers.
        unsafe {
            let _ = RegCloseKey(root_key);
        }
        distros
    }

    /// Checks if `\\.\pipe\docker_engine` is ready and fetches running container metadata over HTTP/1.0
    /// using a bounded 250ms timeout so a wedged Docker Desktop pipe never blocks the engine thread.
    pub fn query_docker_containers_pipe() -> Vec<DockerContainerSummary> {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Duration;

        static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
        if IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Vec::new();
        }

        let pipe_name_wide: Vec<u16> = "\\\\.\\pipe\\docker_engine\0".encode_utf16().collect();
        // SAFETY: Null-terminated UTF-16 pipe path lives across WaitNamedPipeW.
        let ready = unsafe { WaitNamedPipeW(PCWSTR(pipe_name_wide.as_ptr()), 40) };
        if !ready.as_bool() {
            IN_FLIGHT.store(false, Ordering::Release);
            return Vec::new();
        }

        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("bbstats-docker-pipe".to_string())
            .spawn(move || {
                let res = (|| {
                    let mut pipe = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(r"\\.\pipe\docker_engine")
                        .ok()?;
                    let req = b"GET /containers/json HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                    pipe.write_all(req).ok()?;
                    let mut raw = Vec::with_capacity(8192);
                    let mut chunk = [0u8; 4096];
                    while raw.len() < 2 * 1024 * 1024 {
                        match pipe.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => raw.extend_from_slice(&chunk[..n]),
                            Err(_) => break,
                        }
                    }
                    parse_http_200_body(&raw)
                        .as_deref()
                        .map(parse_docker_containers_json)
                })()
                .unwrap_or_default();
                IN_FLIGHT.store(false, Ordering::Release);
                let _ = tx.send(res);
            });

        if spawned.is_err() {
            IN_FLIGHT.store(false, Ordering::Release);
            return Vec::new();
        }
        rx.recv_timeout(Duration::from_millis(250)).unwrap_or_default()
    }

    fn docker_pipe_http_request(req_bytes: Vec<u8>, timeout_ms: u64) -> Option<Vec<u8>> {
        use std::io::{Read, Write};
        use std::sync::mpsc;
        use std::time::Duration;

        let pipe_name_wide: Vec<u16> = "\\\\.\\pipe\\docker_engine\0".encode_utf16().collect();
        // SAFETY: Null-terminated UTF-16 pipe path lives across WaitNamedPipeW.
        let ready = unsafe { WaitNamedPipeW(PCWSTR(pipe_name_wide.as_ptr()), 40) };
        if !ready.as_bool() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("bbstats-docker-op".to_string())
            .spawn(move || {
                let res = (|| {
                    let mut pipe = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(r"\\.\pipe\docker_engine")
                        .ok()?;
                    pipe.write_all(&req_bytes).ok()?;
                    let mut raw = Vec::with_capacity(8192);
                    let mut chunk = [0u8; 4096];
                    while raw.len() < 1024 * 1024 {
                        match pipe.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => raw.extend_from_slice(&chunk[..n]),
                            Err(_) => break,
                        }
                    }
                    Some(raw)
                })();
                let _ = tx.send(res);
            })
            .ok()?;
        rx.recv_timeout(Duration::from_millis(timeout_ms)).ok().flatten()
    }

    pub fn query_docker_container_logs(container_name: &str, max_lines: usize) -> Vec<String> {
        let Some(safe_name) = sanitize_docker_container_name(container_name) else {
            return Vec::new();
        };
        let tail = max_lines.clamp(10, 200);
        let req = format!(
            "GET /containers/{safe_name}/logs?stdout=1&stderr=1&tail={tail} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
        let Some(raw) = docker_pipe_http_request(req.into_bytes(), 300) else {
            return Vec::new();
        };
        parse_docker_logs_payload(&raw, max_lines)
    }

    pub fn control_docker_container(container_name: &str, action: &str) -> bool {
        let Some(safe_name) = sanitize_docker_container_name(container_name) else {
            return false;
        };
        if !matches!(action, "stop" | "restart") {
            return false;
        }
        let req = format!(
            "POST /containers/{safe_name}/{action}?t=5 HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let Some(raw) = docker_pipe_http_request(req.into_bytes(), 6000) else {
            return false;
        };
        let head = std::str::from_utf8(&raw).unwrap_or("");
        head.lines()
            .next()
            .is_some_and(|l| l.contains(" 204 ") || l.contains(" 200 ") || l.contains(" 304 "))
    }

    pub fn query_wsl_dev_processes() -> Vec<WslProcessSummary> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let Ok(out) = Command::new("wsl.exe")
            .args([
                "-e",
                "sh",
                "-c",
                "ps -eo pid=,pcpu=,rss=,comm=,args= | head -n 96",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        parse_wsl_ps_output(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform_imp {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const WINDOWS_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;

    pub fn logical_core_count() -> u32 {
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
            .max(1)
    }

    pub fn query_system_times_100ns() -> Option<(u64, u64, u64)> {
        #[cfg(target_os = "linux")]
        {
            let text = std::fs::read_to_string("/proc/stat").ok()?;
            let cpu_line = text.lines().find(|l| l.starts_with("cpu "))?;
            let nums: Vec<u64> = cpu_line
                .split_whitespace()
                .skip(1)
                .filter_map(|s| s.parse::<u64>().ok())
                .collect();
            if nums.len() >= 4 {
                let user = nums[0] + nums[1];
                let kernel = nums[2] + nums.get(5).copied().unwrap_or(0) + nums.get(6).copied().unwrap_or(0);
                let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
                // Scale ticks by 100_000 (ratios only matter in collector.rs).
                return Some((idle * 100_000, kernel * 100_000, user * 100_000));
            }
            None
        }
        #[cfg(target_os = "macos")]
        {
            const HOST_CPU_LOAD_INFO: i32 = 3;
            const HOST_CPU_LOAD_INFO_COUNT: u32 = 4;
            extern "C" {
                fn mach_host_self() -> u32;
                fn host_statistics(
                    host_priv: u32,
                    flavor: i32,
                    host_info_out: *mut u32,
                    host_info_out_cnt: *mut u32,
                ) -> i32;
            }
            let mut ticks = [0u32; 4]; // [USER, SYSTEM, IDLE, NICE]
            let mut count = HOST_CPU_LOAD_INFO_COUNT;
            // SAFETY: Mach host_statistics fills the 4-u32 cpu load info array on macOS.
            let kr = unsafe {
                host_statistics(
                    mach_host_self(),
                    HOST_CPU_LOAD_INFO,
                    ticks.as_mut_ptr(),
                    &mut count,
                )
            };
            if kr == 0 && count >= 4 {
                let user = (ticks[0] as u64) + (ticks[3] as u64);
                let sys = ticks[1] as u64;
                let idle = ticks[2] as u64;
                return Some((idle * 100_000, sys * 100_000, user * 100_000));
            }
            None
        }
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            None
        }
    }

    pub fn query_system_memory() -> (u64, u64) {
        #[cfg(target_os = "macos")]
        {
            static TOTAL_MEM: OnceLock<u64> = OnceLock::new();
            let total = *TOTAL_MEM.get_or_init(|| {
                Command::new("sysctl")
                    .args(["-n", "hw.memsize"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0)
            });

            let used = Command::new("vm_stat")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| parse_vm_stat_used_bytes(&String::from_utf8_lossy(&o.stdout)))
                .unwrap_or(0);
            (used.min(total), total)
        }
        #[cfg(target_os = "linux")]
        {
            let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
                return (0, 0);
            };
            let mut total_kb = 0u64;
            let mut avail_kb = 0u64;
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    total_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
                } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                    avail_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
            }
            let total = total_kb.saturating_mul(1024);
            let used = total_kb.saturating_sub(avail_kb).saturating_mul(1024);
            (used.min(total), total)
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
        {
            (0, 0)
        }
    }

    pub fn current_system_time_100ns() -> i64 {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        ((secs + WINDOWS_EPOCH_OFFSET_SECS as f64) * 10_000_000.0) as i64
    }

    static ANCHORS: OnceLock<Mutex<HashMap<u32, (i64, i64)>>> = OnceLock::new();

    fn stable_create_time_100ns(pid: u32, etime_100ns: i64, now_100ns: i64) -> i64 {
        let cache = ANCHORS.get_or_init(|| Mutex::new(HashMap::with_capacity(256)));
        let approx = now_100ns.saturating_sub(etime_100ns).max(1);
        let Ok(mut map) = cache.lock() else {
            return approx;
        };
        if let Some(&(anchored, prev_etime)) = map.get(&pid) {
            if etime_100ns + 20_000_000 >= prev_etime && (approx - anchored).abs() <= 50_000_000 {
                map.insert(pid, (anchored, etime_100ns));
                return anchored;
            }
        }
        map.insert(pid, (approx, etime_100ns));
        approx
    }

    fn prune_stale_anchors(active_pids: &std::collections::HashSet<u32>) {
        if let Some(cache) = ANCHORS.get() {
            if let Ok(mut map) = cache.lock() {
                if map.len() > active_pids.len().saturating_add(128) {
                    map.retain(|pid, _| active_pids.contains(pid));
                }
            }
        }
    }

    /// Verifies that `key.pid` is still alive and belongs to the same process start time.
    pub fn verify_unix_process_key(key: ProcessKey) -> bool {
        if key.pid <= 1 {
            return false;
        }
        let pid_str = key.pid.to_string();
        let Ok(out) = Command::new("ps")
            .args(["-p", &pid_str, "-o", "etime="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return false;
        }
        if key.create_time_100ns <= 0 {
            return true;
        }
        let Some(etime_100ns) = parse_ps_duration_100ns(trimmed) else {
            return false;
        };
        let approx = current_system_time_100ns().saturating_sub(etime_100ns);
        // Tightened to 2.5 seconds to minimize PID-reuse window while allowing 1s ps jitter.
        (approx - key.create_time_100ns).abs() <= 25_000_000
    }

    pub fn enumerate_processes(_scratch_buf: &mut Vec<u64>, out: &mut Vec<RawProcessEntry>) {
        out.clear();
        #[cfg(target_os = "linux")]
        {
            if let Ok(entries) = std::fs::read_dir("/proc") {
                let uptime_secs = std::fs::read_to_string("/proc/uptime")
                    .ok()
                    .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
                    .unwrap_or(0.0);
                // SAFETY: sysconf with standard POSIX constants returns positive scalar values.
                let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
                // SAFETY: sysconf with standard POSIX constants returns positive scalar values.
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
                let now_100ns = current_system_time_100ns();
                let mut active_pids = std::collections::HashSet::with_capacity(256);

                for entry in entries.flatten() {
                    let fname = entry.file_name();
                    let Some(pid_str) = fname.to_str() else {
                        continue;
                    };
                    if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
                        continue;
                    }
                    let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                        continue;
                    };
                    let Some(parsed) = parse_proc_stat_entry(&stat) else {
                        continue;
                    };
                    active_pids.insert(parsed.pid);
                    let cpu_100ns =
                        ((parsed.cpu_ticks as f64 / clk_tck) * 10_000_000.0) as i64;
                    let start_secs = parsed.starttime_ticks as f64 / clk_tck;
                    let etime_100ns =
                        ((uptime_secs - start_secs).max(0.0) * 10_000_000.0) as i64;
                    let create_time_100ns =
                        stable_create_time_100ns(parsed.pid, etime_100ns, now_100ns);
                    let private_ram_bytes = parsed.rss_pages.saturating_mul(page_size);
                    out.push(RawProcessEntry {
                        pid: parsed.pid,
                        parent_pid: parsed.ppid,
                        session_id: if parsed.pid <= 1 { 0 } else { 1 },
                        create_time_100ns,
                        cpu_time_100ns: cpu_100ns,
                        private_ram_bytes,
                        image_name: parsed.comm,
                    });
                }
                if !out.is_empty() {
                    prune_stale_anchors(&active_pids);
                    return;
                }
            }
        }

        let Ok(output) = Command::new("ps")
            .args(["-axo", "pid=,ppid=,rss=,etime=,time=,comm="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let now_100ns = current_system_time_100ns();
        let text = String::from_utf8_lossy(&output.stdout);
        let mut active_pids = std::collections::HashSet::with_capacity(256);
        for line in text.lines() {
            let Some((pid, ppid, rss_kb, etime_100ns, cpu_time_100ns, comm_raw)) =
                parse_ps_line(line)
            else {
                continue;
            };
            active_pids.insert(pid);
            let image_name = Path::new(comm_raw)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("pid-{pid}"));

            let create_time_100ns = stable_create_time_100ns(pid, etime_100ns, now_100ns);
            out.push(RawProcessEntry {
                pid,
                parent_pid: ppid,
                session_id: if pid <= 1 { 0 } else { 1 },
                create_time_100ns,
                cpu_time_100ns,
                private_ram_bytes: rss_kb.saturating_mul(1024),
                image_name,
            });
        }
        prune_stale_anchors(&active_pids);
    }

    /// Batch-prefetches `comm`, `args`, `cwd`, and safe developer env vars.
    pub fn prefetch_unix_pebs(pids: &[u32]) -> HashMap<u32, PebInspection> {
        let mut map: HashMap<u32, PebInspection> = HashMap::with_capacity(pids.len());
        let unique: Vec<u32> = {
            let mut seen = HashSet::with_capacity(pids.len());
            pids.iter()
                .copied()
                .filter(|&p| p > 1 && seen.insert(p))
                .collect()
        };

        #[cfg(target_os = "linux")]
        {
            for &pid in &unique {
                let proc_dir = PathBuf::from(format!("/proc/{pid}"));
                if !proc_dir.is_dir() {
                    continue;
                }
                let mut peb = PebInspection::default();
                if let Ok(exe) = std::fs::read_link(proc_dir.join("exe")) {
                    peb.image_path = Some(exe);
                }
                if let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) {
                    peb.working_dir = Some(cwd);
                }
                if let Ok(raw_cmd) = std::fs::read(proc_dir.join("cmdline")) {
                    let args: Vec<String> = raw_cmd
                        .split(|&b| b == 0)
                        .filter(|s| !s.is_empty())
                        .map(|s| String::from_utf8_lossy(s).into_owned())
                        .collect();
                    if !args.is_empty() {
                        peb.cmdline = Some(args.join(" "));
                    }
                }
                if let Ok(raw_env) = std::fs::read(proc_dir.join("environ")) {
                    for entry in raw_env.split(|&b| b == 0).filter(|s| !s.is_empty()) {
                        let s = String::from_utf8_lossy(entry);
                        if let Some((k, v)) = s.split_once('=') {
                            apply_captured_env_entry(k, v, &mut peb);
                        }
                    }
                }
                peb.peb_readable =
                    peb.cmdline.is_some() || peb.working_dir.is_some() || peb.image_path.is_some();
                map.insert(pid, peb);
            }
            if !map.is_empty() {
                return map;
            }
        }

        for chunk in unique.chunks(64) {
            let csv = chunk
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");

            if let Ok(comm) = Command::new("ps")
                .args(["-p", &csv, "-o", "pid=,comm="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                for line in String::from_utf8_lossy(&comm.stdout).lines() {
                    let t = line.trim();
                    if let Some((pid_s, rest)) = t.split_once(char::is_whitespace) {
                        if let Ok(pid) = pid_s.parse::<u32>() {
                            let val = rest.trim();
                            if !val.is_empty() {
                                map.entry(pid).or_default().image_path = Some(PathBuf::from(val));
                            }
                        }
                    }
                }
            }

            if let Ok(args) = Command::new("ps")
                .args(["-p", &csv, "-o", "pid=,args="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                for line in String::from_utf8_lossy(&args.stdout).lines() {
                    let t = line.trim();
                    if let Some((pid_s, rest)) = t.split_once(char::is_whitespace) {
                        if let Ok(pid) = pid_s.parse::<u32>() {
                            let val = rest.trim();
                            if !val.is_empty() {
                                map.entry(pid).or_default().cmdline = Some(val.to_string());
                            }
                        }
                    }
                }
            }

            // Capture developer env vars (VIRTUAL_ENV, PORT, NODE_ENV, etc.) via `ps eww`.
            if let Ok(envs) = Command::new("ps")
                .args(["eww", "-p", &csv, "-o", "pid=,command="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                for line in String::from_utf8_lossy(&envs.stdout).lines() {
                    let t = line.trim();
                    if let Some((pid_s, rest)) = t.split_once(char::is_whitespace) {
                        if let Ok(pid) = pid_s.parse::<u32>() {
                            let peb = map.entry(pid).or_default();
                            for tok in rest.split_whitespace() {
                                if let Some((k, v)) = tok.split_once('=') {
                                    apply_captured_env_entry(k, v, peb);
                                }
                            }
                        }
                    }
                }
            }

            if let Ok(cwd) = Command::new("lsof")
                .args(["-a", "-p", &csv, "-d", "cwd", "-Fn"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                let mut cur_pid = 0u32;
                for line in String::from_utf8_lossy(&cwd.stdout).lines() {
                    if let Some(p) = line.strip_prefix('p') {
                        cur_pid = p.trim().parse::<u32>().unwrap_or(0);
                    } else if let Some(path) = line.strip_prefix('n') {
                        let trimmed = path.trim();
                        if cur_pid > 1 && !trimmed.is_empty() {
                            map.entry(cur_pid)
                                .or_default()
                                .working_dir
                                .get_or_insert_with(|| PathBuf::from(trimmed));
                        }
                    }
                }
            }
        }
        for peb in map.values_mut() {
            peb.peb_readable = peb.cmdline.is_some() || peb.working_dir.is_some();
        }
        map
    }

    pub fn inspect_process_peb(pid: u32) -> PebInspection {
        prefetch_unix_pebs(&[pid]).remove(&pid).unwrap_or_default()
    }

    pub fn is_process_critical_kernel(pid: u32) -> bool {
        pid <= 1
    }

    pub fn current_pid() -> u32 {
        std::process::id()
    }

    pub fn list_wsl_distros_from_registry() -> Vec<String> {
        Vec::new()
    }

    fn docker_socket_candidates() -> Vec<PathBuf> {
        let mut candidates = vec![PathBuf::from("/var/run/docker.sock")];
        if let Some(home) = std::env::var_os("HOME") {
            let h = PathBuf::from(home);
            candidates.push(h.join(".docker").join("run").join("docker.sock"));
            candidates.push(h.join(".orbstack").join("run").join("docker.sock"));
        }
        candidates
    }

    pub fn docker_socket_exists() -> bool {
        docker_socket_candidates().into_iter().any(|p| p.exists())
    }

    pub fn query_docker_containers_pipe() -> Vec<DockerContainerSummary> {
        for sock in docker_socket_candidates() {
            if !sock.exists() {
                continue;
            }
            let Ok(mut stream) = UnixStream::connect(&sock) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(80)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(80)));
            let req = b"GET /containers/json HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(req).is_err() {
                continue;
            }
            let mut raw = Vec::with_capacity(8192);
            let mut chunk = [0u8; 4096];
            while raw.len() < 2 * 1024 * 1024 {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => raw.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            if let Some(body) = parse_http_200_body(&raw) {
                return parse_docker_containers_json(&body);
            }
        }
        Vec::new()
    }

    fn docker_unix_http_request(req_bytes: &[u8], timeout_ms: u64) -> Option<Vec<u8>> {
        for sock in docker_socket_candidates() {
            if !sock.exists() {
                continue;
            }
            let Ok(mut stream) = UnixStream::connect(&sock) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(timeout_ms)));
            if stream.write_all(req_bytes).is_err() {
                continue;
            }
            let mut raw = Vec::with_capacity(8192);
            let mut chunk = [0u8; 4096];
            while raw.len() < 1024 * 1024 {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => raw.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            if !raw.is_empty() {
                return Some(raw);
            }
        }
        None
    }

    pub fn query_docker_container_logs(container_name: &str, max_lines: usize) -> Vec<String> {
        let Some(safe_name) = sanitize_docker_container_name(container_name) else {
            return Vec::new();
        };
        let tail = max_lines.clamp(10, 200);
        let req = format!(
            "GET /containers/{safe_name}/logs?stdout=1&stderr=1&tail={tail} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
        let Some(raw) = docker_unix_http_request(req.as_bytes(), 300) else {
            return Vec::new();
        };
        parse_docker_logs_payload(&raw, max_lines)
    }

    pub fn control_docker_container(container_name: &str, action: &str) -> bool {
        let Some(safe_name) = sanitize_docker_container_name(container_name) else {
            return false;
        };
        if !matches!(action, "stop" | "restart") {
            return false;
        }
        let req = format!(
            "POST /containers/{safe_name}/{action}?t=5 HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let Some(raw) = docker_unix_http_request(req.as_bytes(), 6000) else {
            return false;
        };
        let head = std::str::from_utf8(&raw).unwrap_or("");
        head.lines()
            .next()
            .is_some_and(|l| l.contains(" 204 ") || l.contains(" 200 ") || l.contains(" 304 "))
    }

    pub fn query_wsl_dev_processes() -> Vec<WslProcessSummary> {
        Vec::new()
    }
}

/// Summary of an active developer runtime discovered inside WSL2 (`bbstats` opt-in WSL bridge).
#[derive(Debug, Clone, PartialEq)]
pub struct WslProcessSummary {
    pub pid: u32,
    pub cpu_percent: f32,
    pub ram_bytes: u64,
    pub comm: String,
    pub cmdline: String,
}

pub fn parse_wsl_ps_output(text: &str) -> Vec<WslProcessSummary> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let mut parts = t.split_whitespace();
        let Some(Ok(pid)) = parts.next().map(|s| s.parse::<u32>()) else {
            continue;
        };
        let Some(Ok(cpu)) = parts.next().map(|s| s.parse::<f32>()) else {
            continue;
        };
        let Some(Ok(rss_kb)) = parts.next().map(|s| s.parse::<u64>()) else {
            continue;
        };
        let Some(comm) = parts.next() else {
            continue;
        };
        let low = comm.to_ascii_lowercase();
        let is_dev = matches!(
            low.as_str(),
            "node" | "python" | "python3" | "cargo" | "rustc" | "go" | "ruby" | "java" | "vite"
                | "next-server" | "uvicorn" | "gunicorn" | "deno" | "bun" | "postgres"
                | "redis-server" | "mongod" | "dockerd"
        );
        if !is_dev {
            continue;
        }
        let rest_args: Vec<&str> = parts.collect();
        let cmdline = if rest_args.is_empty() {
            comm.to_string()
        } else {
            rest_args.join(" ")
        };
        out.push(WslProcessSummary {
            pid,
            cpu_percent: cpu.clamp(0.0, 100.0),
            ram_bytes: rss_kb.saturating_mul(1024),
            comm: comm.to_string(),
            cmdline,
        });
    }
    out
}

fn sanitize_docker_container_name(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_start_matches('/');
    if trimmed.is_empty()
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(any(target_os = "linux", test))]
pub(crate) struct ParsedProcStat {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub cpu_ticks: u64,
    pub starttime_ticks: u64,
    pub rss_pages: u64,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_proc_stat_entry(stat: &str) -> Option<ParsedProcStat> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid: u32 = stat[..open].trim().parse().ok()?;
    let comm = stat[open + 1..close].to_string();
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // Fields after `(comm)` start at index 0 = state (field 3 in proc(5)):
    // ppid=1, utime=11, stime=12, starttime=19, rss=21
    if rest.len() <= 21 {
        return None;
    }
    let ppid: u32 = rest[1].parse().ok()?;
    let utime: u64 = rest[11].parse().ok()?;
    let stime: u64 = rest[12].parse().ok()?;
    let starttime_ticks: u64 = rest[19].parse().ok()?;
    let rss_pages: u64 = rest[21].parse().ok()?;
    Some(ParsedProcStat {
        pid,
        ppid,
        comm,
        cpu_ticks: utime.saturating_add(stime),
        starttime_ticks,
        rss_pages,
    })
}

#[cfg(not(target_os = "windows"))]
fn apply_captured_env_entry(k: &str, v: &str, out: &mut PebInspection) {
    if v.is_empty() {
        return;
    }
    if k.eq_ignore_ascii_case("VIRTUAL_ENV") {
        out.virtual_env = Some(v.to_string());
    } else if k.eq_ignore_ascii_case("COMPOSE_PROJECT_NAME") {
        out.compose_project_env = Some(v.to_string());
    }
    if is_safe_restart_env_key(k) && out.dev_env.len() < 16 {
        out.dev_env.push((k.to_string(), v.to_string()));
    }
}

fn is_safe_restart_env_key(k: &str) -> bool {
    let up = k.to_ascii_uppercase();
    matches!(
        up.as_str(),
        "VIRTUAL_ENV"
            | "CONDA_PREFIX"
            | "CONDA_DEFAULT_ENV"
            | "NODE_ENV"
            | "PORT"
            | "HOST"
            | "COMPOSE_PROJECT_NAME"
            | "PYTHONPATH"
            | "RUST_LOG"
            | "ASPNETCORE_ENVIRONMENT"
            | "FLASK_ENV"
            | "DJANGO_SETTINGS_MODULE"
            | "BBSTATS_RUN_LOG"
            | "PATH"
    )
}

pub use platform_imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_containers_json_with_compose_labels_and_ports() {
        let sample = r#"[{"Id":"abc123def456","Names":["/saas-db-1"],"Image":"postgres:16","Labels":{"com.docker.compose.project":"saas-stack","com.docker.compose.service":"db"},"Ports":[{"PrivatePort":5432,"PublicPort":5432,"Type":"tcp"}]}]"#;
        let parsed = parse_docker_containers_json(sample);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "saas-db-1");
        assert_eq!(parsed[0].compose_project.as_deref(), Some("saas-stack"));
        assert_eq!(parsed[0].ports, vec![5432]);
    }

    #[test]
    fn parses_ps_elapsed_and_cpu_durations() {
        assert_eq!(parse_ps_duration_100ns("01:23"), Some(83 * 10_000_000));
        assert_eq!(
            parse_ps_duration_100ns("02:15:30.50"),
            Some((2 * 3600 + 15 * 60 + 30) * 10_000_000 + 5_000_000)
        );
        assert_eq!(
            parse_ps_duration_100ns("3-04:05:06"),
            Some((3 * 86_400 + 4 * 3600 + 5 * 60 + 6) * 10_000_000)
        );
    }

    #[test]
    fn parses_macos_vm_stat_memory_pages() {
        let sample = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                              12345.
Pages active:                           200000.
Pages inactive:                         100000.
Pages wired down:                        50000.
Pages occupied by compressor:            25000.
";
        let used = parse_vm_stat_used_bytes(sample).unwrap();
        assert_eq!(used, (200_000 + 50_000 + 25_000) * 16_384);
    }

    #[test]
    fn parses_ps_line_when_etime_equals_cputime_and_preserves_spaces() {
        let line = "  4321     1  12345   00:01   00:01 /Applications/My App/node";
        let (pid, ppid, rss, etime, cpu, comm) = parse_ps_line(line).unwrap();
        assert_eq!(pid, 4321);
        assert_eq!(ppid, 1);
        assert_eq!(rss, 12345);
        assert_eq!(etime, 10_000_000);
        assert_eq!(cpu, 10_000_000);
        assert_eq!(comm, "/Applications/My App/node");
    }

    #[test]
    fn parses_docker_status_uptime_and_multiplexed_logs() {
        assert_eq!(parse_docker_status_uptime_secs("Up 12 minutes"), Some(720));
        assert_eq!(parse_docker_status_uptime_secs("Up About an hour (healthy)"), Some(3600));
        assert_eq!(parse_docker_status_uptime_secs("Up 2 days"), Some(172_800));

        let msg1 = b"server listening on :3000\n";
        let msg2 = b"warn: slow query\n";
        let mut raw = b"HTTP/1.0 200 OK\r\nContent-Type: application/vnd.docker.raw-stream\r\n\r\n".to_vec();
        for (stream_id, msg) in [(1u8, msg1.as_slice()), (2u8, msg2.as_slice())] {
            raw.push(stream_id);
            raw.extend_from_slice(&[0, 0, 0]);
            raw.extend_from_slice(&(msg.len() as u32).to_be_bytes());
            raw.extend_from_slice(msg);
        }
        let lines = parse_docker_logs_payload(&raw, 10);
        assert_eq!(lines, vec!["server listening on :3000", "warn: slow query"]);
    }

    #[test]
    fn parses_linux_proc_stat_entry_with_spaces_in_comm() {
        let stat = "1234 (node worker) S 100 1234 100 0 -1 4194304 10 0 0 0 150 45 0 0 20 0 4 0 98765 1000000 4096 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";
        let parsed = parse_proc_stat_entry(stat).unwrap();
        assert_eq!(parsed.pid, 1234);
        assert_eq!(parsed.ppid, 100);
        assert_eq!(parsed.comm, "node worker");
        assert_eq!(parsed.cpu_ticks, 195);
        assert_eq!(parsed.starttime_ticks, 98765);
        assert_eq!(parsed.rss_pages, 4096);
    }

    #[test]
    fn parses_wsl_ps_dev_runtimes_and_ignores_system_daemons() {
        let sample = "\
  101  0.0  4200 systemd /sbin/init
  245  4.2 98304 node node /home/dev/app/server.js --port 3000
  310  1.5 32768 python3 python3 -m uvicorn main:app --reload
";
        let procs = parse_wsl_ps_output(sample);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 245);
        assert_eq!(procs[0].comm, "node");
        assert_eq!(procs[0].ram_bytes, 98304 * 1024);
        assert!(procs[1].cmdline.contains("uvicorn"));
    }
}
