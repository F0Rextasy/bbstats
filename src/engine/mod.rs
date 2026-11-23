//! Background process monitoring engine: single `std::thread`, adaptive polling,
//! Job-first project grouping, 8s short-lived job debounce, log tailing,
//! known-stopped job discovery, sparkline histories, crash/load alerts, and safe actions.

pub mod actions;
pub mod collector;
pub mod known_jobs;
pub mod labels;
pub mod project_resolver;
pub mod terminal;
pub mod win_nt;

mod snapshot;
pub use snapshot::format_snapshot_text;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::engine::actions::{
    compute_self_ancestors, execute_safe_kill_many, is_kill_allowed_static, is_normal_exit_code,
    ExitEvent, ExitWatcher,
};
use crate::engine::collector::{
    are_tcp_ports_listening, CollectedProcess, PidNetworkInfo, ProcessCollector,
};
use crate::engine::known_jobs::KnownJobsScanner;
use crate::engine::labels::{expand_win_env_vars, LabelRule};
use crate::engine::project_resolver::{
    LogTailCache, ProcessMetadata, ProjectResolver, RestartRecipe,
};
use crate::engine::terminal::{inspect_terminal_output, ManagedJobStore};
use crate::engine::win_nt::current_pid;
use crate::model::{
    Alert, AlertKind, EngineCmd, JobId, Language, PollingMode, ProcInfo, ProcessKey, ProcessRole,
    ProjectId, Snapshot, SortMode,
};

pub type WakeCallback = Arc<dyn Fn() + Send + Sync>;

use snapshot::{build_snapshot, unix_now_ms, CrashedJobRecord, ProjectRuntimeState};

enum EngineMsg {
    Cmd(EngineCmd),
    Exited(ExitEvent),
    ActionError(String),
    RestartFinished(Vec<JobId>),
}

/// Thread-safe handle held by the UI layer to read the latest `Snapshot` and send `EngineCmd`s.
pub struct EngineHandle {
    latest: Arc<Mutex<Arc<Snapshot>>>,
    tx: Sender<EngineMsg>,
    join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Returns the most recently published immutable telemetry snapshot.
    pub fn latest(&self) -> Arc<Snapshot> {
        self.latest
            .lock()
            .map(|g| Arc::clone(&g))
            .unwrap_or_else(|e| Arc::clone(&e.into_inner()))
    }

    /// Sends a command to the background engine thread, waking it immediately.
    pub fn send(&self, cmd: EngineCmd) {
        let _ = self.tx.send(EngineMsg::Cmd(cmd));
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineMsg::Cmd(EngineCmd::Shutdown));
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Starts the background engine on a single `std::thread` (no async runtime).
pub fn start_engine(wake_cb: WakeCallback) -> EngineHandle {
    let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
    let latest_bg = Arc::clone(&latest);
    let (tx, rx) = mpsc::channel::<EngineMsg>();
    let tx_exit = tx.clone();

    let join = thread::Builder::new()
        .name("bbstats-engine".to_string())
        .spawn(move || {
            let mut runtime = EngineRuntime::new(latest_bg, wake_cb, tx_exit);
            runtime.run(rx);
        })
        .expect("failed to spawn bbstats-engine thread");

    EngineHandle {
        latest,
        tx,
        join: Some(join),
    }
}


#[derive(Debug, Clone)]
struct WatchedProcessSnapshot {
    meta: Arc<ProcessMetadata>,
    had_port: bool,
    uptime_secs: u64,
    ports: Vec<u16>,
}

#[derive(Debug, Clone)]
struct StoredRestartRecipe {
    project_id: ProjectId,
    job_id: JobId,
    primary_key: ProcessKey,
    recipe: RestartRecipe,
    ports: Vec<u16>,
    last_seen: Instant,
}

enum RestartTarget {
    Process(ProcessKey),
    Job(JobId),
    Project(ProjectId),
}

struct EngineRuntime {
    latest: Arc<Mutex<Arc<Snapshot>>>,
    wake_cb: WakeCallback,
    cmd_tx: Sender<EngineMsg>,
    collector: ProcessCollector,
    resolver: ProjectResolver,
    exit_watcher: ExitWatcher,
    log_tail_cache: LogTailCache,
    known_scanner: KnownJobsScanner,
    managed_jobs: Arc<ManagedJobStore>,
    mode: PollingMode,
    collapsed_interval: Duration,
    expanded_interval: Duration,
    show_all_processes: bool,
    inspected_target: Option<(ProjectId, Option<JobId>)>,
    sort_mode: SortMode,
    pinned_keys: HashSet<String>,
    hidden_keys: HashSet<String>,
    alias_keys: HashMap<String, String>,
    project_order: Vec<String>,
    job_order: Vec<String>,
    system_cpu_history: [u8; 24],
    system_ram_history: [u8; 24],
    project_states: HashMap<ProjectId, ProjectRuntimeState>,
    watched_meta: HashMap<ProcessKey, WatchedProcessSnapshot>,
    job_crashes: HashMap<JobId, CrashedJobRecord>,
    last_procs: Vec<CollectedProcess>,
    last_ports: HashMap<u32, PidNetworkInfo>,
    last_ports_keys: HashMap<u32, i64>,
    user_kills: HashMap<ProcessKey, Instant>,
    restart_recipes: HashMap<JobId, StoredRestartRecipe>,
    restarting_jobs: HashMap<JobId, Instant>,
    auto_restart_attempts: HashMap<JobId, Vec<Instant>>,
    alerts: Vec<Alert>,
    next_alert_id: u64,
    tick_counter: u64,
    last_summary_sig: (u16, u16, u16, usize, i32, u64, u64),
    language: Language,
    vanished_since: HashMap<ProcessKey, Instant>,
    last_action_error: Option<(u64, String)>,
    next_error_id: u64,
}

impl EngineRuntime {
    fn new(
        latest: Arc<Mutex<Arc<Snapshot>>>,
        wake_cb: WakeCallback,
        exit_tx_raw: Sender<EngineMsg>,
    ) -> Self {
        let cmd_tx = exit_tx_raw.clone();
        let on_exit = Arc::new(move |ev: ExitEvent| {
            let _ = exit_tx_raw.send(EngineMsg::Exited(ev));
        });
        Self {
            latest,
            wake_cb,
            cmd_tx,
            collector: ProcessCollector::new(),
            resolver: ProjectResolver::new(current_pid()),
            exit_watcher: ExitWatcher::new(on_exit),
            log_tail_cache: LogTailCache::new(),
            known_scanner: KnownJobsScanner::new(),
            managed_jobs: Arc::new(ManagedJobStore::new()),
            mode: PollingMode::CollapsedIdle,
            collapsed_interval: Duration::from_millis(2500),
            expanded_interval: Duration::from_millis(600),
            show_all_processes: false,
            inspected_target: None,
            sort_mode: SortMode::Custom,
            pinned_keys: HashSet::new(),
            hidden_keys: HashSet::new(),
            alias_keys: HashMap::new(),
            project_order: Vec::new(),
            job_order: Vec::new(),
            system_cpu_history: [0; 24],
            system_ram_history: [0; 24],
            project_states: HashMap::with_capacity(32),
            watched_meta: HashMap::with_capacity(64),
            job_crashes: HashMap::new(),
            last_procs: Vec::new(),
            last_ports: HashMap::new(),
            last_ports_keys: HashMap::new(),
            user_kills: HashMap::new(),
            restart_recipes: HashMap::new(),
            restarting_jobs: HashMap::new(),
            auto_restart_attempts: HashMap::new(),
            alerts: Vec::new(),
            next_alert_id: 1,
            tick_counter: 0,
            last_summary_sig: (0, 0, 0, 0, -1, 0, 0),
            language: Language::En,
            vanished_since: HashMap::new(),
            last_action_error: None,
            next_error_id: 0,
        }
    }

    fn record_action_error(&mut self, msg: String) {
        crate::config::log_error(&msg);
        self.next_error_id += 1;
        self.last_action_error = Some((self.next_error_id, msg));
    }

    fn current_interval(&self) -> Duration {
        match self.mode {
            PollingMode::CollapsedIdle => self.collapsed_interval,
            PollingMode::HoverPeek => Duration::from_millis(1000),
            PollingMode::ExpandedActive => self.expanded_interval,
            PollingMode::LowPowerLocked => Duration::from_millis(5000),
        }
    }

