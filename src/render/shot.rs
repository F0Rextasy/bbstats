//! Offscreen Direct2D screenshot dumper (`--dump-shot <dir>`) for visual review.

use crate::config::{AppConfig, DockPosition};
use crate::model::{
    BindScope, Job, JobId, PollingMode, PortBinding, ProcInfo, ProcessKey, ProcessRole,
    ProjectGroup, ProjectId, ProjectStatus, Snapshot, TerminalOutput, TerminalSourceKind,
    TransportProto,
};
use crate::platform::monitor::{monitor_work_height_dip, select_monitor, MonitorInfo};
use crate::render::device::RenderDevice;
use crate::render::primitives::DrawCtx;
use crate::ui::hit_test::HitMap;
use crate::ui::layout::{target_dimensions, UiState, ViewMode};
use crate::ui::views::{render_ui, MorphParams};
use std::path::Path;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap1, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_MAP_OPTIONS_READ, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

/// Builds a deterministic, privacy-clean developer project snapshot for `--dump-shot`.
pub fn build_demo_snapshot() -> Snapshot {
    use std::path::PathBuf;

    let mb = 1024 * 1024u64;
    let gb = 1024 * mb;

    let mk_port = |port: u16| PortBinding {
        port,
        protocol: TransportProto::Tcp,
        scope: BindScope::Localhost,
        is_listening: true,
        suggested_url: Some(format!("http://localhost:{port}")),
    };

    let mk_proc = |pid: u32,
                   parent: u32,
                   name: &str,
                   role: ProcessRole,
                   role_lbl: &str,
                   cmd: &str,
                   root: &str,
                   cpu: f32,
                   ram_mb: u64,
                   ports: Vec<PortBinding>,
                   uptime: u64| ProcInfo {
        key: ProcessKey::new(pid, pid as i64 * 100),
        parent_pid: parent,
        name: name.to_string(),
        role,
        role_label: role_lbl.to_string(),
        cmdline: Some(cmd.to_string()),
        working_dir: Some(PathBuf::from(root)),
        cpu,
        ram: ram_mb * mb,
        ports,
        uptime_secs: uptime,
        is_helper_collapsed: false,
        is_critical_system: false,
        can_kill: true,
        can_restart: true,
        access_limited: false,
    };

    let mk_job = |proj_id: &str,
                  proj_name: &str,
                  job_slug: &str,
                  title: &str,
                  subtitle: &str,
                  root: &str,
                  log_hint: &str,
                  procs: Vec<ProcInfo>,
                  ports: Vec<PortBinding>,
                  cpu: f32,
                  ram_mb: u64,
                  uptime: u64| Job {
        id: JobId(format!("{proj_id}::{job_slug}")),
        project: ProjectId(proj_id.to_string()),
        project_name: proj_name.to_string(),
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        icon: None,
        color: None,
        primary_key: procs[0].key,
        procs,
        cpu,
        ram: ram_mb * mb,
        ports,
        uptime,
        status: ProjectStatus::Healthy,
        last_log: Some(log_hint.to_string()),
        crash_summary: None,
        root: Some(PathBuf::from(root)),
        match_hint: job_slug.to_string(),
        log_path: Some(PathBuf::from(root).join("logs").join(format!("{job_slug}.log"))),
        custom_command: None,
        is_watched: false,
        can_kill: true,
        can_restart: true,
    };

    let acme_root = r"C:\workspace\acme-platform";
    let acme_p1 = vec![
        mk_proc(
            14220,
            1000,
            "api-gateway.exe",
            ProcessRole::ApiBackend,
            "Rust API",
            "target\\release\\api-gateway.exe --port 8080 --metrics 9091",
            acme_root,
            5.8,
            142,
            vec![mk_port(8080), mk_port(9091)],
            4320,
        ),
        mk_proc(
            14192,
            1000,
            "cargo.exe",
            ProcessRole::CompilerOrBuild,
            "Cargo Watch",
            "cargo watch -x \"run --bin api-gateway\"",
            acme_root,
            0.6,
            42,
            Vec::new(),
            4322,
        ),
    ];
    let acme_p2 = vec![mk_proc(
        15840,
        1000,
        "node.exe",
        ProcessRole::DevServer,
        "Vite Dev",
        "node node_modules/vite/bin/vite.js --port 5173",
        acme_root,
        3.2,
        212,
        vec![mk_port(5173)],
        4280,
    )];
    let acme_p3 = vec![mk_proc(
        16210,
        1000,
        "python.exe",
        ProcessRole::WorkerOrQueue,
        "Python Worker",
        "python -m worker.webhooks --concurrency 4",
        acme_root,
        1.8,
        96,
        Vec::new(),
        3900,
    )];
    let mut acme_all_procs = Vec::new();
    acme_all_procs.extend(acme_p1.clone());
    acme_all_procs.extend(acme_p2.clone());
    acme_all_procs.extend(acme_p3.clone());

    let acme = ProjectGroup {
        id: ProjectId("demo:acme-platform".to_string()),
        name: "acme-platform".to_string(),
        root: Some(PathBuf::from(acme_root)),
        git_branch: Some("main".to_string()),
        icon: None,
        color: Some("#0A84FF".to_string()),
        pinned: true,
        status: ProjectStatus::Healthy,
        cpu: 11.4,
        ram: 492 * mb,
        ports: vec![mk_port(8080), mk_port(5173), mk_port(9091)],
        jobs: vec![
            mk_job(
                "demo:acme-platform",
                "acme-platform",
                "api-gateway",
                "api-gateway",
                "crates/gateway · cargo run",
                acme_root,
                "GET /v1/health 200 (1.2ms)",
                acme_p1,
                vec![mk_port(8080), mk_port(9091)],
                6.4,
                184,
                4320,
            ),
            mk_job(
                "demo:acme-platform",
                "acme-platform",
                "vite-web-app",
                "vite-web-app",
                "apps/web · pnpm dev",
                acme_root,
                "vite v6.1 ready in 190ms",
                acme_p2,
                vec![mk_port(5173)],
                3.2,
                212,
                4280,
            ),
            mk_job(
                "demo:acme-platform",
                "acme-platform",
                "stripe-webhook-worker",
                "stripe-webhook-worker",
                "services/billing · python worker",
                acme_root,
                "processed 48 events",
                acme_p3,
                Vec::new(),
                1.8,
                96,
                3900,
            ),
        ],
        watchdog_note: None,
        procs: acme_all_procs,
        cpu_history: [
            6, 8, 9, 14, 18, 13, 11, 10, 15, 22, 19, 14, 12, 10, 13, 16, 20, 17, 13, 11, 12, 15,
            18, 16, 13, 11, 12, 14, 12, 11,
        ],
        ram_history_mb: [
            240, 265, 295, 340, 385, 360, 315, 290, 320, 390, 445, 430, 395, 370, 410, 465, 510,
            480, 435, 405, 425, 460, 495, 470, 440, 455, 480, 502, 488, 492,
        ],
    };

    let ml_root = r"C:\workspace\ml-pipeline";
    let ml_p1 = vec![
        mk_proc(
            18412,
            1000,
            "python.exe",
            ProcessRole::WorkerOrQueue,
            "Embedder",
            "python index_vectors.py --batch 128 --port 9090",
            ml_root,
            14.2,
            340,
            vec![mk_port(9090)],
            1860,
        ),
        mk_proc(
            18440,
            18412,
            "worker.exe",
            ProcessRole::WorkerOrQueue,
            "ONNX Pool",
            "worker.exe --threads 4",
            ml_root,
            4.3,
            80,
            Vec::new(),
            1859,
        ),
    ];
    let ml_p2 = vec![mk_proc(
        18604,
        1000,
        "python.exe",
        ProcessRole::WorkerOrQueue,
        "Scheduler",
        "python scheduler.py --interval 60",
        ml_root,
        1.1,
        68,
        Vec::new(),
        5400,
    )];
    let mut ml_procs = Vec::new();
    ml_procs.extend(ml_p1.clone());
    ml_procs.extend(ml_p2.clone());

    let ml = ProjectGroup {
        id: ProjectId("demo:ml-pipeline".to_string()),
        name: "ml-pipeline".to_string(),
        root: Some(PathBuf::from(ml_root)),
        git_branch: Some("feat/embeddings".to_string()),
        icon: None,
        color: Some("#34D399".to_string()),
        pinned: false,
        status: ProjectStatus::Healthy,
        cpu: 19.6,
        ram: 488 * mb,
        ports: vec![mk_port(9090)],
        jobs: vec![
            mk_job(
                "demo:ml-pipeline",
                "ml-pipeline",
                "embedding-indexer",
                "embedding-indexer",
                "index_vectors.py --batch 128",
                ml_root,
                "indexed batch 18/24 (94.2%)",
                ml_p1,
                vec![mk_port(9090)],
                18.5,
                420,
                1860,
            ),
            mk_job(
                "demo:ml-pipeline",
                "ml-pipeline",
                "queue-scheduler",
                "queue-scheduler",
                "scheduler.py · every 60s",
                ml_root,
                "next sweep in 42s",
                ml_p2,
                Vec::new(),
                1.1,
                68,
                5400,
            ),
        ],
        watchdog_note: None,
        procs: ml_procs,
        cpu_history: [
            12, 15, 22, 28, 31, 24, 18, 16, 20, 27, 34, 29, 22, 19, 17, 25, 30, 26, 21, 18, 20, 24,
            28, 25, 22, 19, 21, 23, 21, 20,
        ],
        ram_history_mb: [
            180, 210, 260, 330, 390, 350, 280, 240, 290, 370, 460, 520, 470, 410, 360, 430, 505,
            490, 420, 380, 405, 450, 515, 495, 455, 430, 460, 492, 482, 488,
        ],
    };

    let infra_root = r"C:\workspace\infra-compose";
    let infra_p1 = vec![mk_proc(
        19200,
        1000,
        "postgres.exe",
        ProcessRole::DatabaseOrCache,
        "PostgreSQL 16",
        "postgres -D /var/lib/postgresql/data -p 5432",
        infra_root,
        2.4,
        164,
        vec![mk_port(5432)],
        9600,
    )];
    let infra_p2 = vec![mk_proc(
        19312,
        1000,
        "redis-server.exe",
        ProcessRole::DatabaseOrCache,
        "Redis 7",
        "redis-server --port 6379 --appendonly yes",
        infra_root,
        0.7,
        34,
        vec![mk_port(6379)],
        9600,
    )];
    let mut infra_procs = Vec::new();
    infra_procs.extend(infra_p1.clone());
    infra_procs.extend(infra_p2.clone());

    let infra = ProjectGroup {
        id: ProjectId("demo:infra-compose".to_string()),
        name: "infra-compose".to_string(),
        root: Some(PathBuf::from(infra_root)),
        git_branch: None,
        icon: None,
        color: Some("#F59E0B".to_string()),
        pinned: false,
        status: ProjectStatus::Healthy,
        cpu: 3.1,
        ram: 198 * mb,
        ports: vec![mk_port(5432), mk_port(6379)],
        jobs: vec![
            mk_job(
                "demo:infra-compose",
                "infra-compose",
                "postgres-16",
                "postgres-16",
                "compose service · postgres:16-alpine",
                infra_root,
                "database system is ready to accept connections",
                infra_p1,
                vec![mk_port(5432)],
                2.4,
                164,
                9600,
            ),
            mk_job(
                "demo:infra-compose",
                "infra-compose",
                "redis-cache",
                "redis-cache",
                "compose service · redis:7-alpine",
                infra_root,
                "Ready to accept connections tcp",
                infra_p2,
                vec![mk_port(6379)],
                0.7,
                34,
                9600,
            ),
        ],
        watchdog_note: None,
        procs: infra_procs,
        cpu_history: [3; 30],
        ram_history_mb: [
            190, 191, 192, 192, 194, 194, 195, 195, 196, 196, 196, 197, 197, 197, 198, 198, 198,
            198, 198, 198, 198, 198, 198, 198, 198, 198, 198, 198, 198, 198,
        ],
    };

    let terminal = TerminalOutput {
        project_id: ProjectId("demo:acme-platform".to_string()),
        job_id: Some(JobId("demo:acme-platform::api-gateway".to_string())),
        source_kind: TerminalSourceKind::LogFile,
        source_hint: Some("api-gateway.log".to_string()),
        lines: vec![
            "2026-11-18T11:48:02Z INFO  booting acme-gateway v0.4.2 (release)".to_string(),
            "2026-11-18T11:48:02Z INFO  connected postgres://localhost:5432/acme (pool=16)"
                .to_string(),
            "2026-11-18T11:48:02Z INFO  connected redis://localhost:6379".to_string(),
            "2026-11-18T11:48:03Z READY listening on 127.0.0.1:8080 (metrics :9091)".to_string(),
            "2026-11-18T11:48:15Z WARN  upstream cache miss ratio 14.2% (>10% threshold)"
                .to_string(),
            "2026-11-18T11:48:24Z INFO  POST /v1/events/ingest -> 202 Accepted (4.1ms)"
                .to_string(),
            "2026-11-18T11:48:39Z ERROR webhook timeout dlv_9f2a (retry in 2s)".to_string(),
            "2026-11-18T11:48:41Z READY webhook delivered dlv_9f2a (200 OK)".to_string(),
            "2026-11-18T11:49:00Z INFO  GET /v1/health -> 200 OK (1.2ms)".to_string(),
        ],
    };

    Snapshot {
        timestamp_ms: 1_794_991_800_000,
        polling_mode: PollingMode::ExpandedActive,
        total_projects: 3,
        total_jobs: 7,
        total_procs: 9,
        total_dev_cpu: 34.1,
        total_dev_ram_bytes: 1178 * mb,
        system_cpu: 28.4,
        system_ram_used_bytes: 14 * gb,
        system_ram_total_bytes: 32 * gb,
        system_cpu_history: [
            18, 22, 19, 25, 31, 28, 24, 29, 34, 30, 27, 25, 22, 26, 33, 38, 35, 31, 29, 27, 24, 26,
            30, 28,
        ],
        system_ram_history: [44; 24],
        projects: vec![acme, ml, infra],
        alerts: Vec::new(),
        all_procs: Vec::new(),
        active_terminal: Some(terminal),
        last_action_error: None,
    }
}

