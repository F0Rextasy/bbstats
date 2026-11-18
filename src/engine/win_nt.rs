//! Low-overhead OS telemetry wrappers.
//! - Windows: unsafe Win32 / NT native APIs (`NtQuerySystemInformation`, PEB reads, named pipes).
//! - macOS / Unix: `ps`, `lsof`, `sysctl`, `vm_stat`, and Unix domain socket queries.

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
    pub read_bytes: u64,
    pub write_bytes: u64,
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
    pub conda_env: Option<String>,
    pub compose_project_env: Option<String>,
    pub npm_package_name: Option<String>,
}

/// Summary of a running Docker container parsed from Engine API `/containers/json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainerSummary {
    pub name: String,
    pub compose_project: Option<String>,
    pub ports: Vec<u16>,
}

/// Lightweight parser for Docker `/containers/json` array without needing `serde_json`.
pub fn parse_docker_containers_json(body: &str) -> Vec<DockerContainerSummary> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut obj_start = None;
    let mut in_str = false;
    let mut escape = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => {
                    if depth == 0 {
                        obj_start = Some(i);
                    }
                    depth += 1;
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = obj_start.take() {
                            let obj = &body[start..=i];
                            if let Some(summary) = parse_single_docker_obj(obj) {
                                out.push(summary);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    out
}

fn parse_single_docker_obj(obj: &str) -> Option<DockerContainerSummary> {
    let id = extract_json_str(obj, "\"Id\"")?;
    let compose_project = extract_json_str(obj, "\"com.docker.compose.project\"");
    let name = extract_first_array_str(obj, "\"Names\"")
        .map(|s| s.trim_start_matches('/').to_string())
        .unwrap_or_else(|| id.chars().take(12).collect());

    let mut ports = Vec::new();
    let mut search = obj;
    while let Some(idx) = search.find("\"PublicPort\":") {
        let rest = &search[idx + "\"PublicPort\":".len()..];
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(p) = digits.parse::<u16>() {
            if p > 0 && !ports.contains(&p) {
                ports.push(p);
            }
        }
        search = rest;
    }

    Some(DockerContainerSummary {
        name,
        compose_project,
        ports,
    })
}

fn extract_json_str(haystack: &str, key: &str) -> Option<String> {
    let idx = haystack.find(key)?;
    let rest = haystack[idx + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    parse_json_quoted(rest).map(|(s, _)| s)
}

fn extract_first_array_str(haystack: &str, key: &str) -> Option<String> {
    let idx = haystack.find(key)?;
    let rest = haystack[idx + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('[')?.trim_start();
    parse_json_quoted(rest).map(|(s, _)| s)
}

fn parse_json_quoted(s: &str) -> Option<(String, &str)> {
    let rest = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '"' => return Some((out, &rest[idx + 1..])),
            '\\' => {
                if let Some((_, esc)) = chars.next() {
                    out.push(esc);
                }
            }
            _ => out.push(ch),
        }
    }
    None
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
    use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HANDLE};
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
        GetCurrentProcessId, GetExitCodeProcess, GetSystemTimes, IsProcessCritical, OpenProcess,
        QueryFullProcessImageNameW, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_READ,
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
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    /// Returns the number of logical CPU cores on this machine (minimum 1).
    pub fn logical_core_count() -> u32 {
        unsafe {
            let mut info: SYSTEM_INFO = zeroed();
            GetSystemInfo(&mut info);
            info.dwNumberOfProcessors.max(1)
        }
    }

    /// Queries system-wide `(idle_100ns, kernel_100ns, user_100ns)`.
    /// Note: on Windows, `kernel_100ns` includes `idle_100ns`.
    pub fn query_system_times_100ns() -> Option<(u64, u64, u64)> {
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
        let ft = unsafe { GetSystemTimeAsFileTime() };
        (((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)) as i64
    }

    /// Enumerates all OS processes into `out` reusing `scratch_buf`.
    #[cfg(target_pointer_width = "64")]
    pub fn enumerate_processes(scratch_buf: &mut Vec<u8>, out: &mut Vec<RawProcessEntry>) {
        out.clear();
        if scratch_buf.capacity() < 768 * 1024 {
            scratch_buf.reserve(768 * 1024 - scratch_buf.capacity());
        }
        if scratch_buf.is_empty() {
            scratch_buf.resize(768 * 1024, 0);
        }

        let mut ret_len: u32 = 0;
        loop {
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    scratch_buf.as_mut_ptr() as *mut c_void,
                    scratch_buf.len() as u32,
                    &mut ret_len,
                )
            };

            if status >= 0 {
                break;
            }
            if status == STATUS_INFO_LENGTH_MISMATCH || status == STATUS_BUFFER_TOO_SMALL {
                let needed = (ret_len as usize).max(scratch_buf.len()) + 64 * 1024;
                scratch_buf.resize(needed, 0);
                continue;
            }
            return;
        }

        let valid_len = if (ret_len as usize) > 0 && (ret_len as usize) <= scratch_buf.len() {
            ret_len as usize
        } else {
            scratch_buf.len()
        };

        let base_ptr = scratch_buf.as_ptr();
        let base_addr = base_ptr as usize;
        let mut offset: usize = 0;

        while offset + size_of::<SystemProcessInformation>() <= valid_len {
            let entry = unsafe { &*(base_ptr.add(offset) as *const SystemProcessInformation) };
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
            let read_bytes = entry.read_transfer_count.max(0) as u64;
            let write_bytes = entry.write_transfer_count.max(0) as u64;

            out.push(RawProcessEntry {
                pid,
                parent_pid,
                session_id: entry.session_id,
                create_time_100ns: entry.create_time,
                cpu_time_100ns,
                private_ram_bytes,
                read_bytes,
                write_bytes,
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
        let slice = unsafe { std::slice::from_raw_parts(buf_addr as *const u16, byte_len / 2) };
        Some(String::from_utf16_lossy(slice))
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

        let mut pbi: ProcessBasicInformation64 = unsafe { zeroed() };
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
                if k.eq_ignore_ascii_case("VIRTUAL_ENV") && !v.is_empty() {
                    out.virtual_env = Some(v.to_string());
                } else if k.eq_ignore_ascii_case("CONDA_DEFAULT_ENV") && !v.is_empty() {
                    out.conda_env = Some(v.to_string());
                } else if k.eq_ignore_ascii_case("COMPOSE_PROJECT_NAME") && !v.is_empty() {
                    out.compose_project_env = Some(v.to_string());
                } else if k.eq_ignore_ascii_case("npm_package_name") && !v.is_empty() {
                    out.npm_package_name = Some(v.to_string());
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
        unsafe { OpenProcess(rights, false, pid).ok().map(OwnedHandle) }
    }

    pub fn open_process_for_wait(pid: u32) -> Option<OwnedHandle> {
        open_process_owned(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, pid)
    }

    pub fn open_process_for_terminate(pid: u32) -> Option<OwnedHandle> {
        open_process_owned(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, pid)
    }

    pub fn is_process_critical_kernel(pid: u32) -> bool {
        let Some(h) = open_process_owned(PROCESS_QUERY_LIMITED_INFORMATION, pid) else {
            return false;
        };
        let mut critical = BOOL(0);
        unsafe {
            if IsProcessCritical(h.raw(), &mut critical).is_ok() {
                critical.as_bool()
            } else {
                false
            }
        }
    }

    pub fn current_pid() -> u32 {
        unsafe { GetCurrentProcessId() }
    }

    pub fn get_exit_code(handle: HANDLE) -> Option<i32> {
        let mut code = 0u32;
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

        unsafe {
            let _ = RegCloseKey(root_key);
        }
        distros
    }

    /// Checks if `\\.\pipe\docker_engine` is ready and fetches running container metadata over HTTP/1.0.
    pub fn query_docker_containers_pipe() -> Vec<DockerContainerSummary> {
        use std::io::{Read, Write};

        let pipe_name_wide: Vec<u16> = "\\\\.\\pipe\\docker_engine\0".encode_utf16().collect();
        let ready = unsafe { WaitNamedPipeW(PCWSTR(pipe_name_wide.as_ptr()), 40) };
        if !ready.as_bool() {
            return Vec::new();
        }

        let Ok(mut pipe) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\docker_engine")
        else {
            return Vec::new();
        };

        let req = b"GET /v1.41/containers/json HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        if pipe.write_all(req).is_err() {
            return Vec::new();
        }

        let mut raw = Vec::with_capacity(8192);
        let mut chunk = [0u8; 4096];
        while raw.len() < 512 * 1024 {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }

        let text = String::from_utf8_lossy(&raw);
        let Some((_, body)) = text.split_once("\r\n\r\n") else {
            return Vec::new();
        };

        parse_docker_containers_json(body)
    }
}

#[cfg(not(target_os = "windows"))]
mod platform_imp {
    use super::*;
    use std::collections::HashMap;
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
        None
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
        #[cfg(not(target_os = "macos"))]
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

    fn stable_create_time_100ns(pid: u32, etime_100ns: i64, now_100ns: i64) -> i64 {
        static ANCHORS: OnceLock<Mutex<HashMap<u32, (i64, i64)>>> = OnceLock::new();
        let cache = ANCHORS.get_or_init(|| Mutex::new(HashMap::with_capacity(256)));
        let approx = now_100ns.saturating_sub(etime_100ns).max(1);
        let Ok(mut map) = cache.lock() else {
            return approx;
        };
        if map.len() > 2048 {
            map.clear();
        }
        if let Some(&(anchored, prev_etime)) = map.get(&pid) {
            if etime_100ns + 20_000_000 >= prev_etime && (approx - anchored).abs() <= 50_000_000 {
                map.insert(pid, (anchored, etime_100ns));
                return anchored;
            }
        }
        map.insert(pid, (approx, etime_100ns));
        approx
    }

    pub fn enumerate_processes(_scratch_buf: &mut Vec<u8>, out: &mut Vec<RawProcessEntry>) {
        out.clear();
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
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut parts = trimmed.split_whitespace();
            let Some(pid_s) = parts.next() else { continue };
            let Some(ppid_s) = parts.next() else { continue };
            let Some(rss_s) = parts.next() else { continue };
            let Some(etime_s) = parts.next() else { continue };
            let Some(cputime_s) = parts.next() else { continue };

            let Ok(pid) = pid_s.parse::<u32>() else { continue };
            let ppid = ppid_s.parse::<u32>().unwrap_or(0);
            let rss_kb = rss_s.parse::<u64>().unwrap_or(0);
            let etime_100ns = parse_ps_duration_100ns(etime_s).unwrap_or(0);
            let cpu_time_100ns = parse_ps_duration_100ns(cputime_s).unwrap_or(0);

            // Reconstruct the remainder after `cputime_s` so executable paths containing spaces stay intact.
            let comm_raw = trimmed
                .find(cputime_s)
                .map(|idx| trimmed[idx + cputime_s.len()..].trim())
                .unwrap_or("");
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
                read_bytes: 0,
                write_bytes: 0,
                image_name,
            });
        }
    }

    pub fn inspect_process_peb(pid: u32) -> PebInspection {
        let mut out = PebInspection::default();
        if pid <= 1 {
            return out;
        }
        let pid_str = pid.to_string();

        if let Ok(comm) = Command::new("ps")
            .args(["-p", &pid_str, "-o", "comm="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            let s = String::from_utf8_lossy(&comm.stdout).trim().to_string();
            if !s.is_empty() {
                out.image_path = Some(PathBuf::from(s));
            }
        }

        if let Ok(args) = Command::new("ps")
            .args(["-p", &pid_str, "-o", "args="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            let s = String::from_utf8_lossy(&args.stdout).trim().to_string();
            if !s.is_empty() {
                out.cmdline = Some(s);
            }
        }

        if let Ok(cwd) = Command::new("lsof")
            .args(["-a", "-p", &pid_str, "-d", "cwd", "-Fn"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            for line in String::from_utf8_lossy(&cwd.stdout).lines() {
                if let Some(path) = line.strip_prefix('n') {
                    let trimmed = path.trim();
                    if !trimmed.is_empty() {
                        out.working_dir = Some(PathBuf::from(trimmed));
                        break;
                    }
                }
            }
        }

        out.peb_readable = out.cmdline.is_some() || out.working_dir.is_some();
        out
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

    pub fn query_docker_containers_pipe() -> Vec<DockerContainerSummary> {
        let mut candidates = vec![PathBuf::from("/var/run/docker.sock")];
        if let Some(home) = std::env::var_os("HOME") {
            let h = PathBuf::from(home);
            candidates.push(h.join(".docker").join("run").join("docker.sock"));
            candidates.push(h.join(".orbstack").join("run").join("docker.sock"));
        }

        for sock in candidates {
            if !sock.exists() {
                continue;
            }
            let Ok(mut stream) = UnixStream::connect(&sock) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(80)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(80)));
            let req = b"GET /v1.41/containers/json HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(req).is_err() {
                continue;
            }
            let mut raw = Vec::with_capacity(8192);
            let mut chunk = [0u8; 4096];
            while raw.len() < 512 * 1024 {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => raw.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&raw);
            if let Some((_, body)) = text.split_once("\r\n\r\n") {
                return parse_docker_containers_json(body);
            }
        }
        Vec::new()
    }
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
}