    fn run(&mut self, rx: mpsc::Receiver<EngineMsg>) {
        self.tick();
        let mut next_tick = Instant::now() + self.current_interval();

        loop {
            let now = Instant::now();
            let timeout = next_tick.saturating_duration_since(now);
            match rx.recv_timeout(timeout) {
                Ok(first_msg) => {
                    if matches!(first_msg, EngineMsg::Cmd(EngineCmd::Shutdown)) {
                        return;
                    }
                    let mut force_tick = self.handle_msg(first_msg);
                    while let Ok(msg) = rx.try_recv() {
                        if matches!(msg, EngineMsg::Cmd(EngineCmd::Shutdown)) {
                            return;
                        }
                        if self.handle_msg(msg) {
                            force_tick = true;
                        }
                    }
                    if force_tick {
                        self.tick();
                        next_tick = Instant::now() + self.current_interval();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.tick();
                    next_tick = Instant::now() + self.current_interval();
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn handle_msg(&mut self, msg: EngineMsg) -> bool {
        match msg {
            EngineMsg::Exited(ev) => {
                self.handle_process_exit(ev);
                true
            }
            EngineMsg::ActionError(err) => {
                self.record_action_error(err);
                true
            }
            EngineMsg::RestartFinished(job_ids) => {
                for jid in job_ids {
                    self.restarting_jobs.remove(&jid);
                }
                false
            }
            EngineMsg::Cmd(cmd) => match cmd {
                EngineCmd::Shutdown => false,
                EngineCmd::SetPollingMode(mode) => {
                    let prev = self.mode;
                    self.mode = mode;
                    prev != mode && matches!(mode, PollingMode::ExpandedActive | PollingMode::HoverPeek)
                }
                EngineCmd::SetInterval {
                    collapsed_ms,
                    expanded_ms,
                } => {
                    if let Some(ms) = collapsed_ms {
                        self.collapsed_interval = Duration::from_millis(ms.clamp(500, 30_000));
                    }
                    if let Some(ms) = expanded_ms {
                        self.expanded_interval = Duration::from_millis(ms.clamp(250, 10_000));
                    }
                    false
                }
                EngineCmd::Kill { key, tree } => {
                    execute_safe_kill_many(&[key], &self.last_procs, tree, &mut self.user_kills);
                    true
                }
                EngineCmd::KillJob(job_id) => {
                    let keys: Vec<ProcessKey> = self
                        .watched_meta
                        .iter()
                        .filter(|(_, w)| w.meta.job_id == job_id && !w.meta.is_critical_system)
                        .map(|(k, _)| *k)
                        .collect();
                    execute_safe_kill_many(&keys, &self.last_procs, true, &mut self.user_kills);
                    true
                }
                EngineCmd::KillProject(project_id) => {
                    let keys: Vec<ProcessKey> = self
                        .watched_meta
                        .iter()
                        .filter(|(_, w)| w.meta.project_id == project_id && !w.meta.is_critical_system)
                        .map(|(k, _)| *k)
                        .collect();
                    execute_safe_kill_many(&keys, &self.last_procs, true, &mut self.user_kills);
                    true
                }
                EngineCmd::Restart(key) => {
                    let managed = Arc::clone(&self.managed_jobs);
                    let _ = self.execute_restart_target_with(
                        RestartTarget::Process(key),
                        true,
                        |rec| {
                            managed.spawn_with_capture_env(
                                &rec.raw_cmdline,
                                &rec.working_dir,
                                &rec.env_vars,
                                rec.use_shell,
                            )
                        },
                    );
                    true
                }
                EngineCmd::RestartJob(job_id) => {
                    let managed = Arc::clone(&self.managed_jobs);
                    let _ = self.execute_restart_target_with(
                        RestartTarget::Job(job_id),
                        true,
                        |rec| {
                            managed.spawn_with_capture_env(
                                &rec.raw_cmdline,
                                &rec.working_dir,
                                &rec.env_vars,
                                rec.use_shell,
                            )
                        },
                    );
                    true
                }
                EngineCmd::RestartProject(project_id) => {
                    let managed = Arc::clone(&self.managed_jobs);
                    let _ = self.execute_restart_target_with(
                        RestartTarget::Project(project_id),
                        true,
                        |rec| {
                            managed.spawn_with_capture_env(
                                &rec.raw_cmdline,
                                &rec.working_dir,
                                &rec.env_vars,
                                rec.use_shell,
                            )
                        },
                    );
                    true
                }
                EngineCmd::SyncConfigState {
                    pinned_projects,
                    hidden_projects,
                    project_aliases,
                    project_order,
                    job_order,
                    sort_mode,
                    language,
                } => {
                    self.language = language;
                    self.resolver.set_language(language);
                    self.pinned_keys = pinned_projects
                        .into_iter()
                        .map(|s| s.to_ascii_lowercase())
                        .collect();
                    self.hidden_keys = hidden_projects
                        .into_iter()
                        .map(|s| s.to_ascii_lowercase())
                        .collect();
                    self.alias_keys = project_aliases
                        .into_iter()
                        .map(|(k, v)| (k.to_ascii_lowercase(), v))
                        .collect();
                    self.project_order = project_order;
                    self.job_order = job_order;
                    self.sort_mode = sort_mode;
                    for (pid, st) in self.project_states.iter_mut() {
                        let key_l = pid.0.to_ascii_lowercase();
                        st.pinned = self.pinned_keys.contains(&key_l);
                        st.hidden = self.hidden_keys.contains(&key_l);
                        st.display_name_override = self.alias_keys.get(&key_l).cloned();
                    }
                    true
                }
                EngineCmd::SetPinned { project_id, pinned } => {
                    let key_l = project_id.0.to_ascii_lowercase();
                    if pinned {
                        self.pinned_keys.insert(key_l);
                    } else {
                        self.pinned_keys.remove(&key_l);
                    }
                    let st = self.project_states.entry(project_id).or_default();
                    st.pinned = pinned;
                    true
                }
                EngineCmd::SetAlias {
                    project_id,
                    custom_name,
                } => {
                    let cleaned = custom_name.filter(|s| !s.trim().is_empty());
                    let key_l = project_id.0.to_ascii_lowercase();
                    if let Some(ref name) = cleaned {
                        self.alias_keys.insert(key_l, name.clone());
                    } else {
                        self.alias_keys.remove(&key_l);
                    }
                    let st = self.project_states.entry(project_id).or_default();
                    st.display_name_override = cleaned;
                    true
                }
                EngineCmd::HideProject(project_id) => {
                    self.hidden_keys.insert(project_id.0.to_ascii_lowercase());
                    let st = self.project_states.entry(project_id).or_default();
                    st.hidden = true;
                    true
                }
                EngineCmd::DismissAlert(id) => {
                    for a in &mut self.alerts {
                        if a.id == id {
                            a.dismissed = true;
                        }
                    }
                    self.alerts.retain(|a| !a.dismissed);
                    true
                }
                EngineCmd::SetShowAllProcesses(enabled) => {
                    self.show_all_processes = enabled;
                    true
                }
                EngineCmd::InspectTerminal(target) => {
                    let changed = self.inspected_target != target;
                    self.inspected_target = target;
                    changed
                }
                EngineCmd::SaveLabelRule {
                    match_substr,
                    project,
                    job,
                    icon,
                    color,
                    command,
                    working_dir,
                    log_file,
                    watch,
                } => {
                    if let Err(err) = self.resolver.save_label_rule(LabelRule {
                        match_substr,
                        project,
                        job,
                        icon,
                        color,
                        log_file,
                        command,
                        working_dir,
                        watch,
                        ..Default::default()
                    }) {
                        self.record_action_error(format!("labels.toml save failed: {err}"));
                    }
                    true
                }
                EngineCmd::RemoveLabelRule(match_substr) => {
                    if let Err(err) = self.resolver.remove_label_rule(&match_substr) {
                        self.record_action_error(format!("labels.toml remove failed: {err}"));
                    }
                    true
                }
                EngineCmd::StartCustomJob(job_id) => {
                    self.start_custom_job(&job_id);
                    true
                }
                EngineCmd::ForceRescan => {
                    self.resolver.invalidate_all();
                    true
                }
            },
        }
    }

    fn handle_process_exit(&mut self, ev: ExitEvent) {
        self.vanished_since.remove(&ev.key);
        self.exit_watcher.unwatch(&ev.key);
        let Some(watched) = self.watched_meta.remove(&ev.key) else {
            return;
        };

        let was_user_killed = self.user_kills.remove(&ev.key).is_some();
        if was_user_killed || is_normal_exit_code(ev.exit_code) {
            return;
        }

        let is_transient_tool = matches!(
            watched.meta.role,
            ProcessRole::CompilerOrBuild | ProcessRole::IdeTooling
        ) || watched.uptime_secs < 5;
        if is_transient_tool && !watched.had_port {
            return;
        }

        let job_still_alive = self.watched_meta.values().any(|w| {
            w.meta.job_id == watched.meta.job_id
                && !matches!(
                    w.meta.role,
                    ProcessRole::CompilerOrBuild | ProcessRole::IdeTooling
                )
        });
        if job_still_alive {
            return;
        }

        let proj_id = watched.meta.project_id.clone();
        let proj_name = self
            .project_states
            .get(&proj_id)
            .and_then(|s| s.display_name_override.clone())
            .unwrap_or_else(|| watched.meta.project_display_name.clone());

        self.project_states.entry(proj_id.clone()).or_default().crashed_until =
            Some(Instant::now() + Duration::from_secs(300));

        let can_restart = self.restart_recipes.contains_key(&watched.meta.job_id);
        self.job_crashes.insert(
            watched.meta.job_id.clone(),
            CrashedJobRecord {
                until: Instant::now() + Duration::from_secs(300),
                exit_code: ev.exit_code,
                project_id: proj_id.clone(),
                project_name: proj_name.clone(),
                title: watched.meta.job_title.clone(),
                subtitle: watched.meta.job_subtitle.clone(),
                icon: watched.meta.job_icon.clone(),
                color: watched.meta.job_color.clone(),
                root: watched.meta.project_root.clone(),
                primary_key: ev.key,
                match_hint: watched.meta.match_hint.clone(),
                log_path: watched.meta.log_path.clone(),
                custom_command: watched.meta.custom_command.clone(),
                is_watched: watched.meta.is_watched,
                can_restart,
            },
        );
        let port_hint = watched
            .ports
            .first()
            .map(|p| format!(" · :{p} {}", self.language.pick("down", "kapalı")))
            .unwrap_or_default();
        let job_name = watched.meta.job_title.clone();
        let auto_suffix = if watched.meta.auto_restart && can_restart {
            if self.record_and_should_auto_restart(&watched.meta.job_id, Instant::now()) {
                let managed = Arc::clone(&self.managed_jobs);
                let started = self.execute_restart_target_with(
                    RestartTarget::Job(watched.meta.job_id.clone()),
                    true,
                    |rec| {
                        managed.spawn_with_capture_env(
                            &rec.raw_cmdline,
                            &rec.working_dir,
                            &rec.env_vars,
                            rec.use_shell,
                        )
                    },
                );
                if started {
                    self.language
                        .pick(" · auto-restarting", " · otomatik yeniden başlatılıyor")
                } else {
                    self.language
                        .pick(" · auto-restart failed", " · otomatik yeniden başlatma başarısız")
                }
            } else {
                self.language.pick(
                    " · auto-restart limit (3/10m) reached",
                    " · otomatik yeniden başlatma sınırı (3/10dk) aşıldı",
                )
            }
        } else {
            ""
        };
        let message = match self.language {
            Language::En => format!(
                "{proj_name} › {job_name} crashed (code {}{port_hint}){auto_suffix}",
                ev.exit_code
            ),
            Language::Tr => format!(
                "{proj_name} › {job_name} çöktü (kod {}{port_hint}){auto_suffix}",
                ev.exit_code
            ),
        };
        let alert = Alert {
            id: self.next_alert_id,
            project_id: proj_id,
            project_name: proj_name.clone(),
            kind: AlertKind::Crash {
                pid: ev.key.pid,
                process_name: watched.meta.image_name.clone(),
                exit_code: ev.exit_code,
                can_restart,
            },
            message,
            timestamp_ms: unix_now_ms(),
            dismissed: false,
        };
        self.next_alert_id += 1;
        self.alerts.insert(0, alert);
        if self.alerts.len() > 20 {
            self.alerts.truncate(20);
        }
    }

    fn record_and_should_auto_restart(&mut self, job_id: &JobId, now: Instant) -> bool {
        let attempts = self.auto_restart_attempts.entry(job_id.clone()).or_default();
        attempts.retain(|t| now.saturating_duration_since(*t) < Duration::from_secs(600));
        if attempts.len() >= 3 {
            return false;
        }
        attempts.push(now);
        true
    }

    fn clear_job_crash(&mut self, meta: &ProcessMetadata) {
        if matches!(
            meta.role,
            ProcessRole::CompilerOrBuild | ProcessRole::IdeTooling
        ) {
            return;
        }
        if self.job_crashes.remove(&meta.job_id).is_some() {
            let proj_has_crashes = self
                .job_crashes
                .values()
                .any(|c| c.project_id == meta.project_id);
            if !proj_has_crashes {
                if let Some(ps) = self.project_states.get_mut(&meta.project_id) {
                    ps.crashed_until = None;
                }
                for alert in &mut self.alerts {
                    if alert.project_id == meta.project_id
                        && matches!(alert.kind, AlertKind::Crash { .. })
                    {
                        alert.dismissed = true;
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        self.tick_counter += 1;
        let now = Instant::now();
        self.user_kills
            .retain(|_, ts| now.saturating_duration_since(*ts) < Duration::from_secs(12));
        self.job_crashes.retain(|_, rec| now < rec.until);

        let collected = self.collector.collect_processes();

        let should_poll_ports = match self.mode {
            PollingMode::ExpandedActive => true,
            PollingMode::HoverPeek => self.tick_counter % 2 == 1,
            PollingMode::CollapsedIdle => self.tick_counter % 4 == 1,
            PollingMode::LowPowerLocked => false,
        };
        if should_poll_ports {
            self.last_ports = self.collector.collect_ports();
            self.last_ports_keys = collected
                .processes
                .iter()
                .map(|p| (p.key.pid, p.key.create_time_100ns))
                .collect();
        } else {
            // Evict ports whose owning PID exited or was reused since the last port poll.
            let live_times: HashMap<u32, i64> = collected
                .processes
                .iter()
                .map(|p| (p.key.pid, p.key.create_time_100ns))
                .collect();
            self.last_ports.retain(|pid, _| {
                match (live_times.get(pid), self.last_ports_keys.get(pid)) {
                    (Some(&cur_ct), Some(&snap_ct)) => cur_ct == snap_ct,
                    _ => false,
                }
            });
        }

        let allow_expanded_io = matches!(self.mode, PollingMode::ExpandedActive);
        let ports_ref = &self.last_ports;
        self.resolver
            .resolve_tick(&collected.processes, allow_expanded_io, &|pid| {
                ports_ref
                    .get(&pid)
                    .is_some_and(|n| n.ports.iter().any(|p| p.is_listening))
            });

        let current_keys: HashSet<ProcessKey> = collected.processes.iter().map(|p| p.key).collect();
        self.reconcile_vanished_watched(&current_keys, now);

        let self_pid = current_pid();
        let ancestors = compute_self_ancestors(&collected.processes, self_pid);

        for proc in &collected.processes {
            let Some(meta) = self.resolver.cached_meta(&proc.key).cloned() else {
                continue;
            };
            if meta.is_ignored_noise || meta.is_critical_system {
                continue;
            }
            self.clear_job_crash(&meta);
            let p_ports: Vec<u16> = self
                .last_ports
                .get(&proc.key.pid)
                .map(|n| n.ports.iter().filter(|p| p.is_listening).map(|p| p.port).collect())
                .unwrap_or_default();

            if !self.watched_meta.contains_key(&proc.key) {
                self.exit_watcher.ensure_watched(proc.key);
            }
            self.watched_meta.insert(
                proc.key,
                WatchedProcessSnapshot {
                    meta,
                    had_port: !p_ports.is_empty(),
                    uptime_secs: proc.uptime_secs,
                    ports: p_ports,
                },
            );
        }
        self.update_restart_recipes(&collected.processes, now);

        // Known stopped jobs & periodic watchdog info (cheap, 60s TTL, only while expanded).
        let known_ref = self.known_scanner.get_or_refresh(
            allow_expanded_io,
            self.resolver.labels(),
            self.language,
        );

        let sys_cpu_sample = collected.system_cpu_percent.round().clamp(0.0, 100.0) as u8;
        let sys_ram_pct = if collected.system_ram_total_bytes > 0 {
            ((collected.system_ram_used_bytes as f64 / collected.system_ram_total_bytes as f64)
                * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8
        } else {
            0
        };
        if self.tick_counter <= 2 && self.system_cpu_history[0] == 0 {
            self.system_cpu_history.fill(sys_cpu_sample);
            self.system_ram_history.fill(sys_ram_pct);
        } else {
            self.system_cpu_history.copy_within(1.., 0);
            self.system_cpu_history[23] = sys_cpu_sample;
            self.system_ram_history.copy_within(1.., 0);
            self.system_ram_history[23] = sys_ram_pct;
        }

        let mut snapshot = build_snapshot(
            &collected.processes,
            collected.system_cpu_percent,
            collected.system_ram_used_bytes,
            collected.system_ram_total_bytes,
            self.system_cpu_history,
            self.system_ram_history,
            &self.last_ports,
            &self.resolver,
            known_ref,
            &mut self.log_tail_cache,
            &self.job_crashes,
            &mut self.project_states,
            &self.pinned_keys,
            &self.hidden_keys,
            &self.alias_keys,
            self.sort_mode,
            &self.project_order,
            &self.job_order,
            &mut self.alerts,
            &mut self.next_alert_id,
            self.mode,
            self_pid,
            &ancestors,
            self.language,
        );

        if self.project_states.len() > 64 {
            let active_ids: HashSet<ProjectId> =
                snapshot.projects.iter().map(|p| p.id.clone()).collect();
            self.project_states.retain(|id, st| {
                active_ids.contains(id)
                    || st.pinned
                    || st.hidden
                    || st.display_name_override.is_some()
                    || st
                        .last_seen
                        .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1800))
            });
        }
        snapshot.last_action_error = self.last_action_error.clone();

        // Only compute terminal output lazily while Detail is actively open.
        if allow_expanded_io {
            if let Some((ref proj_id, ref job_id)) = self.inspected_target {
                let proj_opt = snapshot.projects.iter().find(|p| &p.id == proj_id);
                let target_job = job_id
                    .as_ref()
                    .and_then(|jid| {
                        snapshot
                            .projects
                            .iter()
                            .flat_map(|p| p.jobs.iter())
                            .find(|j| &j.id == jid)
                    })
                    .or_else(|| proj_opt.and_then(|p| p.jobs.first()));
                let effective_job_id = job_id
                    .as_ref()
                    .or_else(|| target_job.map(|j| &j.id));
                let (pids, log_path): (Vec<u32>, Option<&std::path::Path>) =
                    if let Some(j) = target_job {
                        (
                            j.procs.iter().map(|pr| pr.key.pid).collect(),
                            j.log_path.as_deref(),
                        )
                    } else if let Some(p) = proj_opt {
                        (
                            p.procs.iter().map(|pr| pr.key.pid).collect(),
                            p.jobs.iter().find_map(|j| j.log_path.as_deref()),
                        )
                    } else {
                        (Vec::new(), None)
                    };

                snapshot.active_terminal = Some(inspect_terminal_output(
                    proj_id,
                    effective_job_id,
                    &pids,
                    log_path,
                    self.resolver.labels().rules(),
                    &self.managed_jobs,
                ));
            }
        }

        if self.show_all_processes && allow_expanded_io {
            snapshot.all_procs = collected
                .processes
                .iter()
                .map(|p| {
                    let net = self.last_ports.get(&p.key.pid);
                    let ports = net.map(|n| n.ports.clone()).unwrap_or_default();
                    let meta = self.resolver.cached_meta(&p.key);
                    ProcInfo {
                        key: p.key,
                        parent_pid: p.parent_pid,
                        name: p.image_name.clone(),
                        role: meta.map(|m| m.role).unwrap_or(ProcessRole::Unknown),
                        role_label: meta
                            .map(|m| m.job_title.clone())
                            .unwrap_or_else(|| p.image_name.clone()),
                        cmdline: meta.and_then(|m| m.sanitized_cmdline.clone()),
                        working_dir: meta.and_then(|m| m.working_dir.clone()),
                        cpu: p.cpu_percent,
                        ram: p.private_ram_bytes,
                        ports,
                        uptime_secs: p.uptime_secs,
                        is_helper_collapsed: meta.is_some_and(|m| m.is_wrapper_or_helper),
                        is_critical_system: meta.is_some_and(|m| m.is_critical_system)
                            || crate::engine::actions::is_hard_blacklisted_name(
                                &p.image_name.to_ascii_lowercase(),
                            )
                            || p.key.pid <= 4,
                        can_kill: meta.is_none_or(|m| !m.is_critical_system)
                            && is_kill_allowed_static(
                                p.key.pid,
                                p.session_id,
                                &p.image_name,
                                self_pid,
                                &ancestors,
                            ),
                        can_restart: meta.is_some_and(|m| m.restart_recipe.is_some()),
                        access_limited: meta.is_some_and(|m| m.access_limited),
                    }
                })
                .collect();
            snapshot.all_procs.sort_by(|a, b| {
                b.cpu
                    .partial_cmp(&a.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.ram.cmp(&a.ram))
            });
        }

        self.last_procs = collected.processes;

        let sig = (
            snapshot.total_projects,
            snapshot.total_jobs,
            snapshot.total_procs,
            snapshot.alerts.len(),
            (snapshot.total_dev_cpu + snapshot.system_cpu * 0.25).round() as i32,
            snapshot.total_dev_ram_bytes / (16 * 1024 * 1024),
            self.next_error_id,
        );
        let should_wake = match self.mode {
            PollingMode::ExpandedActive | PollingMode::HoverPeek => true,
            PollingMode::CollapsedIdle => sig != self.last_summary_sig || self.tick_counter <= 2,
            PollingMode::LowPowerLocked => !snapshot.alerts.is_empty() && sig != self.last_summary_sig,
        };
        self.last_summary_sig = sig;

        if let Ok(mut guard) = self.latest.lock() {
            *guard = Arc::new(snapshot);
        }
        if should_wake {
            (self.wake_cb)();
        }
    }

    fn update_restart_recipes(&mut self, procs: &[CollectedProcess], now: Instant) {
        let mut by_job: HashMap<JobId, Vec<(&CollectedProcess, Arc<ProcessMetadata>)>> =
            HashMap::new();
        for proc in procs {
            let Some(meta) = self.resolver.cached_meta(&proc.key).cloned() else {
                continue;
            };
            if meta.is_ignored_noise || meta.is_critical_system {
                continue;
            }
            by_job
                .entry(meta.job_id.clone())
                .or_default()
                .push((proc, meta));
        }

        let mut active_job_ids: HashSet<JobId> = HashSet::with_capacity(by_job.len());
        for (job_id, items) in by_job {
            active_job_ids.insert(job_id.clone());

            let mut job_ports: Vec<u16> = Vec::new();
            for (proc, _) in &items {
                if let Some(net) = self.last_ports.get(&proc.key.pid) {
                    for pb in &net.ports {
                        if pb.is_listening && !job_ports.contains(&pb.port) {
                            job_ports.push(pb.port);
                        }
                    }
                }
            }
            job_ports.sort_unstable();

            let non_wrapper_pids: HashMap<u32, i64> = items
                .iter()
                .filter(|(_, m)| !m.is_wrapper_or_helper)
                .map(|(p, _)| (p.key.pid, p.key.create_time_100ns))
                .collect();

            let canonical = items
                .iter()
                .filter(|(_, m)| m.restart_recipe.is_some() && !m.is_wrapper_or_helper)
                .min_by_key(|(p, _)| {
                    let parent_is_sibling = non_wrapper_pids
                        .get(&p.parent_pid)
                        .is_some_and(|&pct| pct <= p.key.create_time_100ns && p.parent_pid != p.key.pid);
                    (parent_is_sibling, p.key.create_time_100ns, p.key.pid)
                });

            if let Some((root_proc, root_meta)) = canonical {
                if let Some(recipe) = &root_meta.restart_recipe {
                    self.restart_recipes.insert(
                        job_id.clone(),
                        StoredRestartRecipe {
                            project_id: root_meta.project_id.clone(),
                            job_id,
                            primary_key: root_proc.key,
                            recipe: recipe.clone(),
                            ports: job_ports,
                            last_seen: now,
                        },
                    );
                }
            }
        }

        self.restart_recipes.retain(|jid, r| {
            active_job_ids.contains(jid)
                || now.saturating_duration_since(r.last_seen) < Duration::from_secs(900)
        });
    }

    fn collect_restart_recipes(
        &self,
        target: RestartTarget,
    ) -> (Vec<ProcessKey>, Vec<StoredRestartRecipe>) {
        match target {
            RestartTarget::Process(key) => {
                let job_id = self
                    .watched_meta
                    .get(&key)
                    .map(|w| w.meta.job_id.clone())
                    .or_else(|| {
                        self.restart_recipes
                            .values()
                            .find(|r| r.primary_key == key)
                            .map(|r| r.job_id.clone())
                    });
                let Some(jid) = job_id else {
                    return (Vec::new(), Vec::new());
                };
                let mut kill_keys: Vec<ProcessKey> = self
                    .watched_meta
                    .iter()
                    .filter(|(_, w)| w.meta.job_id == jid && !w.meta.is_critical_system)
                    .map(|(k, _)| *k)
                    .collect();
                if kill_keys.is_empty() && self.last_procs.iter().any(|p| p.key == key) {
                    kill_keys.push(key);
                }
                let recipes = self.restart_recipes.get(&jid).cloned().into_iter().collect();
                (kill_keys, recipes)
            }
            RestartTarget::Job(job_id) => {
                let kill_keys: Vec<ProcessKey> = self
                    .watched_meta
                    .iter()
                    .filter(|(_, w)| w.meta.job_id == job_id && !w.meta.is_critical_system)
                    .map(|(k, _)| *k)
                    .collect();
                let recipes = self
                    .restart_recipes
                    .get(&job_id)
                    .cloned()
                    .into_iter()
                    .collect();
                (kill_keys, recipes)
            }
            RestartTarget::Project(project_id) => {
                let mut active_project_jobs: HashSet<JobId> = HashSet::new();
                let kill_keys: Vec<ProcessKey> = self
                    .watched_meta
                    .iter()
                    .filter(|(_, w)| w.meta.project_id == project_id && !w.meta.is_critical_system)
                    .map(|(k, w)| {
                        active_project_jobs.insert(w.meta.job_id.clone());
                        *k
                    })
                    .collect();
                let mut recipes: Vec<StoredRestartRecipe> = self
                    .restart_recipes
                    .values()
                    .filter(|r| {
                        r.project_id == project_id
                            && (active_project_jobs.contains(&r.job_id)
                                || self.job_crashes.contains_key(&r.job_id))
                    })
                    .cloned()
                    .collect();
                recipes.sort_by(|a, b| a.job_id.0.cmp(&b.job_id.0));
                (kill_keys, recipes)
            }
        }
    }

    fn execute_restart_target_with<F>(
        &mut self,
        target: RestartTarget,
        async_port_wait: bool,
        mut spawn_sync: F,
    ) -> bool
    where
        F: FnMut(&crate::engine::project_resolver::RestartRecipe) -> Result<u32, String>,
    {
        let now = Instant::now();
        self.restarting_jobs
            .retain(|_, ts| now.saturating_duration_since(*ts) < Duration::from_secs(10));

        let (kill_keys, all_recipes) = self.collect_restart_recipes(target);
        let recipes: Vec<StoredRestartRecipe> = all_recipes
            .into_iter()
            .filter(|r| !self.restarting_jobs.contains_key(&r.job_id))
            .collect();
        if recipes.is_empty() {
            return false;
        }
        if !kill_keys.is_empty() {
            execute_safe_kill_many(&kill_keys, &self.last_procs, true, &mut self.user_kills);
        }

        let mut expected_ports: Vec<u16> = Vec::new();
        for rec in &recipes {
            for &port in &rec.ports {
                if !expected_ports.contains(&port) {
                    expected_ports.push(port);
                }
            }
        }

        if !async_port_wait || expected_ports.is_empty() {
            let mut errs = Vec::new();
            let mut any_ok = false;
            for rec in &recipes {
                match spawn_sync(&rec.recipe) {
                    Ok(_) => any_ok = true,
                    Err(err) => errs.push(err),
                }
            }
            for err in errs {
                self.record_action_error(err);
            }
            return any_ok;
        }

        let job_ids: Vec<JobId> = recipes.iter().map(|r| r.job_id.clone()).collect();
        let job_ids_cleanup = job_ids.clone();
        for jid in &job_ids {
            self.restarting_jobs.insert(jid.clone(), now);
        }
        let managed = Arc::clone(&self.managed_jobs);
        let tx = self.cmd_tx.clone();
        let spawned = thread::Builder::new()
            .name("bbstats-restart".to_string())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_millis(2000);
                while Instant::now() < deadline {
                    if !are_tcp_ports_listening(&expected_ports) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                for rec in &recipes {
                    if let Err(err) = managed.spawn_with_capture_env(
                        &rec.recipe.raw_cmdline,
                        &rec.recipe.working_dir,
                        &rec.recipe.env_vars,
                        rec.recipe.use_shell,
                    ) {
                        let _ = tx.send(EngineMsg::ActionError(err));
                    }
                }
                let _ = tx.send(EngineMsg::RestartFinished(job_ids));
                let _ = tx.send(EngineMsg::Cmd(EngineCmd::ForceRescan));
            });
        if spawned.is_err() {
            for jid in job_ids_cleanup {
                self.restarting_jobs.remove(&jid);
            }
            return false;
        }
        true
    }

    fn reconcile_vanished_watched(&mut self, current_keys: &HashSet<ProcessKey>, now: Instant) {
        self.vanished_since.retain(|k, _| !current_keys.contains(k));
        let vanished: Vec<ProcessKey> = self
            .watched_meta
            .keys()
            .filter(|k| !current_keys.contains(k))
            .copied()
            .collect();
        for key in vanished {
            if let Some(code) = self
                .exit_watcher
                .poll_exit_code(&key)
                .or_else(|| self.managed_jobs.take_exit_code(key.pid))
            {
                self.vanished_since.remove(&key);
                self.handle_process_exit(ExitEvent { key, exit_code: code });
            } else {
                let since = *self.vanished_since.entry(key).or_insert(now);
                if now.saturating_duration_since(since) >= Duration::from_secs(3) {
                    self.watched_meta.remove(&key);
                    self.vanished_since.remove(&key);
                    self.exit_watcher.unwatch(&key);
                }
            }
        }
    }

    fn start_custom_job(&mut self, job_id: &JobId) {
        let j_low = job_id.0.to_ascii_lowercase();
        let rules = self.resolver.labels().rules();
        // Prefer exact `custom:<needle>` match; fallback to longest `contains` match.
        let rule = rules
            .iter()
            .find(|r| {
                let needle = r.match_substr.trim().to_ascii_lowercase();
                !needle.is_empty()
                    && r.command.as_ref().is_some_and(|c| !c.trim().is_empty())
                    && j_low == format!("custom:{needle}")
            })
            .or_else(|| {
                rules
                    .iter()
                    .filter(|r| {
                        let needle = r.match_substr.trim().to_ascii_lowercase();
                        !needle.is_empty()
                            && r.command.as_ref().is_some_and(|c| !c.trim().is_empty())
                            && j_low.contains(&needle)
                    })
                    .max_by_key(|r| r.match_substr.trim().len())
            })
            .cloned();
        if let Some(rule) = rule {
            if let Some(ref cmd) = rule.command {
                let cwd = rule
                    .working_dir
                    .as_deref()
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                    .map(|w| PathBuf::from(expand_win_env_vars(w)))
                    .or_else(|| {
                        std::env::var_os("USERPROFILE")
                            .or_else(|| std::env::var_os("HOME"))
                            .map(PathBuf::from)
                    })
                    .unwrap_or_else(|| PathBuf::from("."));
                let extra_env = rule
                    .env_file
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(|p| crate::engine::labels::load_env_file_vars(&PathBuf::from(expand_win_env_vars(p))))
                    .unwrap_or_default();
                if let Err(err) = self.managed_jobs.spawn_with_capture_env(cmd, &cwd, &extra_env, true) {
                    self.record_action_error(err);
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::known_jobs::{KnownJobsSnapshot, KnownStoppedJob};
    use crate::model::{BindScope, PortBinding, ProjectStatus, TransportProto};
    use crate::engine::win_nt::{inspect_process_peb, PebInspection};

    #[test]
    fn win_nt_enumerates_self_and_reads_peb() {
        let mut collector = ProcessCollector::new();
        let tick = collector.collect_processes();
        let my_pid = current_pid();
        let me = tick
            .processes
            .iter()
            .find(|p| p.key.pid == my_pid)
            .expect("NtQuerySystemInformation must return current test process");
        assert!(me.private_ram_bytes > 0);
        assert!(tick.system_ram_total_bytes > 0);

        let peb = inspect_process_peb(my_pid);
        assert!(peb.peb_readable, "should read own PEB");
        assert!(peb.working_dir.is_some());
        assert!(peb.cmdline.is_some());
    }

    #[test]
    fn grouping_collapses_wrappers_into_jobs_and_applies_8s_debounce() {
        let mut resolver = ProjectResolver::new(999_999);
        resolver.set_labels_for_test(Vec::new());

        let mk = |pid: u32, ppid: u32, name: &str, uptime: u64| CollectedProcess {
            key: ProcessKey::new(pid, 1000 + pid as i64),
            parent_pid: ppid,
            session_id: 1,
            image_name: name.to_string(),
            cpu_percent: 2.0,
            private_ram_bytes: 20 * 1024 * 1024,
            uptime_secs: uptime,
        };

        // 1. `cmd /c node ...` + `conhost.exe` + `node.exe` => 1 Job with 3 procs!
        // 2. `.venv\Scripts\pythonw.exe` + child `pythonw.exe` => 1 Job with 2 procs!
        // 3. Short-lived 3s `python.exe -m studio.service --stop-bg` => debounced (< 8s)!
        let procs = vec![
            mk(100, 1, "cmd.exe", 120),
            mk(101, 100, "conhost.exe", 120),
            mk(102, 100, "node.exe", 120),
            mk(200, 1, "pythonw.exe", 60),
            mk(201, 200, "pythonw.exe", 60),
            mk(300, 1, "python.exe", 3),
        ];

        // Seed resolver meta directly via `resolve_from_peb` + pass 2 simulation
        let peb_cmd = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\asd\edge-proxy")),
            cmdline: Some(
                r"cmd /c node C:\Users\Dev\asd\edge-proxy\.token-rotator.js --port 18911"
                    .to_string(),
            ),
            peb_readable: true,
            ..Default::default()
        };
        let peb_node = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\asd\edge-proxy")),
            cmdline: Some(
                r"node C:\Users\Dev\asd\edge-proxy\.token-rotator.js --port 18911".to_string(),
            ),
            peb_readable: true,
            ..Default::default()
        };
        let peb_py_trampoline = PebInspection {
            image_path: Some(PathBuf::from(
                r"C:\MockUsers\Dev\Desktop\x\.venv\Scripts\pythonw.exe",
            )),
            working_dir: Some(PathBuf::from(r"C:\MockUsers\Dev\Desktop\x")),
            cmdline: Some(
                r"C:\MockUsers\Dev\Desktop\x\.venv\Scripts\pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_webhook_poller.py"
                    .to_string(),
            ),
            peb_readable: true,
            ..Default::default()
        };
        let peb_py_child = PebInspection {
            image_path: Some(PathBuf::from(r"C:\Python312\pythonw.exe")),
            working_dir: Some(PathBuf::from(r"C:\MockUsers\Dev\Desktop\x")),
            cmdline: Some(
                r"C:\Python312\pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_webhook_poller.py"
                    .to_string(),
            ),
            peb_readable: true,
            ..Default::default()
        };
        let peb_short = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\Desktop\new\studio")),
            cmdline: Some("python.exe -m studio.service --stop-bg".to_string()),
            peb_readable: true,
            ..Default::default()
        };

        let m_node = resolver.resolve_from_peb(&procs[2], true, Some("cmd.exe"), &peb_node);
        let mut m_cmd = resolver.resolve_from_peb(&procs[0], false, None, &peb_cmd);
        m_cmd.is_ignored_noise = false;
        m_cmd.is_wrapper_or_helper = true;
        m_cmd.job_id = m_node.job_id.clone();
        m_cmd.project_id = m_node.project_id.clone();
        m_cmd.project_display_name = m_node.project_display_name.clone();
        m_cmd.job_title = m_node.job_title.clone();

        let mut m_con = m_node.clone();
        m_con.image_name = "conhost.exe".to_string();
        m_con.is_wrapper_or_helper = true;

        let m_py1 = resolver.resolve_from_peb(&procs[3], false, None, &peb_py_trampoline);
        let m_py2 = resolver.resolve_from_peb(&procs[4], false, Some("pythonw.exe"), &peb_py_child);
        let m_short = resolver.resolve_from_peb(&procs[5], false, None, &peb_short);

        // Verify both python copies generated the exact same canonical JobId and project!
        assert_eq!(m_py1.job_id, m_py2.job_id);
        assert_eq!(m_py1.job_title, "Webhook poller");

        // Build snapshot using a test helper map via `resolve_tick`-compatible cache?
        // Instead, test `build_snapshot` directly by seeding `ProjectResolver`?
        // Let's keep `meta_cache` testable via a `#[cfg(test)]` helper on `ProjectResolver`!
        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_id = 1;
        let mut tail = LogTailCache::new();
        let known = KnownJobsSnapshot::default();
        let mut ports = HashMap::new();
        ports.insert(
            102,
            PidNetworkInfo {
                ports: vec![PortBinding {
                    port: 18911,
                    protocol: TransportProto::Tcp,
                    scope: BindScope::Localhost,
                    is_listening: true,
                    suggested_url: Some("http://localhost:18911".to_string()),
                }],
            },
        );

        // Use a dedicated resolver populated with our 6 synthetic entries:
        resolver.insert_test_meta(procs[0].key, m_cmd);
        resolver.insert_test_meta(procs[1].key, m_con);
        resolver.insert_test_meta(procs[2].key, m_node);
        resolver.insert_test_meta(procs[3].key, m_py1);
        resolver.insert_test_meta(procs[4].key, m_py2);
        resolver.insert_test_meta(procs[5].key, m_short);

        let snap = build_snapshot(
            &procs,
            10.0,
            4_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &ports,
            &resolver,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &[],
            &[],
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );

        // Short-lived 3s job (`studio`) must be debounced out!
        assert_eq!(snap.projects.len(), 2);
        assert_eq!(snap.total_jobs, 2);

        let proxy = snap.projects.iter().find(|p| p.name == "edge-proxy").unwrap();
        assert_eq!(proxy.jobs.len(), 1);
        assert_eq!(proxy.jobs[0].title, "Token rotator");
        assert_eq!(proxy.jobs[0].procs.len(), 3);
        assert_eq!(proxy.jobs[0].ports[0].port, 18911);
        assert_eq!(proxy.jobs[0].ram, 60 * 1024 * 1024);

        let x_proj = snap.projects.iter().find(|p| p.name == "X").unwrap();
        assert_eq!(x_proj.jobs.len(), 1);
        assert_eq!(x_proj.jobs[0].title, "Webhook poller");
        assert_eq!(x_proj.jobs[0].procs.len(), 2);
        assert_eq!(x_proj.jobs[0].ram, 40 * 1024 * 1024);
    }

    #[test]
    fn grouping_merges_stopped_known_jobs_and_watchdog_note() {
        let resolver = ProjectResolver::new(999_999);
        let bot_id = ProjectId(r"path:c:\users\dev\.omp\local\worker-bot".to_string());
        let mut known = KnownJobsSnapshot::default();
        known.jobs.push(KnownStoppedJob {
            job_id: JobId(format!("{}::script:auto_updater.py", bot_id.0)),
            project_id: bot_id.clone(),
            project_name: "WorkerBot".to_string(),
            title: "Auto update botu".to_string(),
            subtitle: r"WorkerBot › ...\worker-bot".to_string(),
            icon: Some("🤖".to_string()),
            color: Some("#F5B73B".to_string()),
            root: PathBuf::from(r"C:\Users\dev\.omp\local\worker-bot"),
            match_hint: "auto_updater.py".to_string(),
            last_log: Some("19:14 ok".to_string()),
            log_path: None,
            custom_command: None,
            is_watched: false,
        });
        known.watchdog_notes.insert(
            bot_id.clone(),
            "Watchdog · her 15 dk · son çalışma 19:14".to_string(),
        );

        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_id = 1;
        let mut tail = LogTailCache::new();
        let snap = build_snapshot(
            &[],
            5.0,
            4_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &resolver,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &[],
            &[],
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );

        assert_eq!(snap.projects.len(), 1);
        let bot = &snap.projects[0];
        assert_eq!(bot.name, "WorkerBot");
        assert_eq!(bot.status, ProjectStatus::Stopped);
        assert_eq!(
            bot.watchdog_note.as_deref(),
            Some("Watchdog · her 15 dk · son çalışma 19:14")
        );
        assert_eq!(bot.jobs.len(), 1);
        assert_eq!(bot.jobs[0].status, ProjectStatus::Stopped);
        assert_eq!(bot.jobs[0].title, "Auto update botu");
        assert_eq!(bot.jobs[0].last_log.as_deref(), Some("19:14 ok"));

        // Now simulate `auto_updater` running via state-file binding -> must NOT emit duplicate Stopped row!
        let mut resolver_active = ProjectResolver::new(999_999);
        resolver_active.insert_test_meta(
            ProcessKey::new(34968, 5000),
            ProcessMetadata {
                image_name: "python.exe".to_string(),
                working_dir: Some(PathBuf::from(r"C:\Users\dev\.omp\local\worker-bot")),
                sanitized_cmdline: None,
                access_limited: true,
                project_id: bot_id.clone(),
                project_display_name: "WorkerBot".to_string(),
                project_root: Some(PathBuf::from(r"C:\Users\dev\.omp\local\worker-bot")),
                git_branch: None,
                job_id: JobId(format!("{}::state:worker-bot\\auto_updater", bot_id.0)),
                job_title: "Auto update botu".to_string(),
                job_subtitle: r"WorkerBot › ...\worker-bot".to_string(),
                job_icon: Some("🤖".to_string()),
                job_color: Some("#F5B73B".to_string()),
                match_hint: r"worker-bot\auto_updater".to_string(),
                log_path: None,
                inline_last_log: Some("19:15 idle".to_string()),
                custom_command: None,
                is_watched: true,
                role: ProcessRole::WorkerOrQueue,
                role_label: "Auto update botu".to_string(),
                inference_source: crate::engine::project_resolver::InferenceSource::UserLabelRule,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            },
        );
        let active_proc = CollectedProcess {
            key: ProcessKey::new(34968, 5000),
            parent_pid: 1,
            session_id: 1,
            image_name: "python.exe".to_string(),
            cpu_percent: 0.2,
            private_ram_bytes: 32 * 1024 * 1024,
            uptime_secs: 600,
        };
        let snap2 = build_snapshot(
            &[active_proc],
            5.0,
            4_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &resolver_active,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &[],
            &[],
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );
        assert_eq!(snap2.projects.len(), 1);
        assert_eq!(snap2.projects[0].jobs.len(), 1, "must deduplicate active state-bound job vs known stopped job");
        assert_eq!(snap2.projects[0].jobs[0].status, ProjectStatus::Healthy);
    }

    #[test]
    fn crash_detection_ignores_user_kill_and_normal_exit_but_alerts_on_crash() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);

        let mk_meta = |name: &str| {
            Arc::new(ProcessMetadata {
                image_name: name.to_string(),
                working_dir: None,
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("path:c:\\dev\\api".to_string()),
                project_display_name: "acme-api".to_string(),
                project_root: Some(PathBuf::from(r"C:\dev\api")),
                git_branch: None,
                job_id: JobId("path:c:\\dev\\api::uvicorn".to_string()),
                job_title: "Uvicorn API".to_string(),
                job_subtitle: "acme-api".to_string(),
                job_icon: None,
                job_color: None,
                match_hint: "uvicorn".to_string(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::ApiBackend,
                role_label: "Uvicorn".to_string(),
                inference_source: crate::engine::project_resolver::InferenceSource::ManifestPyProject,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            })
        };

        let k_user = ProcessKey::new(10, 1);
        rt.watched_meta.insert(
            k_user,
            WatchedProcessSnapshot {
                meta: mk_meta("python.exe"),
                had_port: true,
                uptime_secs: 30,
                ports: vec![8000],
            },
        );
        rt.user_kills.insert(k_user, Instant::now());
        rt.handle_process_exit(ExitEvent {
            key: k_user,
            exit_code: 1,
        });
        assert!(rt.alerts.is_empty());

        let k_ctrlc = ProcessKey::new(11, 2);
        rt.watched_meta.insert(
            k_ctrlc,
            WatchedProcessSnapshot {
                meta: mk_meta("python.exe"),
                had_port: true,
                uptime_secs: 30,
                ports: vec![8000],
            },
        );
        rt.handle_process_exit(ExitEvent {
            key: k_ctrlc,
            exit_code: 0xC000_013A_u32 as i32,
        });
        assert!(rt.alerts.is_empty());

        let k_crash = ProcessKey::new(12, 3);
        rt.watched_meta.insert(
            k_crash,
            WatchedProcessSnapshot {
                meta: mk_meta("python.exe"),
                had_port: true,
                uptime_secs: 45,
                ports: vec![8000],
            },
        );
        rt.handle_process_exit(ExitEvent {
            key: k_crash,
            exit_code: 101,
        });
        assert_eq!(rt.alerts.len(), 1);
        assert!(matches!(
            rt.alerts[0].kind,
            AlertKind::Crash {
                pid: 12,
                exit_code: 101,
                ..
            }
        ));
        assert!(!rt.job_crashes.is_empty());
    }

    #[test]
    fn custom_watched_rule_appears_when_stopped_and_respects_custom_sort_order() {
        let mut resolver = ProjectResolver::new(999_999);
        resolver.set_labels_for_test(vec![
            LabelRule {
                match_substr: "alpha.py".to_string(),
                project: Some("Alpha".to_string()),
                job: "Alpha Worker".to_string(),
                command: Some("python alpha.py".to_string()),
                watch: Some(true),
                ..Default::default()
            },
            LabelRule {
                match_substr: "beta.py".to_string(),
                project: Some("Beta".to_string()),
                job: "Beta Worker".to_string(),
                watch: Some(true),
                ..Default::default()
            },
        ]);

        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_id = 1;
        let mut tail = LogTailCache::new();
        let known = KnownJobsSnapshot::default();
        let order = vec!["rule:beta".to_string(), "rule:alpha".to_string()];

        let snap = build_snapshot(
            &[],
            1.0,
            2_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &resolver,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &order,
            &[],
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );

        assert_eq!(snap.projects.len(), 2);
        assert_eq!(snap.projects[0].name, "Beta");
        assert_eq!(snap.projects[1].name, "Alpha");
        assert!(snap.projects[1].jobs[0].can_restart);
        assert_eq!(
            snap.projects[1].jobs[0].custom_command.as_deref(),
            Some("python alpha.py")
        );
    }

    #[test]
    fn restarting_crashed_job_clears_crash_badge_and_sibling_exit_does_not_crash_running_job() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);

        let proj_id = ProjectId("path:c:\\dev\\bot".to_string());
        let job_id = JobId("path:c:\\dev\\bot::updater".to_string());
        let meta = Arc::new(ProcessMetadata {
            image_name: "pythonw.exe".to_string(),
            working_dir: None,
            sanitized_cmdline: None,
            access_limited: false,
            project_id: proj_id.clone(),
            project_display_name: "WorkerBot".to_string(),
            project_root: Some(PathBuf::from(r"C:\dev\bot")),
            git_branch: None,
            job_id: job_id.clone(),
            job_title: "Auto update botu".to_string(),
            job_subtitle: "WorkerBot".to_string(),
            job_icon: None,
            job_color: None,
            match_hint: "auto_updater.py".to_string(),
            log_path: None,
            inline_last_log: None,
            custom_command: None,
            is_watched: false,
            role: ProcessRole::ApiBackend,
            role_label: "Bot".to_string(),
            inference_source: crate::engine::project_resolver::InferenceSource::ManifestPyProject,
            is_ignored_noise: false,
            is_wrapper_or_helper: false,
            is_critical_system: false,
            restart_recipe: None,
            auto_restart: false,
        });

        // 1. Sibling non-compiler process still alive -> helper exit must not mark job crashed.
        let k_main = ProcessKey::new(201, 1);
        let k_child = ProcessKey::new(202, 2);
        let snap_proc = |m: &Arc<ProcessMetadata>| WatchedProcessSnapshot {
            meta: m.clone(),
            had_port: false,
            uptime_secs: 60,
            ports: vec![],
        };
        rt.watched_meta.insert(k_main, snap_proc(&meta));
        rt.watched_meta.insert(k_child, snap_proc(&meta));
        rt.handle_process_exit(ExitEvent {
            key: k_child,
            exit_code: 1,
        });
        assert!(rt.job_crashes.is_empty());
        assert!(rt.alerts.is_empty());

        // 2. Last process exits with non-zero -> marks crashed; seeing running process clears it.
        rt.handle_process_exit(ExitEvent {
            key: k_main,
            exit_code: 1,
        });
        assert!(rt.job_crashes.contains_key(&job_id));
        assert!(rt.project_states[&proj_id].crashed_until.is_some());
        assert!(!rt.alerts[0].dismissed);

        rt.clear_job_crash(&meta);
        assert!(rt.job_crashes.is_empty());
        assert!(rt.project_states[&proj_id].crashed_until.is_none());
        assert!(rt.alerts[0].dismissed);
    }

    #[test]
    fn crashed_job_persists_canonical_recipe_and_appears_in_snapshot_without_double_spawn() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);

        let proj_id = ProjectId("path:c:\\dev\\api".to_string());
        let job_id = JobId("path:c:\\dev\\api::script:run.py".to_string());
        let mk_meta = |cmd: &str| {
            Arc::new(ProcessMetadata {
                image_name: "pythonw.exe".to_string(),
                working_dir: Some(PathBuf::from(r"C:\dev\api")),
                sanitized_cmdline: Some(cmd.to_string()),
                access_limited: false,
                project_id: proj_id.clone(),
                project_display_name: "Acme API".to_string(),
                project_root: Some(PathBuf::from(r"C:\dev\api")),
                git_branch: None,
                job_id: job_id.clone(),
                job_title: "API Worker".to_string(),
                job_subtitle: "Acme API".to_string(),
                job_icon: None,
                job_color: None,
                match_hint: "run.py".to_string(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::ApiBackend,
                role_label: "API".to_string(),
                inference_source: crate::engine::project_resolver::InferenceSource::ManifestPyProject,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: Some(RestartRecipe {
                    raw_cmdline: cmd.to_string(),
                    working_dir: PathBuf::from(r"C:\dev\api"),
                    env_vars: Vec::new(),
                    use_shell: false,
                }),
                auto_restart: false,
            })
        };

        let p_root = CollectedProcess {
            key: ProcessKey::new(501, 1000),
            parent_pid: 1,
            session_id: 1,
            image_name: "pythonw.exe".to_string(),
            cpu_percent: 1.0,
            private_ram_bytes: 20 * 1024 * 1024,
            uptime_secs: 60,
        };
        let p_child = CollectedProcess {
            key: ProcessKey::new(502, 1050),
            parent_pid: 501,
            session_id: 1,
            image_name: "pythonw.exe".to_string(),
            cpu_percent: 2.0,
            private_ram_bytes: 30 * 1024 * 1024,
            uptime_secs: 60,
        };

        let m_root = mk_meta(r"C:\dev\api\.venv\Scripts\pythonw.exe run.py");
        let m_child = mk_meta(r"C:\Python312\pythonw.exe run.py");
        rt.resolver.insert_test_meta(p_root.key, (*m_root).clone());
        rt.resolver.insert_test_meta(p_child.key, (*m_child).clone());
        let snap_proc = |m: &Arc<ProcessMetadata>| WatchedProcessSnapshot {
            meta: m.clone(),
            had_port: true,
            uptime_secs: 60,
            ports: vec![8000],
        };
        rt.watched_meta.insert(p_root.key, snap_proc(&m_root));
        rt.watched_meta.insert(p_child.key, snap_proc(&m_child));

        let procs = vec![p_root.clone(), p_child.clone()];
        let now = Instant::now();
        rt.update_restart_recipes(&procs, now);
        assert_eq!(rt.restart_recipes.len(), 1);
        assert_eq!(rt.restart_recipes[&job_id].primary_key, p_root.key);

        // Both processes crash -> watched_meta is empty.
        rt.handle_process_exit(ExitEvent {
            key: p_child.key,
            exit_code: 1,
        });
        rt.handle_process_exit(ExitEvent {
            key: p_root.key,
            exit_code: 1,
        });
        assert!(rt.watched_meta.is_empty());

        // Crashed job must appear in Snapshot and survive `filter_project_jobs`.
        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_id = 1;
        let mut tail = LogTailCache::new();
        let snap = build_snapshot(
            &[],
            2.0,
            1_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &rt.resolver,
            &KnownJobsSnapshot::default(),
            &mut tail,
            &rt.job_crashes,
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &[],
            &[],
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.projects[0].status, ProjectStatus::Crashed);
        assert_eq!(snap.projects[0].jobs.len(), 1);
        assert_eq!(snap.projects[0].jobs[0].status, ProjectStatus::Crashed);
        assert_eq!(
            snap.projects[0].jobs[0].crash_summary.as_deref(),
            Some("Crashed · code 1")
        );
        assert!(snap.projects[0].jobs[0].can_restart);
        let filtered = crate::ui::layout::filter_project_jobs(&snap, "acme");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.len(), 1);

        // RestartProject & RestartJob must still respawn root recipe exactly once despite empty watched_meta!
        let mut spawned: Vec<String> = Vec::new();
        rt.execute_restart_target_with(RestartTarget::Project(proj_id), false, |rec| {
            spawned.push(rec.raw_cmdline.clone());
            Ok(9001)
        });
        assert_eq!(spawned, vec![r"C:\dev\api\.venv\Scripts\pythonw.exe run.py"]);

        spawned.clear();
        rt.execute_restart_target_with(RestartTarget::Job(job_id.clone()), false, |rec| {
            spawned.push(rec.raw_cmdline.clone());
            Ok(9002)
        });
        assert_eq!(spawned, vec![r"C:\dev\api\.venv\Scripts\pythonw.exe run.py"]);

        // TTL pruning evicts stale inactive recipes after 15 minutes.
        rt.restart_recipes.get_mut(&job_id).unwrap().last_seen =
            Instant::now() - Duration::from_secs(1000);
        rt.update_restart_recipes(&[], Instant::now());
        assert!(rt.restart_recipes.is_empty());
    }

    #[test]
    fn hidden_projects_sync_hides_and_restores_in_snapshot() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);

        rt.resolver.set_labels_for_test(vec![LabelRule {
            match_substr: "worker.py".to_string(),
            project: Some("HiddenProj".to_string()),
            job: "Worker".to_string(),
            command: Some("python worker.py".to_string()),
            watch: Some(true),
            ..Default::default()
        }]);

        let pid = ProjectId("rule:hiddenproj".to_string());
        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_id = 1;
        let mut tail = LogTailCache::new();
        let known = KnownJobsSnapshot::default();

        // SyncConfigState hides matching project ID case-insensitively.
        rt.handle_msg(EngineMsg::Cmd(EngineCmd::SyncConfigState {
            pinned_projects: Vec::new(),
            hidden_projects: vec![pid.0.to_uppercase()],
            project_aliases: HashMap::new(),
            project_order: Vec::new(),
            job_order: Vec::new(),
            sort_mode: SortMode::Custom,
            language: Language::En,
        }));
        let snap_hidden = build_snapshot(
            &[],
            1.0,
            1_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &rt.resolver,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &rt.pinned_keys,
            &rt.hidden_keys,
            &rt.alias_keys,
            rt.sort_mode,
            &rt.project_order,
            &rt.job_order,
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );
        assert!(snap_hidden.projects.is_empty());

        // Restore clears hidden_keys and makes project visible again.
        rt.handle_msg(EngineMsg::Cmd(EngineCmd::SyncConfigState {
            pinned_projects: Vec::new(),
            hidden_projects: Vec::new(),
            project_aliases: HashMap::new(),
            project_order: Vec::new(),
            job_order: Vec::new(),
            sort_mode: SortMode::Custom,
            language: Language::En,
        }));
        let snap_restored = build_snapshot(
            &[],
            1.0,
            1_000_000_000,
            16_000_000_000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &rt.resolver,
            &known,
            &mut tail,
            &HashMap::new(),
            &mut states,
            &rt.pinned_keys,
            &rt.hidden_keys,
            &rt.alias_keys,
            rt.sort_mode,
            &rt.project_order,
            &rt.job_order,
            &mut alerts,
            &mut next_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );
        assert_eq!(snap_restored.projects.len(), 1);
        assert_eq!(snap_restored.projects[0].name, "HiddenProj");
    }

    #[test]
    fn auto_restart_retries_up_to_three_times_in_ten_minutes_then_halts() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);
        let job_id = JobId("path:c:\\dev\\api::worker".to_string());

