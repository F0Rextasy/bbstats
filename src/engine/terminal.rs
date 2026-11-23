//! Low-memory terminal/log tailing and bounded output capture for inspected jobs.
//!
//! Zero background memory is allocated for processes that are not actively inspected
//! or explicitly launched through `ManagedJobStore` (8 KB fixed ring buffer per launched PID).

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::io::FromRawHandle;
#[cfg(target_os = "windows")]
use windows::core::{PCWSTR, PWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{
    CloseHandle, SetHandleInformation, BOOL, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE,
};
#[cfg(target_os = "windows")]
use windows::Win32::Security::SECURITY_ATTRIBUTES;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{
    AttachConsole, FreeConsole, GetConsoleScreenBufferInfo, GetStdHandle,
    ReadConsoleOutputCharacterW, SetConsoleCtrlHandler, CONSOLE_SCREEN_BUFFER_INFO, COORD,
    STD_OUTPUT_HANDLE,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Pipes::CreatePipe;
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{
    CreateProcessW, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
};

use crate::engine::labels::LabelRule;
use crate::model::{JobId, ProjectId, TerminalOutput, TerminalSourceKind};

const MAX_TAIL_BYTES: u64 = 32 * 1024;
const MAX_TAIL_LINES: usize = 120;
const MAX_LINE_CHARS: usize = 220;
const RING_CAP_BYTES: usize = 8 * 1024;

#[derive(Default)]
struct OutputRing {
    buf: VecDeque<u8>,
}

impl OutputRing {
    fn push_bytes(&mut self, bytes: &[u8]) {
        if bytes.len() >= RING_CAP_BYTES {
            self.buf.clear();
            let start = bytes.len() - RING_CAP_BYTES;
            self.buf.extend(&bytes[start..]);
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(RING_CAP_BYTES);
        for _ in 0..overflow {
            self.buf.pop_front();
        }
        self.buf.extend(bytes);
    }

    fn tail_lines(&self, max_lines: usize) -> Vec<String> {
        let (a, b) = self.buf.as_slices();
        let mut combined = Vec::with_capacity(a.len() + b.len());
        combined.extend_from_slice(a);
        combined.extend_from_slice(b);
        extract_clean_lines(&String::from_utf8_lossy(&combined), max_lines)
    }
}

/// Holds fixed 8 KB stdout/stderr ring buffers ONLY for processes launched by bbstats.
#[derive(Default)]
pub struct ManagedJobStore {
    rings: Mutex<HashMap<u32, Arc<Mutex<OutputRing>>>>,
    exited_codes: Arc<Mutex<VecDeque<(u32, i32, Instant)>>>,
}

impl ManagedJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_exit_code(&self, pid: u32) -> Option<i32> {
        let mut q = self.exited_codes.lock().ok()?;
        let now = Instant::now();
        // Prune stale exit codes older than 30s to prevent PID-reuse cross-matching.
        while let Some((_, _, ts)) = q.front() {
            if now.saturating_duration_since(*ts) > Duration::from_secs(30) {
                q.pop_front();
            } else {
                break;
            }
        }
        let pos = q.iter().rposition(|(p, _, _)| *p == pid)?;
        q.remove(pos).map(|(_, code, _)| code)
    }

    /// Launches a command detached with optional extra environment variables.
    /// When `use_shell` is `false` on Unix, parses `argv` and executes directly without `/bin/sh -c`
    /// to prevent shell injection and preserve spaces in paths.
    #[cfg(target_os = "windows")]
    pub fn spawn_with_capture_env(
        &self,
        cmdline: &str,
        working_dir: &Path,
        extra_env: &[(String, String)],
        use_shell: bool,
    ) -> Result<u32, String> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };

        let mut read_pipe = HANDLE::default();
        let mut write_pipe = HANDLE::default();
        // SAFETY: Valid pointers to stack HANDLEs and SECURITY_ATTRIBUTES.
        unsafe {
            CreatePipe(&mut read_pipe, &mut write_pipe, Some(&sa), 4096)
                .map_err(|e| format!("CreatePipe failed: {e}"))?;
            if let Err(e) = SetHandleInformation(read_pipe, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) {
                let _ = CloseHandle(read_pipe);
                let _ = CloseHandle(write_pipe);
                return Err(format!("SetHandleInformation failed: {e}"));
            }
        }

        let nul_wide: Vec<u16> = "\\\\.\\NUL\0".encode_utf16().collect();
        // SAFETY: Valid NUL-terminated UTF-16 path and SECURITY_ATTRIBUTES.
        let nul_in = unsafe {
            CreateFileW(
                PCWSTR(nul_wide.as_ptr()),
                FILE_GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&sa),
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .unwrap_or(INVALID_HANDLE_VALUE)
        };

        let effective_cmd = if use_shell || needs_windows_cmd_wrapper(cmdline) {
            format!("cmd.exe /D /C {cmdline}")
        } else {
            cmdline.to_string()
        };
        let mut cmd_wide: Vec<u16> = effective_cmd
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let dir_wide: Vec<u16> = working_dir
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let env_block: Option<Vec<u16>> = if extra_env.is_empty() {
            None
        } else {
            Some(build_windows_env_block(extra_env))
        };
        let mut flags = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        let env_ptr = if let Some(ref block) = env_block {
            flags |= CREATE_UNICODE_ENVIRONMENT;
            Some(block.as_ptr() as *const core::ffi::c_void)
        } else {
            None
        };

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: nul_in,
            hStdOutput: write_pipe,
            hStdError: write_pipe,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();

        // SAFETY: `cmd_wide`, `dir_wide`, and optional `env_block` stay alive across the call.
        let created = unsafe {
            CreateProcessW(
                PCWSTR::null(),
                PWSTR(cmd_wide.as_mut_ptr()),
                None,
                None,
                true,
                flags,
                env_ptr,
                PCWSTR(dir_wide.as_ptr()),
                &si,
                &mut pi,
            )
        };

        // Close parent's write end and NUL stdin immediately so ReadFile returns EOF when child exits.
        // SAFETY: `write_pipe` and `nul_in` are owned handles created above.
        unsafe {
            let _ = CloseHandle(write_pipe);
            if nul_in != INVALID_HANDLE_VALUE && !nul_in.is_invalid() {
                let _ = CloseHandle(nul_in);
            }
        }

        if let Err(err) = created {
            // SAFETY: `read_pipe` is an owned handle created above.
            unsafe {
                let _ = CloseHandle(read_pipe);
            }
            return Err(format!("CreateProcessW failed: {err}"));
        }

        let pid = pi.dwProcessId;
        // SAFETY: Process/thread handles returned by CreateProcessW are owned.
        unsafe {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }

        let ring = self.register_ring(pid);
        let pipe_raw = read_pipe.0 as isize;
        let spawned = thread::Builder::new()
            .name(format!("bbstats-pipe-{pid}"))
            .spawn(move || {
                // SAFETY: `pipe_raw` transfers exclusive ownership of `read_pipe` to `File`.
                let mut pipe_file = unsafe { File::from_raw_handle(pipe_raw as *mut core::ffi::c_void) };
                let mut chunk = [0u8; 1024];
                loop {
                    match pipe_file.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read_n) => {
                            if let Ok(mut g) = ring.lock() {
                                g.push_bytes(&chunk[..read_n]);
                            }
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            if let Ok(mut map) = self.rings.lock() {
                map.remove(&pid);
            }
            // SAFETY: Thread spawn failed, so `read_pipe` ownership was not transferred.
            unsafe {
                let _ = CloseHandle(read_pipe);
            }
            return Err(format!("pipe reader thread failed: {err}"));
        }

        Ok(pid)
    }

    /// Launches a POSIX command detached with stdout/stderr streamed into an 8 KB ring buffer.
    #[cfg(not(target_os = "windows"))]
    pub fn spawn_with_capture_env(
        &self,
        cmdline: &str,
        working_dir: &Path,
        extra_env: &[(String, String)],
        use_shell: bool,
    ) -> Result<u32, String> {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};

        let mut cmd = if use_shell {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(cmdline);
            c
        } else {
            let argv = crate::engine::project_resolver::split_cmdline_args(cmdline);
            if argv.is_empty() {
                return Err("empty command".to_string());
            }
            let mut c = Command::new(&argv[0]);
            if argv.len() > 1 {
                c.args(&argv[1..]);
            }
            c
        };
        cmd.current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

        let pid = child.id();
        let ring = self.register_ring(pid);

        if let Some(mut stdout) = child.stdout.take() {
            let r = Arc::clone(&ring);
            let _ = thread::Builder::new()
                .name(format!("bbstats-out-{pid}"))
                .spawn(move || {
                    let mut chunk = [0u8; 1024];
                    while let Ok(n) = stdout.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        if let Ok(mut g) = r.lock() {
                            g.push_bytes(&chunk[..n]);
                        }
                    }
                });
        }
        if let Some(mut stderr) = child.stderr.take() {
            let r = Arc::clone(&ring);
            let _ = thread::Builder::new()
                .name(format!("bbstats-err-{pid}"))
                .spawn(move || {
                    let mut chunk = [0u8; 1024];
                    while let Ok(n) = stderr.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        if let Ok(mut g) = r.lock() {
                            g.push_bytes(&chunk[..n]);
                        }
                    }
                });
        }
        let exited = Arc::clone(&self.exited_codes);
        let _ = thread::Builder::new()
            .name(format!("bbstats-wait-{pid}"))
            .spawn(move || {
                if let Ok(status) = child.wait() {
                    let code = status
                        .code()
                        .or_else(|| status.signal().map(|s| 128 + s))
                        .unwrap_or(1);
                    if let Ok(mut q) = exited.lock() {
                        if q.len() >= 64 {
                            q.pop_front();
                        }
                        q.push_back((pid, code, Instant::now()));
                    }
                }
            });

        Ok(pid)
    }

    fn register_ring(&self, pid: u32) -> Arc<Mutex<OutputRing>> {
        let ring = Arc::new(Mutex::new(OutputRing::default()));
        if let Ok(mut map) = self.rings.lock() {
            // Keep map bounded if many short processes were launched over days.
            if map.len() > 24 {
                map.retain(|_, r| Arc::strong_count(r) > 1);
            }
            map.insert(pid, Arc::clone(&ring));
        }
        ring
    }

    pub fn read_lines_for_pids(&self, pids: &[u32], max_lines: usize) -> Option<(u32, Vec<String>)> {
        let map = self.rings.lock().ok()?;
        for &pid in pids {
            if let Some(ring) = map.get(&pid) {
                if let Ok(g) = ring.lock() {
                    let lines = g.tail_lines(max_lines);
                    if !lines.is_empty() {
                        return Some((pid, lines));
                    }
                }
            }
        }
        None
    }
}

