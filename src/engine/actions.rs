//! Safe process-tree termination, restart execution, critical-process blacklist,
//! and kernel event-driven exit/crash watching (`RegisterWaitForSingleObject` on Windows).

use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(target_os = "windows")]
use std::ffi::c_void;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, BOOLEAN, HANDLE, INVALID_HANDLE_VALUE, LPARAM, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{
    RegisterWaitForSingleObject, TerminateProcess, UnregisterWaitEx, INFINITE, WT_EXECUTEONLYONCE,
    WT_EXECUTELONGFUNCTION,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};

use crate::engine::collector::CollectedProcess;
use crate::engine::win_nt::{current_pid, is_process_critical_kernel};
#[cfg(target_os = "windows")]
use crate::engine::win_nt::{
    get_exit_code, is_process_alive_handle, open_process_for_terminate, open_process_for_wait,
    OwnedHandle,
};
#[cfg(not(target_os = "windows"))]
use crate::engine::win_nt::verify_unix_process_key;
use crate::model::ProcessKey;

/// Hardcoded critical OS/session processes that must NEVER be terminated by bbstats.
const CRITICAL_PROCESS_BLACKLIST: &[&str] = &[
    // Windows
    "system",
    "system idle process",
    "idle",
    "registry",
    "memcompression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "winlogon.exe",
    "dwm.exe",
    "svchost.exe",
    "explorer.exe",
    "sihost.exe",
    "fontdrvhost.exe",
    "audiodg.exe",
    "taskhostw.exe",
    "wudfhost.exe",
    "securityhealthservice.exe",
    "msmpeng.exe",
    "vmmemwsl",
    "vmmem",
    "vmmem.exe",
    // macOS / Unix / Linux
    "launchd",
    "kernel_task",
    "windowserver",
    "loginwindow",
    "sysmond",
    "usereventagent",
    "systemd",
    "init",
    "sshd",
    "dbus-daemon",
    "systemd-journald",
    "systemd-udevd",
    "xorg",
    "wayland",
    "bbstats",
    "bbstats.exe",
];

#[inline]
pub fn is_hard_blacklisted_name(lower_image_name: &str) -> bool {
    CRITICAL_PROCESS_BLACKLIST.contains(&lower_image_name)
}

/// Checks whether a process is safe to terminate (`true` = allowed, `false` = blocked).
pub fn is_kill_allowed(
    pid: u32,
    session_id: u32,
    image_name: &str,
    self_pid: u32,
    protected_ancestors: &HashSet<u32>,
) -> bool {
    if pid <= 4 || pid == self_pid || protected_ancestors.contains(&pid) {
        return false;
    }
    if session_id == 0 {
        return false;
    }
    let lower = image_name.to_ascii_lowercase();
    if is_hard_blacklisted_name(&lower) {
        return false;
    }
    !is_process_critical_kernel(pid)
}

/// Pure version of safety check for unit testing without OS handles.
pub fn is_kill_allowed_static(
    pid: u32,
    session_id: u32,
    image_name: &str,
    self_pid: u32,
    protected_ancestors: &HashSet<u32>,
) -> bool {
    if pid <= 4 || pid == self_pid || protected_ancestors.contains(&pid) {
        return false;
    }
    if session_id == 0 {
        return false;
    }
    !is_hard_blacklisted_name(&image_name.to_ascii_lowercase())
}

/// Computes ancestor PIDs of the current process so the widget never kills its own parent chain.
pub fn compute_self_ancestors(procs: &[CollectedProcess], self_pid: u32) -> HashSet<u32> {
    let by_pid: HashMap<u32, &CollectedProcess> = procs.iter().map(|p| (p.key.pid, p)).collect();
    let mut ancestors = HashSet::new();
    let mut cur_pid = self_pid;
    for _ in 0..64 {
        let Some(p) = by_pid.get(&cur_pid) else {
            break;
        };
        if p.parent_pid == 0 || p.parent_pid == cur_pid || ancestors.contains(&p.parent_pid) {
            break;
        }
        if let Some(parent) = by_pid.get(&p.parent_pid) {
            if parent.key.create_time_100ns <= p.key.create_time_100ns {
                ancestors.insert(parent.key.pid);
                cur_pid = parent.key.pid;
                continue;
            }
        }
        break;
    }
    ancestors
}