pub fn dump_shots(
    out_path: &Path,
    device: &RenderDevice,
    snap: &Snapshot,
    cfg: &AppConfig,
    monitors: &[MonitorInfo],
) -> Result<(), String> {
    let is_single_file = out_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("png"))
        .unwrap_or(false);

    if is_single_file {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
    } else {
        std::fs::create_dir_all(out_path).map_err(|e| e.to_string())?;
    }

    // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
    let dc: ID2D1DeviceContext = unsafe {
        device
            .d2d_device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(|e| e.to_string())?
    };

    let work_h_dip = select_monitor(monitors, cfg.monitor_index)
        .map(monitor_work_height_dip)
        .unwrap_or(1000.0);

    let mut detail_snap = snap.clone();
    let mut detail_target = None;
    if let Some(first_proj) = snap
        .projects
        .iter()
        .find(|p| p.jobs.iter().any(|j| j.log_path.is_some() || j.last_log.is_some()))
        .or_else(|| snap.projects.first())
    {
        let first_job = first_proj
            .jobs
            .iter()
            .find(|j| j.log_path.is_some() || j.last_log.is_some())
            .or_else(|| first_proj.jobs.first());
        if detail_snap.active_terminal.is_none() {
            let pids: Vec<u32> = first_job
                .map(|j| j.procs.iter().map(|pr| pr.key.pid).collect())
                .unwrap_or_default();
            let log_path = first_job.and_then(|j| j.log_path.as_deref());
            let managed = crate::engine::terminal::ManagedJobStore::new();
            let empty_rules: &[crate::engine::labels::LabelRule] = &[];
            detail_snap.active_terminal = Some(crate::engine::terminal::inspect_terminal_output(
                &first_proj.id,
                first_job.map(|j| &j.id),
                &pids,
                log_path,
                empty_rules,
                &managed,
            ));
        }
        detail_target = Some((first_proj.id.clone(), first_job.map(|j| j.id.clone())));
    }

    let mut crash_snap = snap.clone();
    if !is_single_file {
        let crash_pid = ProjectId("shot:crash-demo".to_string());
        let crash_jid = JobId("shot:crash-worker".to_string());
        crash_snap.projects.insert(
            0,
            crate::model::ProjectGroup {
                id: crash_pid.clone(),
                name: "payment-worker".to_string(),
                root: None,
                git_branch: Some("main".to_string()),
                icon: None,
                color: None,
                pinned: false,
                status: ProjectStatus::Crashed,
                cpu: 0.0,
                ram: 0,
                ports: Vec::new(),
                jobs: vec![crate::model::Job {
                    id: crash_jid,
                    project: crash_pid.clone(),
                    project_name: "payment-worker".to_string(),
                    title: "stripe-webhook-consumer".to_string(),
                    subtitle: "python worker.py --queue payments".to_string(),
                    icon: None,
                    color: None,
                    procs: Vec::new(),
                    cpu: 0.0,
                    ram: 0,
                    ports: Vec::new(),
                    uptime: 0,
                    status: ProjectStatus::Crashed,
                    last_log: None,
                    crash_summary: Some(
                        cfg.language
                            .pick(
                                "Exit code 101 · panicked at src/main.rs:42",
                                "Çıkış kodu 101 · panicked at src/main.rs:42",
                            )
                            .to_string(),
                    ),
                    root: None,
                    primary_key: crate::model::ProcessKey::new(4242, 1),
                    match_hint: "worker.py".to_string(),
                    log_path: None,
                    custom_command: Some("python worker.py --queue payments".to_string()),
                    is_watched: true,
                    can_kill: false,
                    can_restart: true,
                }],
                watchdog_note: None,
                procs: Vec::new(),
                cpu_history: [8, 12, 19, 44, 68, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                ram_history_mb: [64; 30],
            },
        );
        crash_snap.total_projects = crash_snap.total_projects.saturating_add(1);
        crash_snap.total_jobs = crash_snap.total_jobs.saturating_add(1);
        crash_snap.alerts.insert(
            0,
            crate::model::Alert {
                id: 999,
                project_id: crash_pid,
                project_name: "payment-worker".to_string(),
                kind: crate::model::AlertKind::Crash {
                    pid: 4242,
                    process_name: "python.exe".to_string(),
                    exit_code: 101,
                    can_restart: true,
                },
                message: cfg
                    .language
                    .pick(
                        "stripe-webhook-consumer exited unexpectedly (code 101)",
                        "stripe-webhook-consumer beklenmedik şekilde sonlandı (101)",
                    )
                    .to_string(),
                timestamp_ms: crash_snap.timestamp_ms,
                dismissed: false,
            },
        );
    }

    let mut tab_detail_snap = detail_snap.clone();
    let mut tab_detail_target = detail_target.clone();
    if !is_single_file {
        if tab_detail_target.is_none() {
            let synth_pid = ProjectId("shot:tab-demo".to_string());
            let synth_jid = JobId("shot:tab-api".to_string());
            tab_detail_snap.projects.push(crate::model::ProjectGroup {
                id: synth_pid.clone(),
                name: "demo-stack".to_string(),
                root: None,
                git_branch: None,
                icon: None,
                color: None,
                pinned: false,
                status: ProjectStatus::Healthy,
                cpu: 4.2,
                ram: 128 * 1024 * 1024,
                ports: Vec::new(),
                jobs: vec![crate::model::Job {
                    id: synth_jid.clone(),
                    project: synth_pid.clone(),
                    project_name: "demo-stack".to_string(),
                    title: "api-server".to_string(),
                    subtitle: "node dist/api.js".to_string(),
                    icon: None,
                    color: None,
                    procs: Vec::new(),
                    cpu: 1.8,
                    ram: 64 * 1024 * 1024,
                    ports: Vec::new(),
                    uptime: 420,
                    status: ProjectStatus::Healthy,
                    last_log: None,
                    crash_summary: None,
                    root: None,
                    primary_key: crate::model::ProcessKey::new(7776, 1),
                    match_hint: "api.js".to_string(),
                    log_path: None,
                    custom_command: None,
                    is_watched: false,
                    can_kill: true,
                    can_restart: false,
                }],
                watchdog_note: None,
                procs: Vec::new(),
                cpu_history: [6; 30],
                ram_history_mb: [96; 30],
            });
            tab_detail_target = Some((synth_pid, Some(synth_jid)));
        }
        if let Some((ref pid, _)) = tab_detail_target {
            if let Some(proj) = tab_detail_snap.projects.iter_mut().find(|p| &p.id == pid) {
                if proj.jobs.len() < 2 {
                    let second_id = JobId("shot:second-tab-worker".to_string());
                    proj.jobs.push(crate::model::Job {
                        id: second_id.clone(),
                        project: proj.id.clone(),
                        project_name: proj.name.clone(),
                        title: "queue-worker".to_string(),
                        subtitle: "node dist/worker.js".to_string(),
                        icon: None,
                        color: None,
                        procs: Vec::new(),
                        cpu: 3.4,
                        ram: 96 * 1024 * 1024,
                        ports: Vec::new(),
                        uptime: 185,
                        status: ProjectStatus::Healthy,
                        last_log: Some("connected to redis://127.0.0.1:6379".to_string()),
                        crash_summary: None,
                        root: proj.root.clone(),
                        primary_key: crate::model::ProcessKey::new(7777, 1),
                        match_hint: "worker.js".to_string(),
                        log_path: None,
                        custom_command: None,
                        is_watched: false,
                        can_kill: true,
                        can_restart: false,
                    });
                    tab_detail_target = Some((proj.id.clone(), Some(second_id)));
                } else {
                    tab_detail_target =
                        Some((proj.id.clone(), proj.jobs.get(1).map(|j| j.id.clone())));
                }
            }
        }
    }

    struct ShotFrame<'a> {
        path: std::path::PathBuf,
        mode: ViewMode,
        prev: Option<(ViewMode, f32, f32)>,
        target_w: f32,
        target_h: f32,
        cur_w: f32,
        cur_h: f32,
        cur_r: f32,
        morph_t: f32,
        bg_opacity: f32,
        hover_pos_dip: Option<(f32, f32)>,
        hover_first_job: bool,
        detail_tab_t: f32,
        detail_tab_dir: f32,
        cfg: AppConfig,
        snap: &'a Snapshot,
    }

    let mut frames: Vec<ShotFrame<'_>> = Vec::new();
    let static_modes: Vec<(std::path::PathBuf, ViewMode, AppConfig, &Snapshot)> = if is_single_file {
        vec![(out_path.to_path_buf(), ViewMode::Expanded, cfg.clone(), snap)]
    } else {
        let mut right_cfg = cfg.clone();
        right_cfg.dock_position = DockPosition::RightEdge;
        let mut list = vec![
            (out_path.join("01-collapsed-pill.png"), ViewMode::Collapsed, cfg.clone(), snap),
            (out_path.join("02-hover-peek.png"), ViewMode::HoverPeek, cfg.clone(), snap),
            (out_path.join("03-expanded-panel.png"), ViewMode::Expanded, cfg.clone(), snap),
            (out_path.join("04-settings.png"), ViewMode::Settings, cfg.clone(), snap),
            (out_path.join("05-right-rail.png"), ViewMode::Collapsed, right_cfg, snap),
        ];
        if let Some((pid, jid)) = detail_target {
            list.push((
                out_path.join("06-detail-terminal.png"),
                ViewMode::Detail {
                    project_id: pid,
                    job_id: jid,
                },
                cfg.clone(),
                &detail_snap,
            ));
        }
        list
    };

    for (target_path, mode, shot_cfg, use_snap) in static_modes {
        let state = UiState {
            mode: mode.clone(),
            ..Default::default()
        };
        let (tw, th, tr) =
            target_dimensions(&state, use_snap, shot_cfg.dock_position, work_h_dip);
        let bg_opacity = if matches!(mode, ViewMode::Collapsed) {
            shot_cfg.idle_opacity
        } else {
            shot_cfg.glass_opacity
        };
        let is_expanded_shot = matches!(mode, ViewMode::Expanded);
        frames.push(ShotFrame {
            path: target_path,
            mode,
            prev: None,
            target_w: tw,
            target_h: th,
            cur_w: tw,
            cur_h: th,
            cur_r: tr,
            morph_t: 1.0,
            bg_opacity,
            hover_pos_dip: if is_expanded_shot {
                Some((tw * 0.38, 18.0))
            } else {
                None
            },
            hover_first_job: is_expanded_shot,
            detail_tab_t: 1.0,
            detail_tab_dir: 1.0,
            cfg: shot_cfg,
            snap: use_snap,
        });
    }

    if !is_single_file {
        let exp_state = UiState {
            mode: ViewMode::Expanded,
            ..Default::default()
        };
        let col_state = UiState {
            mode: ViewMode::Collapsed,
            ..Default::default()
        };
        let (ew, eh, er) = target_dimensions(&exp_state, snap, cfg.dock_position, work_h_dip);
        let (cw, ch, cr) = target_dimensions(&col_state, snap, cfg.dock_position, work_h_dip);
        let lerp = |a: f32, b: f32, f: f32| a + (b - a) * f;
        for (name, to_exp, morph_t, geom_f) in [
            ("07-morph-open-20.png", true, 0.20f32, 0.34f32),
            ("08-morph-open-65.png", true, 0.65f32, 0.80f32),
            ("09-morph-close-25.png", false, 0.25f32, 0.32f32),
            ("10-morph-close-75.png", false, 0.75f32, 0.86f32),
        ] {
            let (mode, prev, tw, th, cur_w, cur_h, cur_r, bg_op) = if to_exp {
                (
                    ViewMode::Expanded,
                    Some((ViewMode::Collapsed, cw, ch)),
                    ew,
                    eh,
                    lerp(cw, ew, geom_f),
                    lerp(ch, eh, geom_f),
                    lerp(cr, er, geom_f),
                    lerp(cfg.idle_opacity.max(0.75), cfg.glass_opacity, geom_f),
                )
            } else {
                (
                    ViewMode::Collapsed,
                    Some((ViewMode::Expanded, ew, eh)),
                    cw,
                    ch,
                    lerp(ew, cw, geom_f),
                    lerp(eh, ch, geom_f),
                    lerp(er, cr, geom_f),
                    lerp(cfg.glass_opacity, cfg.idle_opacity, geom_f),
                )
            };
            frames.push(ShotFrame {
                path: out_path.join(name),
                mode,
                prev,
                target_w: tw,
                target_h: th,
                cur_w,
                cur_h,
                cur_r,
                morph_t,
                bg_opacity: bg_op,
                hover_pos_dip: None,
                hover_first_job: false,
                detail_tab_t: 1.0,
                detail_tab_dir: 1.0,
                cfg: cfg.clone(),
                snap,
            });
        }

        // Frame 11: Crash alert red ambient glow + cursor specular + crashed job row
        let (cew, ceh, cer) = target_dimensions(&exp_state, &crash_snap, cfg.dock_position, work_h_dip);
        frames.push(ShotFrame {
            path: out_path.join("11-crash-glow-alert.png"),
            mode: ViewMode::Expanded,
            prev: None,
            target_w: cew,
            target_h: ceh,
            cur_w: cew,
            cur_h: ceh,
            cur_r: cer,
            morph_t: 1.0,
            bg_opacity: cfg.glass_opacity,
            hover_pos_dip: Some((cew * 0.62, 16.0)),
            hover_first_job: true,
            detail_tab_t: 1.0,
            detail_tab_dir: 1.0,
            cfg: cfg.clone(),
            snap: &crash_snap,
        });

        // Frame 12: Detail tab-switch micro-transition (stationary header/tabs + sliding body)
        if let Some((pid, jid)) = tab_detail_target {
            let tab_mode = ViewMode::Detail {
                project_id: pid,
                job_id: jid,
            };
            let tab_state = UiState {
                mode: tab_mode.clone(),
                ..Default::default()
            };
            let (tw, th, tr) =
                target_dimensions(&tab_state, &tab_detail_snap, cfg.dock_position, work_h_dip);
            frames.push(ShotFrame {
                path: out_path.join("12-detail-tab-switch.png"),
                mode: tab_mode,
                prev: None,
                target_w: tw,
                target_h: th,
                cur_w: tw,
                cur_h: th,
                cur_r: tr,
                morph_t: 1.0,
                bg_opacity: cfg.glass_opacity,
                hover_pos_dip: Some((160.0, 62.0)),
                hover_first_job: false,
                detail_tab_t: 0.45,
                detail_tab_dir: 1.0,
                cfg: cfg.clone(),
                snap: &tab_detail_snap,
            });
        }
    }

    for frame in frames {
        let mut state = UiState {
            mode: frame.mode,
            hover_pos_dip: frame.hover_pos_dip,
            detail_tab_t: frame.detail_tab_t,
            detail_tab_dir: frame.detail_tab_dir,
            ..Default::default()
        };
        let w_dip = frame.cur_w;
        let h_dip = frame.cur_h;
        let r_dip = frame.cur_r;
        let pad = 16u32;
        let w = (w_dip.ceil() as u32) + pad * 2;
        let h = (h_dip.ceil() as u32) + pad * 2;

        let target_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let cpu_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };

        let size = D2D_SIZE_U {
            width: w,
            height: h,
        };
        // SAFETY: Active Direct2D device context or geometry sink inside a valid draw frame.
        unsafe {
            let target_bmp: ID2D1Bitmap1 = dc
                .CreateBitmap(size, None, 0, &target_props)
                .map_err(|e| e.to_string())?;
            let cpu_bmp: ID2D1Bitmap1 = dc
                .CreateBitmap(size, None, 0, &cpu_props)
                .map_err(|e| e.to_string())?;

            dc.SetTarget(&target_bmp);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.BeginDraw();
            let draw_res: Result<(), String> = (|| {
                // Subtle wallpaper backdrop so smoked glass transparency & specular rim show accurately
                dc.Clear(Some(&D2D1_COLOR_F {
                    r: 0.08,
                    g: 0.11,
                    b: 0.17,
                    a: 1.0,
                }));
                dc.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: pad as f32,
                    M32: pad as f32,
                });

                let ctx = DrawCtx::new(
                    &dc,
                    &device.d2d_factory,
                    &device.dwrite_factory,
                    &device.fonts.tabular_typography,
                )
                .map_err(|e| e.to_string())?;
                let mut hit_map = HitMap::default();
                let morph = MorphParams {
                    prev: frame.prev.as_ref().map(|(m, pw, ph)| (m, *pw, *ph)),
                    target_w: frame.target_w,
                    target_h: frame.target_h,
                    morph_t: frame.morph_t,
                    bg_opacity: frame.bg_opacity,
                };
                render_ui(
                    &ctx,
                    &device.fonts,
                    w_dip,
                    h_dip,
                    r_dip,
                    MorphParams {
                        prev: morph.prev,
                        target_w: morph.target_w,
                        target_h: morph.target_h,
                        morph_t: morph.morph_t,
                        bg_opacity: morph.bg_opacity,
                    },
                    &mut state,
                    frame.snap,
                    &frame.cfg,
                    monitors,
                    false,
                    &mut hit_map,
                )
                .map_err(|e| e.to_string())?;

                if frame.hover_first_job {
                    if let Some(first_job_hit) = hit_map
                        .regions
                        .iter()
                        .find(|r| matches!(r.action, crate::ui::hit_test::HitAction::OpenJobDetail(_, _)))
                        .map(|r| r.id)
                    {
                        state.hovered_hit_id = Some(first_job_hit);
                        hit_map.regions.clear();
                        dc.Clear(Some(&D2D1_COLOR_F {
                            r: 0.08,
                            g: 0.11,
                            b: 0.17,
                            a: 1.0,
                        }));
                        render_ui(
                            &ctx,
                            &device.fonts,
                            w_dip,
                            h_dip,
                            r_dip,
                            morph,
                            &mut state,
                            frame.snap,
                            &frame.cfg,
                            monitors,
                            false,
                            &mut hit_map,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
                Ok(())
            })();

            let end_res = dc.EndDraw(None, None).map_err(|e| e.to_string());
            draw_res?;
            end_res?;
            cpu_bmp
                .CopyFromBitmap(None, &target_bmp, None)
                .map_err(|e| e.to_string())?;

            let mapped = cpu_bmp
                .Map(D2D1_MAP_OPTIONS_READ)
                .map_err(|e| e.to_string())?;
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for row in 0..h {
                let src = std::slice::from_raw_parts(
                    mapped.bits.add((row * mapped.pitch) as usize),
                    (w * 4) as usize,
                );
                for col in 0..w as usize {
                    let b = src[col * 4];
                    let g = src[col * 4 + 1];
                    let r = src[col * 4 + 2];
                    let a = src[col * 4 + 3];
                    let dst = (row as usize * w as usize + col) * 4;
                    rgba[dst] = r;
                    rgba[dst + 1] = g;
                    rgba[dst + 2] = b;
                    rgba[dst + 3] = a;
                }
            }
            let _ = cpu_bmp.Unmap();
            write_png_rgba(&frame.path, w, h, &rgba)?;
        }
    }
    Ok(())
}

