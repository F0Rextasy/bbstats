//! Shared data model between the background process engine and the native UI.
//!
//! Keep this module pure `std` with no UI or Win32 types so both `engine` and
//! `ui`/`render` layers can compile against a stable contract.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

/// Unique process identifier protected against Windows PID reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: u32,
    pub create_time_100ns: i64,
}

impl ProcessKey {
    #[inline]
    pub const fn new(pid: u32, create_time_100ns: i64) -> Self {
        Self {
            pid,
            create_time_100ns,
        }
    }
}

/// Stable identifier for a grouped project (normalized root path, docker compose name, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ProjectId(pub String);

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable identifier for a logical Job (one script / entrypoint process tree within a project).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobId(pub String);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Adaptive polling state controlled by UI visibility and OS state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PollingMode {
    /// Collapsed pill on desktop (default ~2500 ms).
    #[default]
    CollapsedIdle,
    /// Mouse hovering over pill (~1000 ms).
    HoverPeek,
    /// Panel expanded and visible (~600 ms).
    ExpandedActive,
    /// Fullscreen game/video or locked workstation (~5000 ms).
    LowPowerLocked,
}

/// Overall health status of a job or project group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectStatus {
    #[default]
    Healthy,
    HighLoad,
    Crashed,
    Stopped,
}

/// Display language for UI views, tray menu, and notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    En,
    Tr,
}

impl Language {
    #[inline]
    pub const fn pick<'a>(self, en: &'a str, tr: &'a str) -> &'a str {
        match self {
            Self::En => en,
            Self::Tr => tr,
        }
    }
}

/// Sort order for projects and jobs in the main panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    Custom,
    Cpu,
    Ram,
    Name,
    Uptime,
}

impl SortMode {
    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Custom => lang.pick("Custom", "Özel"),
            Self::Cpu => "CPU ↓",
            Self::Ram => "RAM ↓",
            Self::Name => lang.pick("Name A-Z", "Ad A-Z"),
            Self::Uptime => lang.pick("Time ↓", "Süre ↓"),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Custom => Self::Cpu,
            Self::Cpu => Self::Ram,
            Self::Ram => Self::Name,
            Self::Name => Self::Uptime,
            Self::Uptime => Self::Custom,
        }
    }
}

/// Origin of captured terminal/log output for an inspected job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalSourceKind {
    ManagedPipe,
    LogFile,
    ConsoleBuffer,
    StateJson,
    #[default]
    None,
}

impl TerminalSourceKind {
    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::ManagedPipe => lang.pick("Live Pipe", "Canlı Pipe"),
            Self::LogFile => lang.pick("Log File", "Log Dosyası"),
            Self::ConsoleBuffer => lang.pick("Console Buffer", "Konsol Tamponu"),
            Self::StateJson => "State Log",
            Self::None => lang.pick("No Output", "Çıktı Yok"),
        }
    }
}

/// Bounded terminal tail snapshot populated ONLY while a Detail view is open.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TerminalOutput {
    pub project_id: ProjectId,
    pub job_id: Option<JobId>,
    pub source_kind: TerminalSourceKind,
    pub source_hint: Option<String>,
    pub lines: Vec<String>,
}

/// Semantic role inferred for a process within a project.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessRole {
    DevServer,
    ApiBackend,
    DatabaseOrCache,
    WorkerOrQueue,
    CompilerOrBuild,
    IdeTooling,
    Container,
    TerminalShell,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportProto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindScope {
    Localhost,
    AllInterfaces,
    SpecificIp,
}

/// Listening or bound network port owned by a process or job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    pub port: u16,
    pub protocol: TransportProto,
    pub scope: BindScope,
    pub is_listening: bool,
    pub suggested_url: Option<String>,
}

/// Per-process telemetry and inferred metadata (underlying raw process).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub key: ProcessKey,
    pub parent_pid: u32,
    pub name: String,
    pub role: ProcessRole,
    pub role_label: String,
    pub cmdline: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub cpu: f32,
    pub ram: u64,
    pub ports: Vec<PortBinding>,
    pub uptime_secs: u64,
    pub is_helper_collapsed: bool,
    pub is_critical_system: bool,
    pub can_kill: bool,
    pub can_restart: bool,
    pub access_limited: bool,
}

/// A human-readable background Job (collapsed process tree for a single script/entrypoint).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub project: ProjectId,
    pub project_name: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub procs: Vec<ProcInfo>,
    pub cpu: f32,
    pub ram: u64,
    pub ports: Vec<PortBinding>,
    pub uptime: u64,
    pub status: ProjectStatus,
    pub last_log: Option<String>,
    pub crash_summary: Option<String>,
    pub root: Option<PathBuf>,
    pub primary_key: ProcessKey,
    pub match_hint: String,
    pub log_path: Option<PathBuf>,
    pub custom_command: Option<String>,
    pub is_watched: bool,
    pub can_kill: bool,
    pub can_restart: bool,
}

