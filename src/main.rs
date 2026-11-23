#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod app_helpers;
mod cli;
mod config;
mod engine;
mod model;
#[cfg(target_os = "windows")]
mod platform;
#[cfg(target_os = "windows")]
mod render;
mod ui;

fn main() {
    #[cfg(target_os = "windows")]
    windows_main();

    #[cfg(not(target_os = "windows"))]
    unix_main();
}

fn start_engine_and_wait_first_snapshot(
    mode: model::PollingMode,
    sync_cmd: Option<model::EngineCmd>,
) -> engine::EngineHandle {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_bg = Arc::clone(&pair);
    let wake_cb: engine::WakeCallback = Arc::new(move || {
        let (lock, cv) = &*pair_bg;
        if let Ok(mut ready) = lock.lock() {
            *ready = true;
            cv.notify_all();
        }
    });
    let eng = engine::start_engine(wake_cb);
    if let Some(cmd) = sync_cmd {
        eng.send(cmd);
    }
    eng.send(model::EngineCmd::SetPollingMode(mode));

    let (lock, cv) = &*pair;
    let deadline = Instant::now() + Duration::from_millis(2500);
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !*guard {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            break;
        }
        guard = cv
            .wait_timeout(guard, rem)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    eng
}

#[cfg(target_os = "windows")]
fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: Best-effort attachment to parent console before CLI stdout/stderr output.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(target_os = "windows")]
fn fatal_startup_error(msg: &str) {
    crate::config::log_error(msg);
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title: Vec<u16> = "bbstats — Startup Error\0".encode_utf16().collect();
    let body: Vec<u16> = format!("{msg}\0").encode_utf16().collect();
    // SAFETY: Null-terminated UTF-16 dialog strings live for the call duration.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(target_os = "windows")]
fn windows_main() {
    use crate::app::App;
    use crate::config::AppConfig;
    use crate::platform::dpi::{dip_to_px, enable_per_monitor_v2, BASE_DPI};
    use crate::platform::monitor::{
        compute_dock_rect, enumerate_monitors, monitor_work_height_dip, select_monitor,
    };
    use crate::platform::tray::TrayIcon;
    use crate::platform::window::{
        acquire_single_instance, attach_and_run, create_island_hwnd, post_snapshot_wake,
    };
    use crate::render::compositor::CompositionHost;
    use crate::render::device::RenderDevice;
    use crate::ui::layout::{target_dimensions, UiState};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED};

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        raw_args.first().map(|s| s.as_str()),
        Some("run" | "port" | "launch")
    ) {
        attach_parent_console();
        if let Some(code) = cli::try_handle_cli(&raw_args) {
            if code != 0 {
                std::process::exit(code);
            }
            return;
        }
    }

    let mut args = raw_args.into_iter();
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                attach_parent_console();
                println!("bbstats {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                attach_parent_console();
                println!(
                    "bbstats {} — local developer project & background process monitor",
                    env!("CARGO_PKG_VERSION")
                );
                println!();
                println!("USAGE:");
                println!("  bbstats                         Launch the desktop Dynamic Island overlay");
                println!("  bbstats run [--cwd DIR] -- CMD  Run a command inheriting env with live stdio");
                println!("  bbstats port PORT [--kill]      Inspect or stop process listening on PORT");
                println!("  bbstats launch [DIR] [--list]   Launch jobs from bbstats.toml/Procfile/package.json");
                println!("  bbstats --dump-snapshot [FILE]  Print or write plain-text process snapshot");
                println!("  bbstats --dump-shot [DIR]       Render overlay PNG screenshots (requires 'screenshots' feature)");
                println!("  bbstats --version               Print version information");
                println!("  bbstats --help                  Print this help message");
                return;
            }
            "--dump-snapshot" => {
                attach_parent_console();
                let out_file = args.next().map(std::path::PathBuf::from);
                let cfg = AppConfig::load();
                let eng = start_engine_and_wait_first_snapshot(
                    model::PollingMode::ExpandedActive,
                    Some(cfg.to_sync_cmd()),
                );
                let snap = eng.latest();
                let text = engine::format_snapshot_text(&snap);
                if let Some(path) = out_file {
                    if let Err(e) = std::fs::write(&path, &text) {
                        eprintln!("Failed to write {}: {e}", path.display());
                        eng.send(model::EngineCmd::Shutdown);
                        std::process::exit(1);
                    }
                } else {
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(text.as_bytes());
                    let _ = std::io::stdout().flush();
                }
                eng.send(model::EngineCmd::Shutdown);
                return;
            }
            "--dump-shot" => {
                attach_parent_console();
                #[cfg(not(feature = "screenshots"))]
                {
                    let _ = args.count();
                    eprintln!(
                        "bbstats was built without the 'screenshots' feature. Rebuild with: cargo build --features screenshots"
                    );
                    std::process::exit(2);
                }
                #[cfg(feature = "screenshots")]
                {
                    enable_per_monitor_v2();
                    // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
                    unsafe {
                        if let Err(e) = RoInitialize(RO_INIT_SINGLETHREADED) {
                            eprintln!("RoInitialize failed: {e}");
                            std::process::exit(1);
                        }
                    }
                    let cfg = AppConfig::load();
                    let monitors = enumerate_monitors();
                    let mut out_path = std::path::PathBuf::from("plan\\shots");
                    let mut use_live = false;
                    let mut lang_override: Option<model::Language> = None;
                    while let Some(extra) = args.next() {
                        if extra == "--live" {
                            use_live = true;
                        } else if extra == "--lang" {
                            if let Some(val) = args.next() {
                                lang_override = match val.to_ascii_lowercase().as_str() {
                                    "tr" => Some(model::Language::Tr),
                                    "en" => Some(model::Language::En),
                                    _ => lang_override,
                                };
                            }
                        } else if let Some(val) = extra.strip_prefix("--lang=") {
                            lang_override = match val.to_ascii_lowercase().as_str() {
                                "tr" => Some(model::Language::Tr),
                                "en" => Some(model::Language::En),
                                _ => lang_override,
                            };
                        } else {
                            out_path = std::path::PathBuf::from(extra);
                        }
                    }
                    if let Ok(device) = RenderDevice::new() {
                        let (snap, mut shot_cfg) = if use_live {
                            let eng = start_engine_and_wait_first_snapshot(
                                model::PollingMode::ExpandedActive,
                                Some(cfg.to_sync_cmd()),
                            );
                            let s = eng.latest();
                            eng.send(model::EngineCmd::Shutdown);
                            ((*s).clone(), cfg)
                        } else {
                            std::env::set_var("APPDATA", r"C:\Users\Dev\AppData\Roaming");
                            (render::shot::build_demo_snapshot(), AppConfig::default())
                        };
                        if let Some(lang) = lang_override {
                            shot_cfg.language = lang;
                        }
                        let _ = render::shot::dump_shots(
                            &out_path,
                            &device,
                            &snap,
                            &shot_cfg,
                            &monitors,
                        );
                    }
                    // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
                    unsafe {
                        RoUninitialize();
                    }
                    return;
                }
            }
            unknown => {
                attach_parent_console();
                eprintln!("Unknown argument: {unknown}. Run 'bbstats --help' for usage.");
                std::process::exit(2);
            }
        }
    }

    enable_per_monitor_v2();

    // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
    unsafe {
        if let Err(e) = RoInitialize(RO_INIT_SINGLETHREADED) {
            fatal_startup_error(&format!("WinRT RoInitialize failed: {e}"));
            return;
        }
    }

    let cfg = AppConfig::load();
    let monitors = enumerate_monitors();
    let active_mon = select_monitor(&monitors, cfg.monitor_index);
    let init_dpi = active_mon.map(|m| m.dpi).unwrap_or(BASE_DPI);

    let _single_instance = match acquire_single_instance() {
        Some(g) => g,
        None => {
            // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
            unsafe { RoUninitialize() };
            return;
        }
    };

    let init_ui = UiState::default();
    let empty_snap = model::Snapshot::default();
    let work_h_dip = active_mon.map(monitor_work_height_dip).unwrap_or(1000.0);
    let (init_w_dip, init_h_dip, init_r_dip) =
        target_dimensions(&init_ui, &empty_snap, cfg.dock_position, work_h_dip);

    let init_rect = active_mon
        .map(|m| compute_dock_rect(m, cfg.dock_position, init_w_dip, init_h_dip, 0.0))
        .unwrap_or((
            400,
            2,
            dip_to_px(init_w_dip, init_dpi),
            dip_to_px(init_h_dip, init_dpi),
        ));

    let hwnd = match create_island_hwnd(
        init_rect.0,
        init_rect.1,
        init_w_dip,
        init_h_dip,
        init_dpi,
        cfg.always_on_top,
    ) {
        Ok(h) => h,
        Err(e) => {
            fatal_startup_error(&format!("Failed to create overlay window: {e}"));
            // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
            unsafe { RoUninitialize() };
            return;
        }
    };

    let device = match RenderDevice::new() {
        Ok(d) => d,
        Err(e) => {
            fatal_startup_error(&format!("Failed to initialize Direct2D/DXGI graphics: {e}"));
            // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
            unsafe { RoUninitialize() };
            return;
        }
    };

    let comp = match CompositionHost::new(hwnd, &device, init_rect.2, init_rect.3) {
        Ok(c) => c,
        Err(e) => {
            fatal_startup_error(&format!("Failed to initialize DirectComposition swapchain: {e}"));
            // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
            unsafe { RoUninitialize() };
            return;
        }
    };

    let tray = TrayIcon::new(hwnd, cfg.language);

    let hwnd_raw = hwnd.0 as isize;
    let wake_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        post_snapshot_wake(hwnd_raw);
    });
    let engine = engine::start_engine(wake_cb);

    let app = Rc::new(RefCell::new(App::new(
        hwnd,
        cfg,
        engine,
        device,
        comp,
        tray,
        monitors,
        init_rect,
        init_w_dip,
        init_h_dip,
        init_r_dip,
    )));

    attach_and_run(hwnd, app);

    // SAFETY: Balanced single-threaded WinRT apartment initialization on the main thread.
    unsafe {
        RoUninitialize();
    }
}