/// Collects `target` and all valid descendants in **bottom-up** (leaves first, root last) order.
pub fn build_kill_tree_order(
    target: ProcessKey,
    procs: &[CollectedProcess],
    self_pid: u32,
    protected_ancestors: &HashSet<u32>,
    include_tree: bool,
) -> Vec<ProcessKey> {
    let by_pid: HashMap<u32, &CollectedProcess> = procs.iter().map(|p| (p.key.pid, p)).collect();
    let Some(root) = by_pid.get(&target.pid).copied() else {
        return Vec::new();
    };
    if root.key != target
        || !is_kill_allowed_static(
            root.key.pid,
            root.session_id,
            &root.image_name,
            self_pid,
            protected_ancestors,
        )
    {
        return Vec::new();
    }

    if !include_tree {
        return vec![target];
    }

    let mut children_map: HashMap<u32, Vec<&CollectedProcess>> = HashMap::new();
    for p in procs {
        if p.parent_pid != 0 && p.parent_pid != p.key.pid {
            children_map.entry(p.parent_pid).or_default().push(p);
        }
    }

    let mut bfs_order = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root);
    visited.insert(root.key);

    while let Some(node) = queue.pop_front() {
        bfs_order.push(node.key);
        if let Some(children) = children_map.get(&node.key.pid) {
            for child in children {
                if child.key.create_time_100ns >= node.key.create_time_100ns
                    && !visited.contains(&child.key)
                    && is_kill_allowed_static(
                        child.key.pid,
                        child.session_id,
                        &child.image_name,
                        self_pid,
                        protected_ancestors,
                    )
                {
                    visited.insert(child.key);
                    queue.push_back(child);
                }
            }
        }
    }

    // Reverse BFS so leaves are terminated before their parent processes.
    bfs_order.reverse();
    bfs_order
}