fn write_png_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let row_bytes = width as usize * 4;
    let mut raw = Vec::with_capacity((height as usize) * (1 + row_bytes));
    for y in 0..height as usize {
        raw.push(1); // PNG Sub filter (bpp = 4) improves DEFLATE compression on UI gradients
        let row_start = y * row_bytes;
        let row = &rgba[row_start..row_start + row_bytes];
        raw.extend_from_slice(&row[..4.min(row_bytes)]);
        for i in 4..row_bytes {
            raw.push(row[i].wrapping_sub(row[i - 4]));
        }
    }

    let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);

    let mut png = Vec::with_capacity(zlib.len() + 128);
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);

    std::fs::write(path, png).map_err(|e| e.to_string())
}

fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc = 0xFFFF_FFFFu32;
    for &b in tag.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::write_png_rgba;

    #[test]
    fn write_png_rgba_produces_valid_compressed_png() {
        let mut path = std::env::temp_dir();
        path.push(format!("bbstats_png_test_{}.png", std::process::id()));
        let w = 64u32;
        let h = 64u32;
        let rgba = vec![42u8; (w * h * 4) as usize];
        write_png_rgba(&path, w, h, &rgba).expect("write compressed png");
        let bytes = std::fs::read(&path).expect("read png");
        let _ = std::fs::remove_file(&path);

        assert!(bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert!(bytes.windows(4).any(|w| w == b"IHDR"));
        assert!(bytes.windows(4).any(|w| w == b"IDAT"));
        assert!(bytes.windows(4).any(|w| w == b"IEND"));
        assert!(bytes.len() < (w * h * 4) as usize);
    }
}