#[cfg(not(target_os = "windows"))]
fn unix_main() {
    use crate::config::AppConfig;
    use crate::model::{EngineCmd, Language, PollingMode, ProjectStatus, Snapshot};
    use crate::ui::layout::{filter_project_jobs, format_ram, format_uptime};
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::try_handle_cli(&raw_args) {
        if code != 0 {
            std::process::exit(code);
        }
        return;
    }

    let mut args = raw_args.into_iter();
    let first = args.next();
    match first.as_deref() {
        Some("--version" | "-V") => {
            println!("bbstats {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            println!(
                "bbstats {} — local developer project & background process monitor",
                env!("CARGO_PKG_VERSION")
            );
            println!();
            println!("USAGE:");
            println!("  bbstats               Live terminal HUD monitor (--watch)");
            println!("  bbstats run [--cwd DIR] -- CMD  Run a command inheriting env with live stdio");
            println!("  bbstats port PORT [--kill]      Inspect or stop process listening on PORT");
            println!("  bbstats launch [DIR] [--list]   Launch jobs from bbstats.toml/Procfile/package.json");
            println!("  bbstats --watch       Continuously stream grouped project & job status");
            println!("  bbstats --status      Print a formatted single-shot overview");
            println!("  bbstats --daemon      Run headless background monitor without terminal UI");
            println!("  bbstats --dump-snapshot [FILE]  Print or write plain-text process snapshot");
            println!("  bbstats --autostart <on|off>    Enable or disable login autostart");
            println!("  bbstats --version     Print version information");
            println!("  bbstats --help        Print this help message");
            return;
        }
        Some("--autostart") => {
            let mode = args.next().unwrap_or_default().to_ascii_lowercase();
            let enable = match mode.as_str() {
                "on" | "true" | "1" => true,
                "off" | "false" | "0" => false,
                _ => {
                    eprintln!("Usage: bbstats --autostart <on|off>");
                    std::process::exit(2);
                }
            };
            if let Err(e) = crate::config::autostart::set_autostart_enabled(enable) {
                eprintln!("Failed to update autostart: {e}");
                std::process::exit(1);
            }
            let mut cfg = AppConfig::load();
            cfg.autostart = enable;
            if let Err(e) = cfg.save() {
                eprintln!("Failed to save config: {e}");
                std::process::exit(1);
            }
            println!(
                "Autostart {}",
                if enable { "enabled" } else { "disabled" }
            );
            return;
        }
        Some("--dump-snapshot") => {
            let out_file = args.next().map(std::path::PathBuf::from);
            let cfg = AppConfig::load();
            let eng = start_engine_and_wait_first_snapshot(
                PollingMode::ExpandedActive,
                Some(cfg.to_sync_cmd()),
            );
            let snap = eng.latest();
            let text = engine::format_snapshot_text(&snap);
            if let Some(path) = out_file {
                if let Err(e) = std::fs::write(&path, &text) {
                    eprintln!("Failed to write {}: {e}", path.display());
                    eng.send(EngineCmd::Shutdown);
                    std::process::exit(1);
                }
            } else {
                let _ = std::io::stdout().write_all(text.as_bytes());
                let _ = std::io::stdout().flush();
            }
            eng.send(EngineCmd::Shutdown);
            return;
        }
        Some("--status") => {
            let cfg = AppConfig::load();
            let eng = start_engine_and_wait_first_snapshot(
                PollingMode::ExpandedActive,
                Some(cfg.to_sync_cmd()),
            );
            let snap = eng.latest();
            let screen = render_unix_hud(&snap, cfg.language);
            print!("{screen}");
            let _ = std::io::stdout().flush();
            eng.send(EngineCmd::Shutdown);
            return;
        }
        None | Some("--watch") | Some("--daemon") => {}
        Some(unknown) => {
            eprintln!("Unknown argument: {unknown}. Run 'bbstats --help' for usage.");
            std::process::exit(2);
        }
    }

    let daemon_mode = first.as_deref() == Some("--daemon");
    let cfg = AppConfig::load();
    let wake_pair = Arc::new((Mutex::new(false), Condvar::new()));
    let wake_bg = Arc::clone(&wake_pair);
    let wake_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let (lock, cv) = &*wake_bg;
        if let Ok(mut ready) = lock.lock() {
            *ready = true;
            cv.notify_one();
        }
    });

    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn handle_sig(_sig: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }
    // SAFETY: Registers an async-signal-safe AtomicBool handler for SIGINT/SIGTERM.
    unsafe {
        libc::signal(libc::SIGINT, handle_sig as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_sig as *const () as libc::sighandler_t);
    }

    let eng = engine::start_engine(wake_cb);
    eng.send(cfg.to_sync_cmd());
    eng.send(EngineCmd::SetInterval {
        collapsed_ms: Some(cfg.collapsed_poll_ms),
        expanded_ms: Some(cfg.expanded_poll_ms),
    });
    eng.send(EngineCmd::SetPollingMode(if daemon_mode {
        PollingMode::CollapsedIdle
    } else {
        PollingMode::ExpandedActive
    }));

    if daemon_mode {
        while !STOP.load(Ordering::Relaxed) {
            let (lock, cv) = &*wake_pair;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (mut g, _) = cv
                .wait_timeout(guard, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner());
            *g = false;
        }
        eng.send(EngineCmd::Shutdown);
        return;
    }

    // Wait briefly for first snapshot so piped/TTY output is immediately populated.
    {
        let (lock, cv) = &*wake_pair;
        let deadline = std::time::Instant::now() + Duration::from_millis(2500);
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !*guard && !STOP.load(Ordering::Relaxed) {
            let rem = deadline.saturating_duration_since(std::time::Instant::now());
            if rem.is_zero() {
                break;
            }
            guard = cv
                .wait_timeout(guard, rem.min(Duration::from_millis(100)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *guard = false;
    }

    // SAFETY: POSIX descriptor check against STDOUT_FILENO.
    let is_tty = unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 };
    let poll_wait = Duration::from_millis(cfg.expanded_poll_ms.clamp(250, 5000));

    while !STOP.load(Ordering::Relaxed) {
        let snap = eng.latest();
        let screen = render_unix_hud(&snap, cfg.language);
        if is_tty {
            print!("\x1b[H\x1b[2J{screen}");
            println!(
                "\x1b[2m{} · {}: {}\x1b[0m",
                cfg.language.pick("Ctrl+C to exit", "Çıkmak için Ctrl+C"),
                cfg.language.pick("config", "ayar"),
                AppConfig::config_path().display()
            );
        } else {
            print!("{screen}");
        }
        let _ = std::io::stdout().flush();

        if !is_tty {
            break;
        }

        let (lock, cv) = &*wake_pair;
        let wait_deadline = std::time::Instant::now() + poll_wait;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !*guard && !STOP.load(Ordering::Relaxed) {
            let rem = wait_deadline.saturating_duration_since(std::time::Instant::now());
            if rem.is_zero() {
                break;
            }
            guard = cv
                .wait_timeout(guard, rem.min(Duration::from_millis(100)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *guard = false;
    }

    eng.send(EngineCmd::Shutdown);

    fn render_unix_hud(snap: &Snapshot, lang: Language) -> String {
        let mut out = String::new();
        let status_dot = match snap.projects.iter().map(|p| p.status).max_by_key(|s| match s {
            ProjectStatus::Crashed => 3,
            ProjectStatus::HighLoad => 2,
            ProjectStatus::Healthy => 1,
            ProjectStatus::Stopped => 0,
        }) {
            Some(ProjectStatus::Crashed) => "\x1b[31m●\x1b[0m",
            Some(ProjectStatus::HighLoad) => "\x1b[33m●\x1b[0m",
            Some(ProjectStatus::Healthy) => "\x1b[32m●\x1b[0m",
            _ => "\x1b[90m●\x1b[0m",
        };
        let jobs_word = lang.plural(snap.total_jobs as usize, "job", "jobs", "iş");
        let proj_word = lang.plural(snap.total_projects as usize, "project", "projects", "proje");
        let dev_cpu_lbl = lang.pick("Dev CPU", "Geliş. CPU");
        let dev_ram_lbl = lang.pick("Dev RAM", "Geliş. RAM");
        let sys_cpu_lbl = lang.pick("System CPU", "Sistem CPU");

        out.push_str(&format!(
            "\x1b[1m╭─ bbstats {status_dot} {} {jobs_word} · {} {proj_word} · {dev_cpu_lbl} {:.1}% · {dev_ram_lbl} {} ─╮\x1b[0m\n",
            snap.total_jobs,
            snap.total_projects,
            snap.total_dev_cpu,
            format_ram(snap.total_dev_ram_bytes),
        ));
        out.push_str(&format!(
            "  {sys_cpu_lbl} {:.1}% · RAM {} / {}\n",
            snap.system_cpu,
            format_ram(snap.system_ram_used_bytes),
            format_ram(snap.system_ram_total_bytes),
        ));
        for alert in snap.alerts.iter().filter(|a| !a.dismissed).take(2) {
            let tag = match alert.kind {
                AlertKind::Crash { .. } => "\x1b[31m[CRASH]\x1b[0m",
                AlertKind::HighCpu { .. } => "\x1b[33m[HIGH CPU]\x1b[0m",
                AlertKind::MemorySpike { .. } => "\x1b[33m[MEM SPIKE]\x1b[0m",
            };
            out.push_str(&format!("  {tag} {}\n", alert.message));
        }
        out.push('\n');

        let groups = filter_project_jobs(snap, "");
        if groups.is_empty() {
            let empty_msg = lang.pick(
                "No active developer projects detected.",
                "Aktif geliştirici projesi algılanmadı.",
            );
            out.push_str(&format!("  \x1b[2m{empty_msg}\x1b[0m\n"));
            return out;
        }

        for (proj, jobs) in groups {
            let branch = proj
                .git_branch
                .as_deref()
                .map(|b| format!(" \x1b[36m({b})\x1b[0m"))
                .unwrap_or_default();
            let p_jobs_word = lang.plural(jobs.len(), "job", "jobs", "iş");
            out.push_str(&format!(
                "\x1b[1m{}\x1b[0m{branch}  \x1b[2m({} {p_jobs_word} · CPU {:.1}% · {})\x1b[0m\n",
                proj.name,
                jobs.len(),
                proj.cpu,
                format_ram(proj.ram)
            ));
            for job in jobs {
                let ports: String = if job.ports.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> = job.ports.iter().map(|p| format!(":{}", p.port)).collect();
                    format!(" \x1b[34m{}\x1b[0m", list.join(" "))
                };
                let badge = match job.status {
                    ProjectStatus::Crashed => " \x1b[31m[CRASHED]\x1b[0m",
                    ProjectStatus::Stopped => " \x1b[90m[STOPPED]\x1b[0m",
                    ProjectStatus::HighLoad => " \x1b[33m[HIGH]\x1b[0m",
                    ProjectStatus::Healthy => "",
                };
                let short_title = if job.title.chars().count() > 26 {
                    let prefix: String = job.title.chars().take(25).collect();
                    format!("{prefix}…")
                } else {
                    job.title.clone()
                };
                out.push_str(&format!(
                    "  ├─ {:<26} {:>5.1}%  {:>8}  {:>6}{badge}{ports}\n",
                    short_title,
                    job.cpu,
                    format_ram(job.ram),
                    format_uptime(job.uptime),
                ));
            }
            out.push('\n');
        }
        out
    }
}