/// Safely terminates a batch of target processes (and optionally their subtrees) with at most
/// one 250ms `WM_CLOSE` grace wait across the entire batch.
pub fn execute_safe_kill_many(
    roots: &[ProcessKey],
    procs: &[CollectedProcess],
    include_tree: bool,
    user_kills: &mut HashMap<ProcessKey, Instant>,
) -> Vec<ProcessKey> {
    if roots.is_empty() {
        return Vec::new();
    }
    let self_pid = current_pid();
    let ancestors = compute_self_ancestors(procs, self_pid);
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for &root in roots {
        for key in build_kill_tree_order(root, procs, self_pid, &ancestors, include_tree) {
            if seen.insert(key) {
                targets.push(key);
            }
        }
    }
    if targets.is_empty() {
        return Vec::new();
    }

    let by_key: HashMap<ProcessKey, &CollectedProcess> = procs.iter().map(|p| (p.key, p)).collect();
    let now = Instant::now();
    let mut actually_killed: Vec<ProcessKey> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // First ask any GUI top-level windows to close cleanly (only after verifying ProcessKey).
        let mut closed_requested: Vec<ProcessKey> = Vec::new();
        for key in &targets {
            let Some(proc_info) = by_key.get(key) else {
                continue;
            };
            if !is_kill_allowed(
                key.pid,
                proc_info.session_id,
                &proc_info.image_name,
                self_pid,
                &ancestors,
            ) {
                continue;
            }
            if open_process_for_wait(*key).is_some() && send_wm_close_to_pid(key.pid) {
                closed_requested.push(*key);
            }
        }
        if !closed_requested.is_empty() {
            std::thread::sleep(Duration::from_millis(250));
        }

        // Force-terminate any remaining targets bottom-up after re-checking identity & kernel criticality.
        for key in &targets {
            let Some(proc_info) = by_key.get(key) else {
                continue;
            };
            if !is_kill_allowed(
                key.pid,
                proc_info.session_id,
                &proc_info.image_name,
                self_pid,
                &ancestors,
            ) {
                continue;
            }
            if let Some(h) = open_process_for_terminate(*key) {
                // If process already exited cleanly from WM_CLOSE, skip TerminateProcess.
                if is_process_alive_handle(h.raw()) {
                    // SAFETY: `h` is a valid PROCESS_TERMINATE handle verified against `key.create_time_100ns`.
                    let ok = unsafe { TerminateProcess(h.raw(), 1) }.is_ok();
                    if ok {
                        actually_killed.push(*key);
                    }
                } else if closed_requested.contains(key) {
                    actually_killed.push(*key);
                }
            } else if closed_requested.contains(key) && open_process_for_wait(*key).is_none() {
                actually_killed.push(*key);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Send graceful SIGTERM first across verified targets.
        let mut sent_term: Vec<ProcessKey> = Vec::new();
        for key in &targets {
            if let Some(proc_info) = by_key.get(key) {
                if is_kill_allowed(
                    key.pid,
                    proc_info.session_id,
                    &proc_info.image_name,
                    self_pid,
                    &ancestors,
                ) && verify_unix_process_key(*key)
                    // SAFETY: Sends a process signal after checking PID/identity and blacklist guards.
                    && unsafe { libc::kill(key.pid as libc::pid_t, libc::SIGTERM) } == 0
                {
                    sent_term.push(*key);
                }
            }
        }
        if !sent_term.is_empty() {
            std::thread::sleep(Duration::from_millis(250));
        }
        // Follow up with SIGKILL only after re-verifying process identity against PID reuse.
        for key in &targets {
            if let Some(proc_info) = by_key.get(key) {
                if !is_kill_allowed(
                    key.pid,
                    proc_info.session_id,
                    &proc_info.image_name,
                    self_pid,
                    &ancestors,
                ) {
                    continue;
                }
                if verify_unix_process_key(*key)
                    // SAFETY: Sends a process signal after checking PID/identity and blacklist guards.
                    && unsafe { libc::kill(key.pid as libc::pid_t, 0) } == 0
                {
                    // SAFETY: Sends a process signal after checking PID/identity and blacklist guards.
                    if unsafe { libc::kill(key.pid as libc::pid_t, libc::SIGKILL) } == 0 {
                        actually_killed.push(*key);
                    }
                } else if sent_term.contains(key) {
                    actually_killed.push(*key);
                }
            }
        }
    }

    for key in &actually_killed {
        user_kills.insert(*key, now);
    }
    actually_killed
}

#[cfg(target_os = "windows")]
fn send_wm_close_to_pid(target_pid: u32) -> bool {
    struct EnumCtx {
        pid: u32,
        sent: bool,
    }

    // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
    unsafe extern "system" fn enum_proc(
        hwnd: windows::Win32::Foundation::HWND,
        lparam: LPARAM,
    ) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        let mut win_pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut win_pid));
        if win_pid == ctx.pid
            && IsWindowVisible(hwnd).as_bool()
            && PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
        {
            ctx.sent = true;
        }
        BOOL(1)
    }

    let mut ctx = EnumCtx {
        pid: target_pid,
        sent: false,
    };
    // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
    }
    ctx.sent
}

/// Returns true if the exit code represents a normal/intentional exit rather than a crash.
#[inline]
pub fn is_normal_exit_code(exit_code: i32) -> bool {
    exit_code == 0
        || exit_code == (0xC000_013A_u32 as i32) // STATUS_CONTROL_C_EXIT
        || exit_code == 130 // SIGINT
        || exit_code == 143 // SIGTERM
}

#[derive(Debug, Clone, Copy)]
pub struct ExitEvent {
    pub key: ProcessKey,
    pub exit_code: i32,
}

