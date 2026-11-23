use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::engine::actions::is_kill_allowed_static;
use crate::engine::collector::{CollectedProcess, PidNetworkInfo};
use crate::engine::known_jobs::KnownJobsSnapshot;
use crate::engine::labels::expand_win_env_vars;
use crate::engine::project_resolver::{LogTailCache, ProjectResolver};
use crate::model::{
    Alert, AlertKind, BindScope, Job, JobId, Language, PollingMode, PortBinding, ProcInfo,
    ProcessKey, ProcessRole, ProjectGroup, ProjectId, ProjectStatus, Snapshot, SortMode,
    TransportProto,
};

const SHORT_JOB_DEBOUNCE_SECS: u64 = 8;

#[derive(Debug, Clone, Default)]
pub(super) struct ProjectRuntimeState {
    pub(super) display_name_override: Option<String>,
    pub(super) pinned: bool,
    pub(super) hidden: bool,
    pub(super) last_known_name: String,
    pub(super) last_known_root: Option<PathBuf>,
    pub(super) last_known_branch: Option<String>,
    pub(super) last_seen: Option<Instant>,
    pub(super) cpu_history: [u8; 30],
    pub(super) ram_history_mb: [u16; 30],
    pub(super) high_cpu_since: Option<Instant>,
    pub(super) ram_baseline: Option<(Instant, u64)>,
    pub(super) crashed_until: Option<Instant>,
    pub(super) last_high_cpu_alert: Option<Instant>,
    pub(super) last_mem_alert: Option<Instant>,
}


#[derive(Debug, Clone)]
pub(super) struct CrashedJobRecord {
    pub(super) until: Instant,
    pub(super) exit_code: i32,
    pub(super) project_id: ProjectId,
    pub(super) project_name: String,
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) icon: Option<String>,
    pub(super) color: Option<String>,
    pub(super) root: Option<PathBuf>,
    pub(super) primary_key: ProcessKey,
    pub(super) match_hint: String,
    pub(super) log_path: Option<PathBuf>,
    pub(super) custom_command: Option<String>,
    pub(super) is_watched: bool,
    pub(super) can_restart: bool,
}