#[cfg(target_os = "windows")]
fn needs_windows_cmd_wrapper(cmdline: &str) -> bool {
    let args = crate::engine::project_resolver::split_cmdline_args(cmdline);
    let Some(first) = args.first() else {
        return false;
    };
    let low = first.to_ascii_lowercase();
    let leaf = low.rsplit(['/', '\\']).next().unwrap_or(&low);
    if leaf == "cmd" || leaf == "cmd.exe" {
        return false;
    }
    leaf.ends_with(".cmd")
        || leaf.ends_with(".bat")
        || matches!(leaf, "npm" | "npx" | "pnpm" | "pnpx" | "yarn" | "yarnpkg" | "corepack")
}

#[cfg(target_os = "windows")]
fn build_windows_env_block(extra_env: &[(String, String)]) -> Vec<u16> {
    let mut merged: std::collections::BTreeMap<String, (String, String)> =
        std::collections::BTreeMap::new();
    for (k, v) in std::env::vars() {
        merged.insert(k.to_ascii_uppercase(), (k, v));
    }
    for (k, v) in extra_env {
        if !k.is_empty() && !k.contains('=') && !k.contains('\0') && !v.contains('\0') {
            merged.insert(k.to_ascii_uppercase(), (k.clone(), v.clone()));
        }
    }
    let mut block: Vec<u16> = Vec::with_capacity(4096);
    for (_, (k, v)) in merged {
        block.extend(k.encode_utf16());
        block.push(b'=' as u16);
        block.extend(v.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// Resolves terminal output lazily ONLY for the active inspected project/job.
pub fn inspect_terminal_output(
    project_id: &ProjectId,
    job_id: Option<&JobId>,
    pids: &[u32],
    log_path: Option<&Path>,
    labels: &[LabelRule],
    managed: &ManagedJobStore,
) -> TerminalOutput {
    if !pids.is_empty() {
        if let Some((matched_pid, lines)) = managed.read_lines_for_pids(pids, MAX_TAIL_LINES) {
            return TerminalOutput {
                project_id: project_id.clone(),
                job_id: job_id.cloned(),
                source_kind: TerminalSourceKind::ManagedPipe,
                source_hint: Some(format!("PID {matched_pid}")),
                lines,
            };
        }
    }

    let file_candidate = log_path.and_then(|path| {
        let lines = read_log_tail_lines(path, MAX_TAIL_BYTES, MAX_TAIL_LINES);
        if lines.is_empty() {
            None
        } else {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            Some((path, lines, mtime))
        }
    });

    // Also check state JSON `logs` arrays if a rule defines `pid_file` (prefer it when newer than `log_path`).
    let state_candidate = job_id.and_then(|jid| {
        let j_low = jid.0.to_ascii_lowercase();
        for rule in labels {
            let needle = rule.match_substr.trim().to_ascii_lowercase();
            if needle.is_empty() || !j_low.contains(&needle) {
                continue;
            }
            if let Some(ref pf) = rule.pid_file {
                let p = Path::new(&crate::engine::labels::expand_win_env_vars(pf)).to_path_buf();
                if let Some(lines) = read_state_json_logs(&p, MAX_TAIL_LINES) {
                    let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                    return Some((p, lines, mtime));
                }
            }
        }
        None
    });

    match (file_candidate, state_candidate) {
        (Some((f_path, f_lines, f_mtime)), Some((s_path, s_lines, s_mtime))) => {
            let prefer_state = match (s_mtime, f_mtime) {
                (Some(sm), Some(fm)) => sm > fm,
                (Some(_), None) => true,
                _ => false,
            };
            if prefer_state {
                return TerminalOutput {
                    project_id: project_id.clone(),
                    job_id: job_id.cloned(),
                    source_kind: TerminalSourceKind::StateJson,
                    source_hint: s_path.file_name().and_then(|s| s.to_str()).map(str::to_string),
                    lines: s_lines,
                };
            }
            return TerminalOutput {
                project_id: project_id.clone(),
                job_id: job_id.cloned(),
                source_kind: TerminalSourceKind::LogFile,
                source_hint: f_path.file_name().and_then(|s| s.to_str()).map(str::to_string),
                lines: f_lines,
            };
        }
        (Some((f_path, f_lines, _)), None) => {
            return TerminalOutput {
                project_id: project_id.clone(),
                job_id: job_id.cloned(),
                source_kind: TerminalSourceKind::LogFile,
                source_hint: f_path.file_name().and_then(|s| s.to_str()).map(str::to_string),
                lines: f_lines,
            };
        }
        (None, Some((s_path, s_lines, _))) => {
            return TerminalOutput {
                project_id: project_id.clone(),
                job_id: job_id.cloned(),
                source_kind: TerminalSourceKind::StateJson,
                source_hint: s_path.file_name().and_then(|s| s.to_str()).map(str::to_string),
                lines: s_lines,
            };
        }
        (None, None) => {}
    }

    for &pid in pids {
        if pid <= 4 {
            continue;
        }
        if let Some(lines) = read_console_buffer_lines(pid, MAX_TAIL_LINES) {
            if !lines.is_empty() {
                return TerminalOutput {
                    project_id: project_id.clone(),
                    job_id: job_id.cloned(),
                    source_kind: TerminalSourceKind::ConsoleBuffer,
                    source_hint: Some(format!("conhost PID {pid}")),
                    lines,
                };
            }
        }
    }

    TerminalOutput {
        project_id: project_id.clone(),
        job_id: job_id.cloned(),
        source_kind: TerminalSourceKind::None,
        source_hint: log_path
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string),
        lines: Vec::new(),
    }
}

/// Reads at most `max_bytes` from the end of `path` and returns the last `max_lines` cleaned lines.
pub fn read_log_tail_lines(path: &Path, max_bytes: u64, max_lines: usize) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    let len = meta.len();
    if len == 0 {
        return Vec::new();
    }
    let start = len.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity((len - start).min(max_bytes) as usize);
    if file.take(max_bytes).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let aligned = if start > 0 {
        text.find('\n').map(|idx| &text[idx + 1..]).unwrap_or(text.as_ref())
    } else {
        text.as_ref()
    };
    extract_clean_lines(aligned, max_lines)
}

fn read_state_json_logs(path: &Path, max_lines: usize) -> Option<Vec<String>> {
    const MAX_JSON_BYTES: u64 = 512 * 1024;
    let file = File::open(path).ok()?;
    let mut raw = Vec::new();
    file.take(MAX_JSON_BYTES).read_to_end(&mut raw).ok()?;
    let val: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let arr = val.get("logs")?.as_array()?;

    let mut out = Vec::with_capacity(arr.len().min(max_lines));
    for item in arr {
        let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        let cleaned = strip_ansi(text.trim());
        if cleaned.is_empty() {
            continue;
        }
        let ts = item
            .get("t")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !cleaned.starts_with(s));
        let formatted = match ts {
            Some(t) => format!("[{t}] {cleaned}"),
            None => cleaned,
        };
        out.push(formatted.chars().take(MAX_LINE_CHARS).collect());
    }
    if out.is_empty() {
        None
    } else {
        if out.len() > max_lines {
            let drop_n = out.len() - max_lines;
            out.drain(..drop_n);
        }
        Some(out)
    }
}