pub type ExitCallback = Arc<dyn Fn(ExitEvent) + Send + Sync>;

#[cfg(target_os = "windows")]
mod watcher_imp {
    use super::*;

    struct WaitContext {
        key: ProcessKey,
        proc_handle: HANDLE,
        on_exit: ExitCallback,
    }

    struct WatchedEntry {
        wait_handle: HANDLE,
        _proc_handle: OwnedHandle,
        ctx_ptr: *mut WaitContext,
    }

    /// Event-driven exit watcher backed by Windows Thread Pool `RegisterWaitForSingleObject`.
    pub struct ExitWatcher {
        entries: HashMap<ProcessKey, WatchedEntry>,
        on_exit: ExitCallback,
    }

    // WaitContext pointers are only freed after `UnregisterWaitEx(wait_handle, INVALID_HANDLE_VALUE)`.
    unsafe impl Send for ExitWatcher {}

    impl ExitWatcher {
        pub fn new(on_exit: ExitCallback) -> Self {
            Self {
                entries: HashMap::with_capacity(64),
                on_exit,
            }
        }

        /// Registers a passive wait on `key` after verifying process creation time.
        pub fn ensure_watched(&mut self, key: ProcessKey) {
            if self.entries.contains_key(&key) || key.pid <= 4 {
                return;
            }
            let Some(proc_handle) = open_process_for_wait(key) else {
                return;
            };

            let ctx_box = Box::new(WaitContext {
                key,
                proc_handle: proc_handle.raw(),
                on_exit: Arc::clone(&self.on_exit),
            });
            let ctx_ptr = Box::into_raw(ctx_box);

            let mut wait_handle = HANDLE::default();
            // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
            let res = unsafe {
                RegisterWaitForSingleObject(
                    &mut wait_handle,
                    proc_handle.raw(),
                    Some(wait_callback),
                    Some(ctx_ptr as *const c_void),
                    INFINITE,
                    WT_EXECUTEONLYONCE | WT_EXECUTELONGFUNCTION,
                )
            };

            if res.is_err() || wait_handle.is_invalid() {
                // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
                unsafe {
                    drop(Box::from_raw(ctx_ptr));
                }
                return;
            }

            self.entries.insert(
                key,
                WatchedEntry {
                    wait_handle,
                    _proc_handle: proc_handle,
                    ctx_ptr,
                },
            );
        }

        /// Queries an already-watched handle synchronously if the process disappeared before
        /// the thread-pool `Exited` message was drained.
        pub fn poll_exit_code(&self, key: &ProcessKey) -> Option<i32> {
            let entry = self.entries.get(key)?;
            if is_process_alive_handle(entry._proc_handle.raw()) {
                None
            } else {
                Some(get_exit_code(entry._proc_handle.raw()).unwrap_or(0))
            }
        }

        /// Unregisters and frees a watched entry once its exit event has been processed or the process is ignored.
        pub fn unwatch(&mut self, key: &ProcessKey) {
            if let Some(entry) = self.entries.remove(key) {
                // SAFETY: Box-allocated WaitContext remains valid until UnregisterWaitEx completes or leaks intentionally on timeout.
                unsafe {
                    let _ = UnregisterWaitEx(entry.wait_handle, INVALID_HANDLE_VALUE);
                    drop(Box::from_raw(entry.ctx_ptr));
                }
            }
        }
    }

    impl Drop for ExitWatcher {
        fn drop(&mut self) {
            let keys: Vec<ProcessKey> = self.entries.keys().copied().collect();
            for key in keys {
                self.unwatch(&key);
            }
        }
    }

    // SAFETY: Called by Win32 with matching system ABI; pointer arguments are checked before dereference.
    unsafe extern "system" fn wait_callback(parameter: *mut c_void, timer_or_wait_fired: BOOLEAN) {
        if parameter.is_null() || timer_or_wait_fired.as_bool() {
            return;
        }
        let ctx = &*(parameter as *const WaitContext);
        let exit_code = get_exit_code(ctx.proc_handle).unwrap_or(0);
        (ctx.on_exit)(ExitEvent {
            key: ctx.key,
            exit_code,
        });
    }
}