/// Pure Job-first grouping, 8s debounce, log tailing, known-stopped merging, and alert evaluation.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_snapshot(
    procs: &[CollectedProcess],
    system_cpu: f32,
    system_ram_used_bytes: u64,
    system_ram_total_bytes: u64,
    system_cpu_history: [u8; 24],
    system_ram_history: [u8; 24],
    ports_by_pid: &HashMap<u32, PidNetworkInfo>,
    resolver: &ProjectResolver,
    known_snap: &KnownJobsSnapshot,
    log_tail_cache: &mut LogTailCache,
    job_crashes: &HashMap<JobId, CrashedJobRecord>,
    project_states: &mut HashMap<ProjectId, ProjectRuntimeState>,
    pinned_keys: &HashSet<String>,
    hidden_keys: &HashSet<String>,
    alias_keys: &HashMap<String, String>,
    sort_mode: SortMode,
    project_order: &[String],
    job_order: &[String],
    alerts: &mut Vec<Alert>,
    next_alert_id: &mut u64,
    polling_mode: PollingMode,
    self_pid: u32,
    ancestors: &HashSet<u32>,
    lang: Language,
) -> Snapshot {
    let now = Instant::now();
    let now_ms = unix_now_ms();
    let allow_log_io = matches!(polling_mode, PollingMode::ExpandedActive);
    let is_hidden = |pid: &ProjectId, states: &HashMap<ProjectId, ProjectRuntimeState>| {
        hidden_keys.contains(&pid.0.to_ascii_lowercase())
            || states.get(pid).is_some_and(|s| s.hidden)
    };

    struct JobAcc {
        id: JobId,
        project: ProjectId,
        project_name: String,
        title: String,
        subtitle: String,
        icon: Option<String>,
        color: Option<String>,
        root: Option<PathBuf>,
        git_branch: Option<String>,
        match_hint: String,
        log_path: Option<PathBuf>,
        inline_last_log: Option<String>,
        inline_last_log_mtime: Option<SystemTime>,
        custom_command: Option<String>,
        is_watched: bool,
        procs: Vec<ProcInfo>,
        cpu: f32,
        ram: u64,
        ports: Vec<PortBinding>,
        uptime: u64,
        primary_key: ProcessKey,
        primary_is_wrapper: bool,
        can_kill: bool,
        can_restart: bool,
        has_non_compiler: bool,
    }

    struct ProjectAcc {
        name: String,
        root: Option<PathBuf>,
        git_branch: Option<String>,
        icon: Option<String>,
        color: Option<String>,
        jobs: Vec<Job>,
        procs: Vec<ProcInfo>,
        ports: Vec<PortBinding>,
        cpu: f32,
        ram: u64,
        proc_count: u16,
        has_non_compiler: bool,
    }

    let mut job_map: HashMap<JobId, JobAcc> = HashMap::new();

    for proc in procs {
        let Some(meta) = resolver.cached_meta(&proc.key) else {
            continue;
        };
        if meta.is_ignored_noise {
            continue;
        }
        if is_hidden(&meta.project_id, project_states) {
            continue;
        }

        let net = ports_by_pid.get(&proc.key.pid);
        let proc_ports = net.map(|n| n.ports.clone()).unwrap_or_default();
        let can_kill = !meta.is_critical_system
            && is_kill_allowed_static(
                proc.key.pid,
                proc.session_id,
                &proc.image_name,
                self_pid,
                ancestors,
            );

        let info = ProcInfo {
            key: proc.key,
            parent_pid: proc.parent_pid,
            name: meta.image_name.clone(),
            role: meta.role,
            role_label: meta.role_label.clone(),
            cmdline: meta.sanitized_cmdline.clone(),
            working_dir: meta.working_dir.clone(),
            cpu: proc.cpu_percent,
            ram: proc.private_ram_bytes,
            ports: proc_ports.clone(),
            uptime_secs: proc.uptime_secs,
            is_helper_collapsed: meta.is_wrapper_or_helper,
            is_critical_system: meta.is_critical_system,
            can_kill,
            can_restart: meta.restart_recipe.is_some(),
            access_limited: meta.access_limited,
        };

        let acc = job_map.entry(meta.job_id.clone()).or_insert_with(|| JobAcc {
            id: meta.job_id.clone(),
            project: meta.project_id.clone(),
            project_name: meta.project_display_name.clone(),
            title: meta.job_title.clone(),
            subtitle: meta.job_subtitle.clone(),
            icon: meta.job_icon.clone(),
            color: meta.job_color.clone(),
            root: meta.project_root.clone(),
            git_branch: meta.git_branch.clone(),
            match_hint: meta.match_hint.clone(),
            log_path: meta.log_path.clone(),
            inline_last_log: meta.inline_last_log.clone(),
            inline_last_log_mtime: resolver
                .state_binding_for_pid(proc.key.pid)
                .and_then(|sb| sb.mtime),
            custom_command: meta.custom_command.clone(),
            is_watched: meta.is_watched,
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: proc.uptime_secs,
            primary_key: proc.key,
            primary_is_wrapper: meta.is_wrapper_or_helper,
            can_kill: false,
            can_restart: false,
            has_non_compiler: false,
        });

        acc.cpu += proc.cpu_percent;
        acc.ram = acc.ram.saturating_add(proc.private_ram_bytes);
        acc.uptime = acc.uptime.max(proc.uptime_secs);
        acc.can_kill |= can_kill;
        acc.can_restart |= meta.restart_recipe.is_some();
        if !matches!(meta.role, ProcessRole::CompilerOrBuild | ProcessRole::IdeTooling) {
            acc.has_non_compiler = true;
        }
        if acc.primary_is_wrapper && !meta.is_wrapper_or_helper {
            acc.primary_key = proc.key;
            acc.primary_is_wrapper = false;
        }
        if acc.root.is_none() && meta.project_root.is_some() {
            acc.root = meta.project_root.clone();
        }
        if acc.git_branch.is_none() && meta.git_branch.is_some() {
            acc.git_branch = meta.git_branch.clone();
        }
        if acc.log_path.is_none() && meta.log_path.is_some() {
            acc.log_path = meta.log_path.clone();
        }
        if acc.custom_command.is_none() && meta.custom_command.is_some() {
            acc.custom_command = meta.custom_command.clone();
        }
        acc.is_watched |= meta.is_watched;
        if let Some(sb) = resolver.state_binding_for_pid(proc.key.pid) {
            if sb.last_log.is_some() {
                acc.inline_last_log = sb.last_log.clone();
                acc.inline_last_log_mtime = sb.mtime;
            }
        } else if acc.inline_last_log.is_none() && meta.inline_last_log.is_some() {
            acc.inline_last_log = meta.inline_last_log.clone();
        }
        if acc.icon.is_none() && meta.job_icon.is_some() {
            acc.icon = meta.job_icon.clone();
        }
        if acc.color.is_none() && meta.job_color.is_some() {
            acc.color = meta.job_color.clone();
        }
        for pb in proc_ports {
            if !acc
                .ports
                .iter()
                .any(|existing| existing.port == pb.port && existing.protocol == pb.protocol)
            {
                acc.ports.push(pb);
            }
        }
        acc.procs.push(info);
    }

    let mut proj_map: HashMap<ProjectId, ProjectAcc> = HashMap::new();
    let mut active_job_ids: HashSet<JobId> = HashSet::new();
    let mut active_job_titles: HashSet<(ProjectId, String)> = HashSet::new();
    let mut active_job_name_pairs: HashSet<(String, String)> = HashSet::new();

    for (_, mut jacc) in job_map {
        // 8-second debounce for short-lived non-port, non-watched jobs so CLI flashes never flicker in UI.
        let has_listening_port = jacc.ports.iter().any(|p| p.is_listening);
        if jacc.uptime < SHORT_JOB_DEBOUNCE_SECS && !has_listening_port && !jacc.is_watched {
            continue;
        }

        active_job_ids.insert(jacc.id.clone());
        active_job_titles.insert((jacc.project.clone(), jacc.title.to_ascii_lowercase()));
        active_job_name_pairs.insert((
            jacc.project_name.to_ascii_lowercase(),
            jacc.title.to_ascii_lowercase(),
        ));
        jacc.ports.sort_by_key(|p| p.port);
        jacc.procs.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.ram.cmp(&a.ram))
        });

        let file_log = jacc
            .log_path
            .as_deref()
            .map(|p| log_tail_cache.read_last_line_with_mtime(p, allow_log_io));
        let last_log = match (file_log, jacc.inline_last_log) {
            (Some((Some(f_line), f_mtime)), Some(inline_line)) => {
                let prefer_inline = match (jacc.inline_last_log_mtime, f_mtime) {
                    (Some(sm), Some(fm)) => sm > fm,
                    _ => false,
                };
                if prefer_inline {
                    Some(inline_line)
                } else {
                    Some(f_line)
                }
            }
            (Some((Some(f_line), _)), None) => Some(f_line),
            (_, inline) => inline,
        };
        let crash_summary = job_crashes
            .get(&jacc.id)
            .filter(|c| now < c.until)
            .map(|c| {
                format!(
                    "{} · {} {}",
                    lang.pick("Crashed", "Çöktü"),
                    lang.pick("code", "kod"),
                    c.exit_code
                )
            });

        let status = if crash_summary.is_some() {
            ProjectStatus::Crashed
        } else if jacc.cpu > 75.0 && jacc.has_non_compiler {
            ProjectStatus::HighLoad
        } else {
            ProjectStatus::Healthy
        };

        let job_procs = jacc.procs;
        let proc_cnt = job_procs.len() as u16;
        let job_has_non_compiler = jacc.has_non_compiler;

        let job = Job {
            id: jacc.id,
            project: jacc.project.clone(),
            project_name: jacc.project_name.clone(),
            title: jacc.title,
            subtitle: jacc.subtitle,
            icon: jacc.icon.clone(),
            color: jacc.color.clone(),
            procs: job_procs,
            cpu: jacc.cpu,
            ram: jacc.ram,
            ports: jacc.ports.clone(),
            uptime: jacc.uptime,
            status,
            last_log,
            crash_summary,
            root: jacc.root.clone(),
            primary_key: jacc.primary_key,
            match_hint: jacc.match_hint,
            log_path: jacc.log_path,
            custom_command: jacc.custom_command,
            is_watched: jacc.is_watched,
            can_kill: jacc.can_kill,
            can_restart: jacc.can_restart,
        };

        let pacc = proj_map.entry(jacc.project).or_insert_with(|| ProjectAcc {
            name: jacc.project_name,
            root: jacc.root,
            git_branch: jacc.git_branch.clone(),
            icon: jacc.icon,
            color: jacc.color,
            jobs: Vec::new(),
            procs: Vec::new(),
            ports: Vec::new(),
            cpu: 0.0,
            ram: 0,
            proc_count: 0,
            has_non_compiler: false,
        });

        pacc.cpu += job.cpu;
        pacc.ram = pacc.ram.saturating_add(job.ram);
        pacc.proc_count = pacc.proc_count.saturating_add(proc_cnt);
        pacc.has_non_compiler |= job_has_non_compiler;
        if pacc.git_branch.is_none() && jacc.git_branch.is_some() {
            pacc.git_branch = jacc.git_branch;
        }
        if pacc.icon.is_none() && job.icon.is_some() {
            pacc.icon = job.icon.clone();
        }
        if pacc.color.is_none() && job.color.is_some() {
            pacc.color = job.color.clone();
        }
        for pb in &job.ports {
            if !pacc
                .ports
                .iter()
                .any(|existing| existing.port == pb.port && existing.protocol == pb.protocol)
            {
                pacc.ports.push(pb.clone());
            }
        }
        pacc.procs.extend(job.procs.iter().cloned());
        pacc.jobs.push(job);
    }

    // Merge recently crashed jobs so they remain visible in the list with red status and Restart.
    for (jid, rec) in job_crashes {
        if now >= rec.until || active_job_ids.contains(jid) {
            continue;
        }
        if is_hidden(&rec.project_id, project_states) {
            continue;
        }
        active_job_ids.insert(jid.clone());
        active_job_titles.insert((rec.project_id.clone(), rec.title.to_ascii_lowercase()));
        active_job_name_pairs.insert((
            rec.project_name.to_ascii_lowercase(),
            rec.title.to_ascii_lowercase(),
        ));
        let pacc = proj_map
            .entry(rec.project_id.clone())
            .or_insert_with(|| ProjectAcc {
                name: rec.project_name.clone(),
                root: rec.root.clone(),
                git_branch: None,
                icon: rec.icon.clone(),
                color: rec.color.clone(),
                jobs: Vec::new(),
                procs: Vec::new(),
                ports: Vec::new(),
                cpu: 0.0,
                ram: 0,
                proc_count: 0,
                has_non_compiler: false,
            });
        pacc.jobs.push(Job {
            id: jid.clone(),
            project: rec.project_id.clone(),
            project_name: rec.project_name.clone(),
            title: rec.title.clone(),
            subtitle: rec.subtitle.clone(),
            icon: rec.icon.clone(),
            color: rec.color.clone(),
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: 0,
            status: ProjectStatus::Crashed,
            last_log: rec
                .log_path
                .as_deref()
                .and_then(|p| log_tail_cache.read_last_line(p, allow_log_io)),
            crash_summary: Some(format!(
                "{} · {} {}",
                lang.pick("Crashed", "Çöktü"),
                lang.pick("code", "kod"),
                rec.exit_code
            )),
            root: rec.root.clone(),
            primary_key: rec.primary_key,
            match_hint: rec.match_hint.clone(),
            log_path: rec.log_path.clone(),
            custom_command: rec.custom_command.clone(),
            is_watched: rec.is_watched,
            can_kill: false,
            can_restart: rec.can_restart,
        });
    }

    // Merge known-but-stopped automation jobs (e.g. scheduled worker scripts).
    for kj in &known_snap.jobs {
        if active_job_ids.contains(&kj.job_id)
            || active_job_titles.contains(&(kj.project_id.clone(), kj.title.to_ascii_lowercase()))
            || active_job_name_pairs.contains(&(
                kj.project_name.to_ascii_lowercase(),
                kj.title.to_ascii_lowercase(),
            ))
        {
            continue;
        }
        if is_hidden(&kj.project_id, project_states) {
            continue;
        }
        let pacc = proj_map
            .entry(kj.project_id.clone())
            .or_insert_with(|| ProjectAcc {
                name: kj.project_name.clone(),
                root: Some(kj.root.clone()),
                git_branch: None,
                icon: kj.icon.clone(),
                color: kj.color.clone(),
                jobs: Vec::new(),
                procs: Vec::new(),
                ports: Vec::new(),
                cpu: 0.0,
                ram: 0,
                proc_count: 0,
                has_non_compiler: false,
            });
        pacc.jobs.push(Job {
            id: kj.job_id.clone(),
            project: kj.project_id.clone(),
            project_name: kj.project_name.clone(),
            title: kj.title.clone(),
            subtitle: kj.subtitle.clone(),
            icon: kj.icon.clone(),
            color: kj.color.clone(),
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: 0,
            status: ProjectStatus::Stopped,
            last_log: kj.last_log.clone(),
            crash_summary: None,
            root: Some(kj.root.clone()),
            primary_key: ProcessKey::new(0, 0),
            match_hint: kj.match_hint.clone(),
            log_path: kj.log_path.clone(),
            custom_command: kj.custom_command.clone(),
            is_watched: kj.is_watched,
            can_kill: false,
            can_restart: kj.custom_command.as_ref().is_some_and(|c| !c.trim().is_empty()),
        });
    }

    // Merge user-defined custom/watched jobs from `labels.toml` (`watch = true` or `command` set)
    // when no live process currently matches them, so user-added jobs stay on Home!
    let active_hints: HashSet<String> = proj_map
        .values()
        .flat_map(|p| p.jobs.iter().map(|j| j.match_hint.to_ascii_lowercase()))
        .filter(|h| !h.is_empty())
        .collect();
    for rule in resolver.labels().rules() {
        let is_watched = rule.watch == Some(true) || rule.command.is_some();
        let needle = rule.match_substr.trim();
        if !is_watched || needle.is_empty() {
            continue;
        }
        let needle_low = needle.to_ascii_lowercase();
        let job_title = if rule.job.trim().is_empty() {
            needle.to_string()
        } else {
            rule.job.trim().to_string()
        };
        let proj_name = rule
            .project
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|p| {
                if p.eq_ignore_ascii_case("Custom Jobs") || p.eq_ignore_ascii_case("Özel İşler") {
                    lang.pick("Custom Jobs", "Özel İşler").to_string()
                } else {
                    p.to_string()
                }
            })
            .unwrap_or_else(|| lang.pick("Custom Jobs", "Özel İşler").to_string());

        if active_hints
            .iter()
            .any(|h| h.contains(&needle_low) || (h.len() >= 4 && h == &needle_low))
            || active_job_name_pairs
                .contains(&(proj_name.to_ascii_lowercase(), job_title.to_ascii_lowercase()))
        {
            continue;
        }
        active_job_name_pairs.insert((proj_name.to_ascii_lowercase(), job_title.to_ascii_lowercase()));

        // Attach to an existing project group with the same display name if present.
        let existing_pid = proj_map
            .iter()
            .find(|(_, p)| p.name.eq_ignore_ascii_case(&proj_name))
            .map(|(pid, _)| pid.clone());
        let proj_id = existing_pid
            .unwrap_or_else(|| ProjectId(format!("rule:{}", proj_name.to_ascii_lowercase())));
        if is_hidden(&proj_id, project_states) {
            continue;
        }

        let work_dir = rule
            .working_dir
            .as_deref()
            .map(|w| PathBuf::from(expand_win_env_vars(w)));
        let log_path = rule
            .log_file
            .as_deref()
            .map(|l| PathBuf::from(expand_win_env_vars(l)));
        let last_log = if allow_log_io {
            log_path
                .as_deref()
                .and_then(|p| log_tail_cache.read_last_line(p, true))
        } else {
            None
        };
        let can_start = rule.command.as_ref().is_some_and(|c| !c.trim().is_empty());
        let sub = if let Some(ref cmd) = rule.command {
            format!("{proj_name} › {cmd}")
        } else {
            format!(
                "{proj_name} › {} ({needle})",
                lang.pick("Watched job", "İzlenen iş")
            )
        };

        let pacc = proj_map.entry(proj_id.clone()).or_insert_with(|| ProjectAcc {
            name: proj_name.clone(),
            root: work_dir.clone(),
            git_branch: None,
            icon: rule.icon.clone(),
            color: rule.color.clone(),
            jobs: Vec::new(),
            procs: Vec::new(),
            ports: Vec::new(),
            cpu: 0.0,
            ram: 0,
            proc_count: 0,
            has_non_compiler: false,
        });
        pacc.jobs.push(Job {
            id: JobId(format!("custom:{needle_low}")),
            project: proj_id,
            project_name: proj_name,
            title: job_title,
            subtitle: sub,
            icon: rule.icon.clone(),
            color: rule.color.clone(),
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: 0,
            status: ProjectStatus::Stopped,
            last_log,
            crash_summary: None,
            root: work_dir,
            primary_key: ProcessKey::new(0, 0),
            match_hint: needle.to_string(),
            log_path,
            custom_command: rule.command.clone(),
            is_watched: true,
            can_kill: false,
            can_restart: can_start,
        });
    }

    // Add Docker Compose project groups discovered via named pipe.
    for container in resolver.docker_containers() {
        let proj_key = container
            .compose_project
            .as_deref()
            .unwrap_or(&container.name);
        let id = ProjectId(format!("docker:{}", proj_key.to_ascii_lowercase()));
        if is_hidden(&id, project_states) {
            continue;
        }
        let pacc = proj_map.entry(id.clone()).or_insert_with(|| ProjectAcc {
            name: proj_key.to_string(),
            root: None,
            git_branch: None,
            icon: None,
            color: None,
            jobs: Vec::new(),
            procs: Vec::new(),
            ports: Vec::new(),
            cpu: 0.0,
            ram: 0,
            proc_count: 0,
            has_non_compiler: false,
        });
        let container_ports: Vec<PortBinding> = container
            .ports
            .iter()
            .map(|&port| PortBinding {
                port,
                protocol: TransportProto::Tcp,
                scope: BindScope::AllInterfaces,
                is_listening: true,
                suggested_url: crate::engine::collector::suggest_url_for_port(port),
            })
            .collect();
        for pb in &container_ports {
            if !pacc.ports.iter().any(|p| p.port == pb.port) {
                pacc.ports.push(pb.clone());
            }
        }
        pacc.jobs.push(Job {
            id: JobId(format!("docker:{}:{}", proj_key, container.name)),
            project: id,
            project_name: proj_key.to_string(),
            title: container.name.clone(),
            subtitle: format!("{proj_key} › {}", lang.pick("Docker Container", "Docker Konteyneri")),
            icon: None,
            color: None,
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: container_ports,
            uptime: 0,
            status: ProjectStatus::Healthy,
            last_log: None,
            crash_summary: None,
            root: None,
            primary_key: ProcessKey::new(0, 0),
            match_hint: container.name.clone(),
            log_path: None,
            custom_command: None,
            is_watched: false,
            can_kill: false,
            can_restart: false,
        });
    }

    let mut total_dev_cpu = 0.0f32;
    let mut total_dev_ram_bytes = 0u64;
    let mut total_jobs = 0u16;
    let mut total_procs = 0u16;
    let mut projects = Vec::with_capacity(proj_map.len());

    for (id, mut pacc) in proj_map {
        pacc.ports.sort_by_key(|p| p.port);
        pacc.procs.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.ram.cmp(&a.ram))
        });
        sort_jobs(&mut pacc.jobs, sort_mode, job_order);

        let st = project_states.entry(id.clone()).or_default();
        let key_l = id.0.to_ascii_lowercase();
        st.pinned = pinned_keys.contains(&key_l);
        st.hidden = hidden_keys.contains(&key_l);
        st.display_name_override = alias_keys.get(&id.0).cloned();
        st.last_known_name = pacc.name.clone();
        st.last_seen = Some(now);
        if pacc.root.is_some() {
            st.last_known_root = pacc.root.clone();
        }
        if pacc.git_branch.is_some() {
            st.last_known_branch = pacc.git_branch.clone();
        }

        let display_name = st.display_name_override.clone().unwrap_or(pacc.name);

        st.cpu_history.copy_within(1.., 0);
        st.cpu_history[29] = pacc.cpu.round().clamp(0.0, 100.0) as u8;
        st.ram_history_mb.copy_within(1.., 0);
        st.ram_history_mb[29] = (pacc.ram / (1024 * 1024)).min(u16::MAX as u64) as u16;

        let has_only_compilers = pacc.proc_count > 0 && !pacc.has_non_compiler;
        let mut high_load = false;
        if pacc.cpu > 85.0 && !has_only_compilers {
            high_load = true;
            let since = *st.high_cpu_since.get_or_insert(now);
            let dur = now.saturating_duration_since(since);
            let cooldown_ok = st
                .last_high_cpu_alert
                .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(120));
            if dur >= Duration::from_secs(15) && cooldown_ok {
                st.last_high_cpu_alert = Some(now);
                let message = match lang {
                    Language::En => format!(
                        "{display_name} high CPU ({:.0}% for {}s)",
                        pacc.cpu,
                        dur.as_secs()
                    ),
                    Language::Tr => format!(
                        "{display_name} yüksek CPU ({}sn boyunca %{:.0})",
                        dur.as_secs(),
                        pacc.cpu
                    ),
                };
                alerts.insert(
                    0,
                    Alert {
                        id: *next_alert_id,
                        project_id: id.clone(),
                        project_name: display_name.clone(),
                        kind: AlertKind::HighCpu {
                            cpu_percent: pacc.cpu,
                            duration_secs: dur.as_secs(),
                        },
                        message,
                        timestamp_ms: now_ms,
                        dismissed: false,
                    },
                );
                *next_alert_id += 1;
            }
        } else {
            st.high_cpu_since = None;
        }

        const TWO_GB: u64 = 2 * 1024 * 1024 * 1024;
        match st.ram_baseline {
            Some((t0, base_ram)) if now.saturating_duration_since(t0) <= Duration::from_secs(30) => {
                if pacc.ram > TWO_GB && base_ram > 0 && pacc.ram >= base_ram.saturating_mul(2) {
                    high_load = true;
                    let cooldown_ok = st
                        .last_mem_alert
                        .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(120));
                    if cooldown_ok {
                        st.last_mem_alert = Some(now);
                        let gb = pacc.ram as f64 / (1024.0 * 1024.0 * 1024.0);
                        let message = match lang {
                            Language::En => format!("{display_name} memory spike ({gb:.1} GB)"),
                            Language::Tr => format!("{display_name} bellek sıçraması ({gb:.1} GB)"),
                        };
                        alerts.insert(
                            0,
                            Alert {
                                id: *next_alert_id,
                                project_id: id.clone(),
                                project_name: display_name.clone(),
                                kind: AlertKind::MemorySpike { ram_bytes: pacc.ram },
                                message,
                                timestamp_ms: now_ms,
                                dismissed: false,
                            },
                        );
                        *next_alert_id += 1;
                    }
                }
            }
            _ => {
                st.ram_baseline = Some((now, pacc.ram));
            }
        }

        let is_crashed = st.crashed_until.is_some_and(|until| now < until)
            || pacc.jobs.iter().any(|j| j.status == ProjectStatus::Crashed);
        let all_stopped = !pacc.jobs.is_empty()
            && pacc
                .jobs
                .iter()
                .all(|j| matches!(j.status, ProjectStatus::Stopped));
        let status = if is_crashed {
            ProjectStatus::Crashed
        } else if all_stopped {
            ProjectStatus::Stopped
        } else if high_load || (pacc.cpu > 75.0 && !has_only_compilers) {
            ProjectStatus::HighLoad
        } else {
            ProjectStatus::Healthy
        };

        let watchdog_note = known_snap.watchdog_notes.get(&id).cloned();

        total_dev_cpu += pacc.cpu;
        total_dev_ram_bytes = total_dev_ram_bytes.saturating_add(pacc.ram);
        total_jobs = total_jobs.saturating_add(pacc.jobs.len() as u16);
        total_procs = total_procs.saturating_add(pacc.proc_count);

        projects.push(ProjectGroup {
            id,
            name: display_name,
            root: pacc.root,
            git_branch: pacc.git_branch,
            icon: pacc.icon,
            color: pacc.color,
            pinned: st.pinned,
            status,
            cpu: pacc.cpu,
            ram: pacc.ram,
            ports: pacc.ports,
            jobs: pacc.jobs,
            watchdog_note,
            procs: pacc.procs,
            cpu_history: st.cpu_history,
            ram_history_mb: st.ram_history_mb,
        });
    }

    // Keep pinned projects that currently have 0 running processes as `Stopped`.
    let active_ids: HashSet<ProjectId> = projects.iter().map(|p| p.id.clone()).collect();
    for (id, st) in project_states.iter() {
        if st.pinned && !st.hidden && !active_ids.contains(id) && !st.last_known_name.is_empty() {
            let name = st
                .display_name_override
                .clone()
                .unwrap_or_else(|| st.last_known_name.clone());
            projects.push(ProjectGroup {
                id: id.clone(),
                name,
                root: st.last_known_root.clone(),
                git_branch: st.last_known_branch.clone(),
                icon: None,
                color: None,
                pinned: true,
                status: ProjectStatus::Stopped,
                cpu: 0.0,
                ram: 0,
                ports: Vec::new(),
                jobs: Vec::new(),
                watchdog_note: known_snap.watchdog_notes.get(id).cloned(),
                procs: Vec::new(),
                cpu_history: st.cpu_history,
                ram_history_mb: st.ram_history_mb,
            });
        }
    }

    sort_projects(&mut projects, sort_mode, project_order);

    if alerts.len() > 20 {
        alerts.truncate(20);
    }

    Snapshot {
        timestamp_ms: now_ms,
        polling_mode,
        total_projects: projects.len() as u16,
        total_jobs,
        total_procs,
        total_dev_cpu,
        total_dev_ram_bytes,
        system_cpu,
        system_ram_used_bytes,
        system_ram_total_bytes,
        system_cpu_history,
        system_ram_history,
        projects,
        alerts: alerts.clone(),
        all_procs: Vec::new(),
        active_terminal: None,
        last_action_error: None,
    }
}