#[cfg(target_os = "windows")]
static CONSOLE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(target_os = "windows")]
// SAFETY: Best-effort attachment to parent console before CLI stdout/stderr output.
unsafe extern "system" fn ignore_console_ctrl(_ctrl_type: u32) -> BOOL {
    BOOL(1)
}

/// Briefly attaches to `pid`'s classic conhost console on Windows, reads recent rows, and detaches.
#[cfg(target_os = "windows")]
fn read_console_buffer_lines(pid: u32, max_lines: usize) -> Option<Vec<String>> {
    let _guard = CONSOLE_LOCK.lock().ok()?;
    // SAFETY: Serialized by `CONSOLE_LOCK`; Ctrl handler is installed only for the brief attach window.
    unsafe {
        if AttachConsole(pid).is_err() {
            return None;
        }
        let _ = SetConsoleCtrlHandler(Some(ignore_console_ctrl), true);
        let result = (|| {
            let h_out = GetStdHandle(STD_OUTPUT_HANDLE).ok()?;
            if h_out == INVALID_HANDLE_VALUE || h_out.0.is_null() {
                return None;
            }
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            if GetConsoleScreenBufferInfo(h_out, &mut info).is_err() {
                return None;
            }
            let width = info.dwSize.X.clamp(1, 300) as usize;
            let cursor_y = info.dwCursorPosition.Y.max(0) as i32;
            let rows_to_read = (max_lines as i32 * 2).min(cursor_y + 1).max(1);
            let start_y = (cursor_y + 1 - rows_to_read).max(0);

            let mut lines = Vec::new();
            let mut row_buf = vec![0u16; width];
            for row in start_y..=cursor_y {
                let mut chars_read = 0u32;
                let ok = ReadConsoleOutputCharacterW(
                    h_out,
                    &mut row_buf,
                    COORD { X: 0, Y: row as i16 },
                    &mut chars_read,
                );
                if ok.is_ok() && chars_read > 0 {
                    let s = String::from_utf16_lossy(&row_buf[..chars_read as usize]);
                    let trimmed = s.trim_end();
                    if !trimmed.is_empty() {
                        lines.push(trimmed.chars().take(MAX_LINE_CHARS).collect::<String>());
                    }
                }
            }
            if lines.len() > max_lines {
                let drop_n = lines.len() - max_lines;
                lines.drain(..drop_n);
            }
            Some(lines)
        })();
        let _ = FreeConsole();
        let _ = SetConsoleCtrlHandler(Some(ignore_console_ctrl), false);
        result
    }
}