#[cfg(target_os = "macos")]
mod watcher_imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    fn decode_wait_status(wstatus: i32) -> i32 {
        let sig = wstatus & 0x7f;
        if sig == 0 {
            (wstatus >> 8) & 0xff
        } else if sig != 0x7f {
            128 + sig
        } else {
            1
        }
    }

    /// Event-driven exit watcher backed by macOS `kqueue` (`EVFILT_PROC` + `NOTE_EXITSTATUS`).
    pub struct ExitWatcher {
        kq: i32,
        watched: Arc<Mutex<HashMap<u32, ProcessKey>>>,
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl ExitWatcher {
        pub fn new(on_exit: ExitCallback) -> Self {
            // SAFETY: Valid kqueue file descriptor and stack-allocated kevent buffers.
            let kq = unsafe { libc::kqueue() };
            let watched = Arc::new(Mutex::new(HashMap::with_capacity(64)));
            let stop = Arc::new(AtomicBool::new(false));

            let join = if kq >= 0 {
                let w_bg = Arc::clone(&watched);
                let s_bg = Arc::clone(&stop);
                std::thread::Builder::new()
                    .name("bbstats-kqueue".to_string())
                    .spawn(move || {
                        // SAFETY: Valid kqueue file descriptor and stack-allocated kevent buffers.
                        let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
                        let timeout = libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 200_000_000,
                        };
                        while !s_bg.load(Ordering::Acquire) {
                            // SAFETY: Valid kqueue file descriptor and stack-allocated kevent buffers.
                            let n = unsafe {
                                libc::kevent(
                                    kq,
                                    std::ptr::null(),
                                    0,
                                    events.as_mut_ptr(),
                                    events.len() as i32,
                                    &timeout,
                                )
                            };
                            if n <= 0 {
                                continue;
                            }
                            for ev in &events[..n as usize] {
                                let pid = ev.ident as u32;
                                let key_opt = w_bg.lock().ok().and_then(|mut m| m.remove(&pid));
                                if let Some(key) = key_opt {
                                    let exit_code = decode_wait_status(ev.data as i32);
                                    on_exit(ExitEvent { key, exit_code });
                                }
                            }
                        }
                    })
                    .ok()
            } else {
                None
            };

            Self {
                kq,
                watched,
                stop,
                join,
            }
        }

        pub fn ensure_watched(&mut self, key: ProcessKey) {
            if self.kq < 0 || key.pid <= 1 || !crate::engine::win_nt::verify_unix_process_key(key) {
                return;
            }
            if let Ok(mut map) = self.watched.lock() {
                if map.get(&key.pid) == Some(&key) {
                    return;
                }
                map.insert(key.pid, key);
            } else {
                return;
            }
            let kev = libc::kevent {
                ident: key.pid as libc::uintptr_t,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT | libc::NOTE_EXITSTATUS,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: Valid kqueue file descriptor and stack-allocated kevent buffers.
            let rc = unsafe { libc::kevent(self.kq, &kev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if rc < 0 {
                if let Ok(mut map) = self.watched.lock() {
                    if map.get(&key.pid) == Some(&key) {
                        map.remove(&key.pid);
                    }
                }
            }
        }

        pub fn poll_exit_code(&self, _key: &ProcessKey) -> Option<i32> {
            None
        }

        pub fn unwatch(&mut self, key: &ProcessKey) {
            if let Ok(mut map) = self.watched.lock() {
                if map.get(&key.pid) == Some(key) {
                    map.remove(&key.pid);
                }
            }
            if self.kq >= 0 && key.pid > 1 {
                let kev = libc::kevent {
                    ident: key.pid as libc::uintptr_t,
                    filter: libc::EVFILT_PROC,
                    flags: libc::EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                };
                // SAFETY: Valid kqueue file descriptor and stack-allocated kevent buffers.
                unsafe {
                    let _ = libc::kevent(self.kq, &kev, 1, std::ptr::null_mut(), 0, std::ptr::null());
                }
            }
        }
    }

    impl Drop for ExitWatcher {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
            if self.kq >= 0 {
                // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
                unsafe {
                    libc::close(self.kq);
                }
            }
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
mod watcher_imp {
    use super::*;

    /// Lightweight non-Windows/non-macOS `ExitWatcher` fallback.
    pub struct ExitWatcher {
        _on_exit: ExitCallback,
    }

    impl ExitWatcher {
        pub fn new(on_exit: ExitCallback) -> Self {
            Self { _on_exit: on_exit }
        }

        pub fn ensure_watched(&mut self, _key: ProcessKey) {}

        pub fn poll_exit_code(&self, _key: &ProcessKey) -> Option<i32> {
            None
        }

        pub fn unwatch(&mut self, _key: &ProcessKey) {}
    }
}

pub use watcher_imp::ExitWatcher;

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_proc(pid: u32, parent: u32, ctime: i64, name: &str, session: u32) -> CollectedProcess {
        CollectedProcess {
            key: ProcessKey::new(pid, ctime),
            parent_pid: parent,
            session_id: session,
            image_name: name.to_string(),
            cpu_percent: 0.0,
            private_ram_bytes: 0,
            uptime_secs: 10,
        }
    }

    #[test]
    fn blacklist_protects_critical_system_self_and_session0() {
        let self_pid = 4242;
        let ancestors = HashSet::from([1000]);

        for critical in [
            "System",
            "csrss.exe",
            "lsass.exe",
            "dwm.exe",
            "svchost.exe",
            "explorer.exe",
            "vmmemWSL",
        ] {
            assert!(
                !is_kill_allowed_static(900, 1, critical, self_pid, &ancestors),
                "{critical} must be blocked"
            );
        }
        assert!(!is_kill_allowed_static(4, 0, "System", self_pid, &ancestors));
        assert!(!is_kill_allowed_static(self_pid, 1, "bbstats.exe", self_pid, &ancestors));
        assert!(!is_kill_allowed_static(1000, 1, "wt.exe", self_pid, &ancestors));
        assert!(!is_kill_allowed_static(2000, 0, "node.exe", self_pid, &ancestors));
        assert!(is_kill_allowed_static(2001, 1, "node.exe", self_pid, &ancestors));
    }

    #[test]
    fn kill_tree_returns_bottom_up_order_and_skips_reused_pids() {
        let self_pid = 9999;
        let ancestors = HashSet::new();
        let procs = vec![
            mk_proc(100, 50, 1000, "node.exe", 1),
            mk_proc(101, 100, 1005, "esbuild.exe", 1),
            mk_proc(102, 100, 1006, "node.exe", 1),
            mk_proc(103, 102, 1010, "python.exe", 1),
            // Older than parent -> PID reuse, must NOT be included!
            mk_proc(104, 100, 500, "unrelated.exe", 1),
        ];

        let order = build_kill_tree_order(
            ProcessKey::new(100, 1000),
            &procs,
            self_pid,
            &ancestors,
            true,
        );
        let pids: Vec<u32> = order.iter().map(|k| k.pid).collect();
        assert_eq!(pids, vec![103, 102, 101, 100]);
        assert!(!pids.contains(&104));
    }

    #[test]
    fn normal_exit_codes_are_not_crashes() {
        assert!(is_normal_exit_code(0));
        assert!(is_normal_exit_code(0xC000_013A_u32 as i32));
        assert!(is_normal_exit_code(130));
        assert!(is_normal_exit_code(143));
        assert!(!is_normal_exit_code(1));
        assert!(!is_normal_exit_code(101));
        assert!(!is_normal_exit_code(0xC000_0005_u32 as i32));
    }
}