pub(super) fn order_rank(order_list: &[String], key_a: &str, key_b: Option<&str>) -> usize {
    order_list
        .iter()
        .position(|k| {
            k.eq_ignore_ascii_case(key_a) || key_b.is_some_and(|b| k.eq_ignore_ascii_case(b))
        })
        .unwrap_or(usize::MAX)
}

pub(super) fn sort_jobs(jobs: &mut [Job], sort_mode: SortMode, job_order: &[String]) {
    jobs.sort_by(|a, b| match sort_mode {
        SortMode::Custom => {
            let oa = order_rank(job_order, &a.id.0, Some(&a.match_hint));
            let ob = order_rank(job_order, &b.id.0, Some(&b.match_hint));
            oa.cmp(&ob)
                .then_with(|| status_rank(b.status).cmp(&status_rank(a.status)))
                .then_with(|| (!b.ports.is_empty()).cmp(&(!a.ports.is_empty())))
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| b.ram.cmp(&a.ram))
                .then_with(|| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()))
        }
        SortMode::Cpu => b
            .cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| status_rank(b.status).cmp(&status_rank(a.status)))
            .then_with(|| b.ram.cmp(&a.ram)),
        SortMode::Ram => b
            .ram
            .cmp(&a.ram)
            .then_with(|| status_rank(b.status).cmp(&status_rank(a.status)))
            .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)),
        SortMode::Name => a
            .title
            .to_ascii_lowercase()
            .cmp(&b.title.to_ascii_lowercase()),
        SortMode::Uptime => b
            .uptime
            .cmp(&a.uptime)
            .then_with(|| status_rank(b.status).cmp(&status_rank(a.status))),
    });
}