#[cfg(not(target_os = "windows"))]
fn read_console_buffer_lines(_pid: u32, _max_lines: usize) -> Option<Vec<String>> {
    None
}

fn extract_clean_lines(raw: &str, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = raw
        .lines()
        .map(strip_ansi)
        .map(|s| s.trim_end().to_string())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(MAX_LINE_CHARS).collect())
        .collect();
    if lines.len() > max_lines {
        let drop_n = lines.len() - max_lines;
        lines.drain(..drop_n);
    }
    lines
}

/// Strips ANSI CSI (`\x1b[...m`) and OSC escape sequences for clean UI rendering.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.peek() == Some(&'\\')) {
                            if c == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some('(' | ')' | '*' | '+') => {
                    chars.next();
                    let _ = chars.next();
                }
                Some('=' | '>' | '7' | '8' | 'c' | 'M') => {
                    chars.next();
                }
                _ => {}
            }
        } else if !ch.is_control() || ch == '\t' {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_sgr_and_osc_codes() {
        let raw = "\x1b[32mINFO\x1b[0m server listening on \x1b[1;34m:3000\x1b[0m";
        assert_eq!(strip_ansi(raw), "INFO server listening on :3000");
    }

    #[test]
    fn ring_buffer_caps_memory_at_8kb() {
        let mut ring = OutputRing::default();
        let line = "0123456789abcdef\n".repeat(700); // > 11 KB
        ring.push_bytes(line.as_bytes());
        assert!(ring.buf.len() <= RING_CAP_BYTES);
        let lines = ring.tail_lines(10);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "0123456789abcdef");
    }

    #[test]
    fn read_log_tail_lines_reads_only_last_lines_and_strips_ansi() {
        let p = std::env::temp_dir().join(format!("bbstats_tail_test_{}.log", std::process::id()));
        let mut content = String::new();
        for i in 1..=100 {
            content.push_str(&format!("\x1b[36mline {i}\x1b[0m\n"));
        }
        std::fs::write(&p, content).unwrap();
        let tail = read_log_tail_lines(&p, 16 * 1024, 5);
        assert_eq!(
            tail,
            vec!["line 96", "line 97", "line 98", "line 99", "line 100"]
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn inspect_terminal_prefers_newer_state_json_over_stale_log_file() {
        let base = std::env::temp_dir().join(format!("bbstats_term_fresh_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let log_path = base.join("auto-console.log");
        let state_path = base.join("auto-state.json");
        std::fs::write(&log_path, "old line 1\nold line 2\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &state_path,
            r#"{"pid":123,"logs":[{"t":"03:07:19","level":"ok","text":"Worker heartbeat pass"}]}"#,
        )
        .unwrap();

        let rules = vec![LabelRule {
            match_substr: "auto_updater".to_string(),
            project: Some("WorkerBot".to_string()),
            job: "Auto update botu".to_string(),
            pid_file: Some(state_path.to_string_lossy().into_owned()),
            log_file: Some(log_path.to_string_lossy().into_owned()),
            ..Default::default()
        }];
        let managed = ManagedJobStore::new();
        let out = inspect_terminal_output(
            &ProjectId("p1".to_string()),
            Some(&JobId("p1::state:auto_updater".to_string())),
            &[],
            Some(&log_path),
            &rules,
            &managed,
        );

        assert_eq!(out.source_kind, TerminalSourceKind::StateJson);
        assert_eq!(out.lines, vec!["[03:07:19] Worker heartbeat pass"]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