/// A logical project grouping one or more related Jobs.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectGroup {
    pub id: ProjectId,
    pub name: String,
    pub root: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub pinned: bool,
    pub status: ProjectStatus,
    pub cpu: f32,
    pub ram: u64,
    pub ports: Vec<PortBinding>,
    pub jobs: Vec<Job>,
    pub watchdog_note: Option<String>,
    pub procs: Vec<ProcInfo>,
    pub cpu_history: [u8; 30],
    pub ram_history_mb: [u16; 30],
}

/// Kind of alert raised by the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum AlertKind {
    Crash {
        pid: u32,
        process_name: String,
        exit_code: i32,
        can_restart: bool,
    },
    HighCpu {
        cpu_percent: f32,
        duration_secs: u64,
    },
    MemorySpike {
        ram_bytes: u64,
    },
}

/// User-facing alert banner / pill notification.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Alert {
    pub id: u64,
    pub project_id: ProjectId,
    pub project_name: String,
    pub kind: AlertKind,
    pub message: String,
    pub timestamp_ms: u64,
    pub dismissed: bool,
}

/// Immutable telemetry snapshot published by the background engine.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub timestamp_ms: u64,
    pub polling_mode: PollingMode,
    pub total_projects: u16,
    pub total_jobs: u16,
    pub total_procs: u16,
    pub total_dev_cpu: f32,
    pub total_dev_ram_bytes: u64,
    pub system_cpu: f32,
    pub system_ram_used_bytes: u64,
    pub system_ram_total_bytes: u64,
    pub system_cpu_history: [u8; 24],
    pub system_ram_history: [u8; 24],
    pub projects: Vec<ProjectGroup>,
    pub alerts: Vec<Alert>,
    /// Populated only when `EngineCmd::SetShowAllProcesses(true)` is active while expanded.
    pub all_procs: Vec<ProcInfo>,
    /// Populated only when a Detail view is actively inspecting a job/project.
    pub active_terminal: Option<TerminalOutput>,
    /// Monotonically-numbered latest action/spawn error surfaced to the UI.
    pub last_action_error: Option<(u64, String)>,
}

/// Commands sent from the UI thread to the background engine thread.
#[derive(Debug, Clone)]
pub enum EngineCmd {
    /// Switch adaptive polling preset.
    SetPollingMode(PollingMode),
    /// Override polling intervals in milliseconds (`None` keeps existing value).
    SetInterval {
        collapsed_ms: Option<u64>,
        expanded_ms: Option<u64>,
    },
    /// Kill a single process or its entire descendant subtree (respects critical-process blacklist).
    Kill {
        key: ProcessKey,
        tree: bool,
    },
    /// Kill all processes belonging to a specific Job.
    KillJob(JobId),
    /// Kill all killable processes belonging to a project group.
    KillProject(ProjectId),
    /// Restart a previously tracked process (or container) if a restart recipe is available.
    #[allow(dead_code)]
    Restart(ProcessKey),
    /// Restart the primary restartable process of a Job.
    RestartJob(JobId),
    /// Restart the primary restartable process(es) of a project.
    RestartProject(ProjectId),
    /// Sync persisted UI ordering, pins, hidden projects, aliases, sort mode, and language to the engine.
    SyncConfigState {
        pinned_projects: Vec<String>,
        hidden_projects: Vec<String>,
        project_aliases: HashMap<String, String>,
        project_order: Vec<String>,
        job_order: Vec<String>,
        sort_mode: SortMode,
        language: Language,
    },
    /// Pin or unpin a project to keep it at the top of the list.
    SetPinned {
        project_id: ProjectId,
        pinned: bool,
    },
    /// Rename a project locally.
    SetAlias {
        project_id: ProjectId,
        custom_name: Option<String>,
    },
    /// Hide a project group from the default view.
    HideProject(ProjectId),
    /// Dismiss an active alert by id.
    DismissAlert(u64),
    /// Toggle populating `Snapshot::all_procs` for the secondary "Show all processes" view.
    SetShowAllProcesses(bool),
    /// Set or clear the currently inspected job/project for lazy terminal tail reading.
    InspectTerminal(Option<(ProjectId, Option<JobId>)>),
    /// Save a custom rename / watch / launch rule to `%APPDATA%\bbstats\labels.toml` and apply immediately.
    SaveLabelRule {
        match_substr: String,
        project: Option<String>,
        job: String,
        icon: Option<String>,
        color: Option<String>,
        command: Option<String>,
        working_dir: Option<String>,
        log_file: Option<String>,
        watch: Option<bool>,
    },
    /// Remove a user rule from `%APPDATA%\bbstats\labels.toml` by `match_substr`.
    RemoveLabelRule(String),
    /// Start a custom or stopped job configured with a launch command.
    StartCustomJob(JobId),
    /// Wake the engine immediately and force a fresh scan.
    ForceRescan,
    /// Gracefully stop the background engine thread.
    Shutdown,
}

/// Unicode and Turkish-aware case folding for search and substring matching
/// (`İ` / `I` / `ı` fold to `i`; `Ö/Ü/Ş/Ğ/Ç` fold to `ö/ü/ş/ğ/ç`).
pub fn fold_search_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'İ' | 'I' | 'ı' => out.push('i'),
            '\u{0307}' => {}
            _ => {
                for lc in ch.to_lowercase() {
                    out.push(lc);
                }
            }
        }
    }
    out
}