pub(super) fn sort_projects(projects: &mut [ProjectGroup], sort_mode: SortMode, project_order: &[String]) {
    projects.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned).then_with(|| match sort_mode {
            SortMode::Custom => {
                let oa = order_rank(project_order, &a.id.0, Some(&a.name));
                let ob = order_rank(project_order, &b.id.0, Some(&b.name));
                oa.cmp(&ob)
                    .then_with(|| status_rank(b.status).cmp(&status_rank(a.status)))
                    .then_with(|| (!b.ports.is_empty()).cmp(&(!a.ports.is_empty())))
                    .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                    .then_with(|| b.ram.cmp(&a.ram))
                    .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
            }
            SortMode::Cpu => b
                .cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.ram.cmp(&a.ram))
                .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
            SortMode::Ram => b
                .ram
                .cmp(&a.ram)
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
            SortMode::Name => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
            SortMode::Uptime => {
                let ua = a.jobs.iter().map(|j| j.uptime).max().unwrap_or(0);
                let ub = b.jobs.iter().map(|j| j.uptime).max().unwrap_or(0);
                ub.cmp(&ua)
                    .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
            }
        })
    });
}

pub(super) fn status_rank(status: ProjectStatus) -> u8 {
    match status {
        ProjectStatus::Crashed => 3,
        ProjectStatus::HighLoad => 2,
        ProjectStatus::Healthy => 1,
        ProjectStatus::Stopped => 0,
    }
}