        let t0 = Instant::now();
        assert!(rt.record_and_should_auto_restart(&job_id, t0));
        assert!(rt.record_and_should_auto_restart(&job_id, t0 + Duration::from_secs(10)));
        assert!(rt.record_and_should_auto_restart(&job_id, t0 + Duration::from_secs(20)));
        assert!(!rt.record_and_should_auto_restart(&job_id, t0 + Duration::from_secs(30)));

        // Expired attempts (> 10 minutes old) are pruned so auto-restart works again later.
        assert!(rt.record_and_should_auto_restart(&job_id, t0 + Duration::from_secs(601)));
    }

    #[test]
    fn reconcile_vanished_watched_preserves_meta_until_grace_or_exit() {
        let latest = Arc::new(Mutex::new(Arc::new(Snapshot::default())));
        let wake: WakeCallback = Arc::new(|| {});
        let (tx, _rx) = mpsc::channel();
        let mut rt = EngineRuntime::new(latest, wake, tx);

        let key = ProcessKey::new(777_001, 42);
        let key_stale = ProcessKey::new(777_002, 43);
        let mk_snap = |job: &str| WatchedProcessSnapshot {
            meta: Arc::new(ProcessMetadata {
                image_name: "python.exe".to_string(),
                working_dir: None,
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("path:c:\\dev\\race".to_string()),
                project_display_name: "RaceProj".to_string(),
                project_root: None,
                git_branch: None,
                job_id: JobId(job.to_string()),
                job_title: "Worker".to_string(),
                job_subtitle: "RaceProj".to_string(),
                job_icon: None,
                job_color: None,
                match_hint: "worker.py".to_string(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: true,
                role: ProcessRole::WorkerOrQueue,
                role_label: "Worker".to_string(),
                inference_source: crate::engine::project_resolver::InferenceSource::ManifestPyProject,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            }),
            had_port: true,
            uptime_secs: 30,
            ports: vec![9000],
        };
        rt.watched_meta.insert(key, mk_snap("job::1"));
        rt.watched_meta.insert(key_stale, mk_snap("job::2"));

        let empty = HashSet::new();
        let t0 = Instant::now();
        rt.reconcile_vanished_watched(&empty, t0);
        // Within 3s grace period, watched_meta is NOT prematurely wiped.
        assert!(rt.watched_meta.contains_key(&key));
        assert!(rt.watched_meta.contains_key(&key_stale));

        // Late exit event arrives 500ms later -> crash recorded!
        rt.reconcile_vanished_watched(&empty, t0 + Duration::from_millis(500));
        rt.handle_process_exit(ExitEvent { key, exit_code: 1 });
        assert!(rt.job_crashes.contains_key(&JobId("job::1".to_string())));
        assert_eq!(rt.alerts.len(), 1);

        // Stale vanished entry with no exit event is evicted once 3s grace expires.
        rt.reconcile_vanished_watched(&empty, t0 + Duration::from_secs(4));
        assert!(!rt.watched_meta.contains_key(&key_stale));
    }
}