pub(super) fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Formats the current Job tree as plain text for `--dump-snapshot`.
pub fn format_snapshot_text(snap: &Snapshot) -> String {
    use crate::ui::layout::{filter_project_jobs, format_ram, format_uptime};

    let mut out = String::new();
    out.push_str(&format!(
        "SUMMARY: {} jobs | {} projects | {} procs | CPU {:.1}% | RAM {}\n",
        snap.total_jobs,
        snap.total_projects,
        snap.total_procs,
        snap.total_dev_cpu,
        format_ram(snap.total_dev_ram_bytes)
    ));
    let groups = filter_project_jobs(snap, "");
    for (proj, jobs) in groups {
        out.push_str(&format!(
            "\n[PROJECT] {} ({} jobs | CPU {:.1}% | RAM {})\n",
            proj.name,
            jobs.len(),
            proj.cpu,
            format_ram(proj.ram)
        ));
        if let Some(ref wd) = proj.watchdog_note {
            out.push_str(&format!("  [INFO] {}\n", wd));
        }
        for job in jobs {
            let status_tag = if job.status == ProjectStatus::Stopped && job.procs.is_empty() {
                " [STOPPED]"
            } else if job.status == ProjectStatus::Crashed {
                " [CRASHED]"
            } else {
                ""
            };
            let ports = if job.ports.is_empty() {
                String::new()
            } else {
                let p: Vec<String> = job.ports.iter().map(|b| format!(":{}", b.port)).collect();
                format!(" | {}", p.join(" "))
            };
            let log = job
                .last_log
                .as_deref()
                .map(|l| format!(" | last_log: {}", l))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {} > {}{} | {} procs | {} | {:.1}% | {}{}{}\n",
                proj.name,
                job.title,
                status_tag,
                job.procs.len(),
                format_ram(job.ram),
                job.cpu,
                format_uptime(job.uptime),
                ports,
                log
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::engine::project_resolver::{InferenceSource, ProcessMetadata};

    fn make_job(id: &str, title: &str, match_hint: &str, cpu: f32, ram: u64, status: ProjectStatus) -> Job {
        Job {
            id: JobId(id.to_string()),
            project: ProjectId("p:1".to_string()),
            project_name: "Demo".to_string(),
            title: title.to_string(),
            subtitle: String::new(),
            icon: None,
            color: None,
            procs: Vec::new(),
            cpu,
            ram,
            ports: Vec::new(),
            uptime: 100,
            status,
            last_log: None,
            crash_summary: None,
            root: None,
            primary_key: ProcessKey::new(10, 100),
            match_hint: match_hint.to_string(),
            log_path: None,
            custom_command: None,
            is_watched: false,
            can_kill: true,
            can_restart: false,
        }
    }

    #[test]
    fn order_rank_and_sort_jobs_custom_and_cpu() {
        let order = vec!["api.js".to_string(), "job:web".to_string()];
        assert_eq!(order_rank(&order, "JOB:WEB", None), 1);
        assert_eq!(order_rank(&order, "other", Some("API.JS")), 0);
        assert_eq!(order_rank(&order, "missing", None), usize::MAX);

        let mut jobs = vec![
            make_job("job:web", "Web", "web.js", 5.0, 100, ProjectStatus::Healthy),
            make_job("job:api", "API", "api.js", 50.0, 200, ProjectStatus::Healthy),
        ];
        sort_jobs(&mut jobs, SortMode::Custom, &order);
        assert_eq!(jobs[0].id.0, "job:api");
        assert_eq!(jobs[1].id.0, "job:web");

        sort_jobs(&mut jobs, SortMode::Cpu, &[]);
        assert_eq!(jobs[0].id.0, "job:api");
    }

    #[test]
    fn build_snapshot_backfills_wrapper_root_and_exempts_compiler_and_watched_debounce() {
        let now = Instant::now();
        let wrapper_key = ProcessKey::new(100, 10);
        let worker_key = ProcessKey::new(101, 20);
        let compiler_key = ProcessKey::new(200, 30);

        let procs = vec![
            CollectedProcess {
                key: wrapper_key,
                parent_pid: 1,
                session_id: 1,
                image_name: "cmd.exe".into(),
                cpu_percent: 0.1,
                private_ram_bytes: 1024,
                uptime_secs: 2,
            },
            CollectedProcess {
                key: worker_key,
                parent_pid: 100,
                session_id: 1,
                image_name: "python.exe".into(),
                cpu_percent: 1.5,
                private_ram_bytes: 8192,
                uptime_secs: 2,
            },
            CollectedProcess {
                key: compiler_key,
                parent_pid: 1,
                session_id: 1,
                image_name: "rustc.exe".into(),
                cpu_percent: 92.0,
                private_ram_bytes: 65536,
                uptime_secs: 30,
            },
        ];

        let mut resolver = ProjectResolver::new(999_999);
        resolver.insert_test_meta(
            wrapper_key,
            ProcessMetadata {
                image_name: "cmd.exe".into(),
                working_dir: None,
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("p:bot".into()),
                project_display_name: "Bot".into(),
                project_root: None,
                git_branch: None,
                job_id: JobId("j:bot".into()),
                job_title: "Worker".into(),
                job_subtitle: String::new(),
                job_icon: None,
                job_color: None,
                match_hint: "worker.py".into(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::WorkerOrQueue,
                role_label: "Worker".into(),
                inference_source: InferenceSource::CommandLineHeuristic,
                is_ignored_noise: false,
                is_wrapper_or_helper: true,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            },
        );
        resolver.insert_test_meta(
            worker_key,
            ProcessMetadata {
                image_name: "python.exe".into(),
                working_dir: Some(PathBuf::from("/dev/bot")),
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("p:bot".into()),
                project_display_name: "Bot".into(),
                project_root: Some(PathBuf::from("/dev/bot")),
                git_branch: Some("main".into()),
                job_id: JobId("j:bot".into()),
                job_title: "Worker".into(),
                job_subtitle: String::new(),
                job_icon: None,
                job_color: None,
                match_hint: "worker.py".into(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: true,
                role: ProcessRole::WorkerOrQueue,
                role_label: "Worker".into(),
                inference_source: InferenceSource::ManifestPyProject,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            },
        );
        resolver.insert_test_meta(
            compiler_key,
            ProcessMetadata {
                image_name: "rustc.exe".into(),
                working_dir: Some(PathBuf::from("/dev/crate")),
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("p:crate".into()),
                project_display_name: "Crate".into(),
                project_root: Some(PathBuf::from("/dev/crate")),
                git_branch: Some("dev".into()),
                job_id: JobId("j:rustc".into()),
                job_title: "Build".into(),
                job_subtitle: String::new(),
                job_icon: None,
                job_color: None,
                match_hint: "rustc".into(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::CompilerOrBuild,
                role_label: "Rust Compiler".into(),
                inference_source: InferenceSource::ManifestCargoToml,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: false,
                restart_recipe: None,
                auto_restart: false,
            },
        );

        let mut job_crashes = HashMap::new();
        // Expired crash must NOT mark live job as Crashed
        job_crashes.insert(
            JobId("j:bot".into()),
            CrashedJobRecord {
                project_id: ProjectId("p:bot".into()),
                project_name: "Bot".into(),
                title: "Worker".into(),
                subtitle: String::new(),
                icon: None,
                color: None,
                root: None,
                primary_key: worker_key,
                match_hint: "worker.py".into(),
                log_path: None,
                custom_command: None,
                is_watched: true,
                can_restart: false,
                exit_code: 1,
                until: now - Duration::from_secs(5),
            },
        );

        let mut states = HashMap::new();
        let mut alerts = Vec::new();
        let mut next_alert_id = 1;
        let mut log_cache = LogTailCache::new();
        let snap = build_snapshot(
            &procs,
            10.0,
            1000,
            2000,
            [0; 24],
            [0; 24],
            &HashMap::new(),
            &resolver,
            &KnownJobsSnapshot::default(),
            &mut log_cache,
            &job_crashes,
            &mut states,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            SortMode::Custom,
            &[],
            &[],
            &mut alerts,
            &mut next_alert_id,
            PollingMode::ExpandedActive,
            999_999,
            &HashSet::new(),
            Language::En,
        );

        let bot = snap.projects.iter().find(|p| p.id.0 == "p:bot").expect("watched job visible immediately");
        assert_eq!(bot.git_branch.as_deref(), Some("main"));
        assert_eq!(bot.root.as_deref(), Some(Path::new("/dev/bot")));
        assert_eq!(bot.status, ProjectStatus::Healthy);

        let crate_p = snap.projects.iter().find(|p| p.id.0 == "p:crate").expect("compiler project");
        assert_eq!(crate_p.status, ProjectStatus::Healthy);
    }
}

