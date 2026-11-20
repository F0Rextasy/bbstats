//! View state machine, Job/Project filtering, and DIP layout dimensions.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use crate::config::DockPosition;
use crate::model::{
    fold_search_lower, Job, JobId, ProcessKey, ProjectGroup, ProjectId, ProjectStatus, Snapshot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    Collapsed,
    HoverPeek,
    Expanded,
    Detail {
        project_id: ProjectId,
        job_id: Option<JobId>,
    },
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKill {
    Project(ProjectId),
    Job(JobId),
    Process(ProcessKey),
}

#[derive(Debug, Clone)]
pub struct RenameDraft {
    pub project_id: Option<ProjectId>,
    pub match_substr: String,
    pub input: String,
}

#[derive(Debug, Clone, Default)]
pub struct AddJobDraft {
    pub title: String,
    pub command_or_match: String,
    pub log_file: String,
    pub active_field: u8,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub mode: ViewMode,
    pub sticky: bool,
    pub unpinned_auto_collapse: bool,
    pub show_all_processes: bool,
    pub search_query: String,
    pub search_focused: bool,
    pub text_all_selected: bool,
    pub rename_draft: Option<RenameDraft>,
    pub add_job_draft: Option<AddJobDraft>,
    pub selected_index: usize,
    pub scroll_offset: f32,
    pub max_scroll: f32,
    pub term_scroll_offset: f32,
    pub term_max_scroll: f32,
    pub hovered_hit_id: Option<u32>,
    pub hover_pos_dip: Option<(f32, f32)>,
    pub content_enter_t: f32,
    pub detail_tab_t: f32,
    pub detail_tab_dir: f32,
    pub pending_kill: Option<PendingKill>,
    pub hotkey_failed: bool,
    #[cfg(target_os = "windows")]
    pub toast: Option<(String, std::time::Instant)>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            mode: ViewMode::Collapsed,
            sticky: false,
            unpinned_auto_collapse: false,
            show_all_processes: false,
            search_query: String::new(),
            search_focused: false,
            text_all_selected: false,
            rename_draft: None,
            add_job_draft: None,
            selected_index: 0,
            scroll_offset: 0.0,
            max_scroll: 0.0,
            term_scroll_offset: 0.0,
            term_max_scroll: 0.0,
            hovered_hit_id: None,
            hover_pos_dip: None,
            content_enter_t: 1.0,
            detail_tab_t: 1.0,
            detail_tab_dir: 1.0,
            pending_kill: None,
            hotkey_failed: false,
            #[cfg(target_os = "windows")]
            toast: None,
        }
    }
}

/// Computes `(opacity, offset_y_dip)` for staggered list row entry given `enter_t` in `0.0..=1.0`.
#[inline]
pub fn stagger_row_progress(enter_t: f32, row_idx: usize) -> (f32, f32) {
    if enter_t >= 0.995 {
        return (1.0, 0.0);
    }
    let t = enter_t.clamp(0.0, 1.0);
    let delay = (row_idx.min(8) as f32) * 0.055;
    let span = (1.0 - delay).max(0.20);
    let u = ((t - delay) / span).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - u).powi(3);
    (eased, (1.0 - eased) * 8.0)
}

/// Filters projects and their visible Jobs against the search query while preserving engine sort order.
pub fn filter_project_jobs<'a>(
    snap: &'a Snapshot,
    query: &str,
) -> Vec<(&'a ProjectGroup, Vec<&'a Job>)> {
    let q = fold_search_lower(query.trim());
    let mut out: Vec<(&'a ProjectGroup, Vec<&'a Job>)> = Vec::new();

    for proj in &snap.projects {
        if proj.jobs.is_empty() && proj.status != ProjectStatus::Crashed {
            continue;
        }
        let proj_matches = q.is_empty() || fold_search_lower(&proj.name).contains(&q);
        let jobs: Vec<&'a Job> = proj
            .jobs
            .iter()
            .filter(|j| {
                if proj_matches {
                    return true;
                }
                if fold_search_lower(&j.title).contains(&q)
                    || fold_search_lower(&j.subtitle).contains(&q)
                    || j.last_log
                        .as_deref()
                        .map(|l| fold_search_lower(l).contains(&q))
                        .unwrap_or(false)
                {
                    return true;
                }
                let port_q = q.trim_start_matches(':');
                if let Ok(port_num) = port_q.parse::<u16>() {
                    if j.ports.iter().any(|b| b.port == port_num) {
                        return true;
                    }
                }
                j.procs
                    .iter()
                    .any(|pr| pr.key.pid.to_string().contains(&q))
            })
            .collect();

        if !jobs.is_empty()
            || (proj_matches && (!proj.jobs.is_empty() || proj.status == ProjectStatus::Crashed))
        {
            out.push((proj, jobs));
        }
    }
    out
}

/// Adjusts `state.scroll_offset` so the keyboard-selected job row stays visible in `viewport_h`.
pub fn ensure_selected_job_visible(state: &mut UiState, snap: &Snapshot, viewport_h: f32) {
    if state.show_all_processes || viewport_h <= 0.0 {
        return;
    }
    let groups = filter_project_jobs(snap, &state.search_query);
    let total_h: f32 = groups
        .iter()
        .map(|(p, jobs)| {
            let wd = if p.watchdog_note.is_some() { 20.0 } else { 0.0 };
            30.0 + wd + (jobs.len().max(1) as f32) * 40.0 + 10.0
        })
        .sum::<f32>()
        + 6.0;
    let max_scroll = (total_h - viewport_h).max(0.0);
    state.max_scroll = max_scroll;

    let mut y = 0.0f32;
    let mut flat_idx = 0usize;
    for (proj, jobs) in groups {
        let wd_h = if proj.watchdog_note.is_some() { 20.0 } else { 0.0 };
        let block_h = 30.0 + wd_h + (jobs.len().max(1) as f32) * 40.0 + 4.0;
        for j_idx in 0..jobs.len() {
            if flat_idx == state.selected_index {
                let row_top = y + 28.0 + wd_h + (j_idx as f32) * 40.0;
                let visible_top = if j_idx == 0 { y } else { row_top };
                let row_bot = row_top + 40.0;
                if visible_top < state.scroll_offset {
                    state.scroll_offset = visible_top.max(0.0);
                } else if row_bot > state.scroll_offset + viewport_h {
                    state.scroll_offset = (row_bot - viewport_h).max(0.0);
                }
                state.scroll_offset = state.scroll_offset.clamp(0.0, max_scroll);
                return;
            }
            flat_idx += 1;
        }
        y += block_h + 6.0;
    }
}

/// Deletes the trailing whitespace and last word or path segment from a text buffer (`Ctrl+Backspace`).
pub fn pop_last_word(s: &mut String) {
    while s.chars().last().is_some_and(char::is_whitespace) {
        s.pop();
    }
    let is_sep = |c: char| matches!(c, '\\' | '/' | ':' | '.' | '-' | '_');
    while s.chars().last().is_some_and(is_sep) {
        s.pop();
    }
    while s
        .chars()
        .last()
        .is_some_and(|c| !c.is_whitespace() && !is_sep(c))
    {
        s.pop();
    }
}

/// Computes the number of wrapped tab rows needed for a project's jobs in `Detail` view.
pub fn detail_tab_row_count(jobs: &[Job], panel_w: f32) -> usize {
    if jobs.len() <= 1 {
        return 0;
    }
    let mut tx = 12.0_f32;
    let mut rows = 1_usize;
    for j in jobs {
        let tab_w = ((j.title.chars().count() as f32) * 6.5 + 22.0).clamp(64.0, 128.0);
        if tx > 12.0 && tx + tab_w > panel_w - 12.0 {
            tx = 12.0;
            rows += 1;
        }
        tx += tab_w + 6.0;
    }
    rows
}

/// Computes target `(width_dip, height_dip, radius_dip)` for the current `ViewMode`, `DockPosition`, & `Snapshot`.
pub fn target_dimensions(
    state: &UiState,
    snap: &Snapshot,
    dock: DockPosition,
    work_h_dip: f32,
) -> (f32, f32, f32) {
    let active_alert = snap.alerts.iter().any(|a| !a.dismissed);
    let max_panel_h = (work_h_dip - 20.0).max(400.0);
    match &state.mode {
        ViewMode::Collapsed => match dock {
            DockPosition::TopCenter => {
                if active_alert {
                    (356.0, 32.0, 16.0)
                } else {
                    (288.0, 30.0, 15.0)
                }
            }
            DockPosition::RightEdge => {
                let rail_h = (work_h_dip - 12.0).clamp(280.0, 1800.0);
                (42.0, rail_h, 12.0)
            }
        },
        ViewMode::HoverPeek => {
            let count = snap
                .projects
                .iter()
                .filter(|p| !p.jobs.is_empty())
                .count()
                .clamp(1, 3) as f32;
            (396.0, 82.0 + count * 34.0, 15.0)
        }
        ViewMode::Expanded => {
            let alert_h = if active_alert { 48.0 } else { 0.0 };
            let rename_h = if state.rename_draft.is_some() { 42.0 } else { 0.0 };
            let add_h = if state.add_job_draft.is_some() { 104.0 } else { 0.0 };
            let content_h = if state.show_all_processes {
                (snap.all_procs.len().clamp(1, 9) as f32) * 38.0 + 28.0
            } else {
                let groups = filter_project_jobs(snap, &state.search_query);
                if groups.is_empty() {
                    64.0
                } else {
                    groups
                        .iter()
                        .map(|(p, jobs)| {
                            let wd = if p.watchdog_note.is_some() { 20.0 } else { 0.0 };
                            30.0 + wd + (jobs.len().max(1) as f32) * 40.0 + 10.0
                        })
                        .sum::<f32>()
                        + 6.0
                }
            };
            let total_h = (254.0 + alert_h + rename_h + add_h + content_h + 26.0)
                .clamp(400.0, max_panel_h.min(740.0));
            (536.0, total_h, 16.0)
        }
        ViewMode::Detail { project_id, job_id } => {
            let proj_opt = snap.projects.iter().find(|p| &p.id == project_id);
            let proc_count = proj_opt
                .map(|p| {
                    if p.jobs.len() > 1 {
                        if let Some(jid) = job_id {
                            if let Some(j) = p.jobs.iter().find(|j| &j.id == jid) {
                                return j.procs.len();
                            }
                        } else if let Some(j) = p.jobs.first() {
                            return j.procs.len();
                        }
                    }
                    p.procs.len()
                })
                .unwrap_or(1)
                .clamp(1, 4) as f32;
            let tab_rows = proj_opt
                .map(|p| detail_tab_row_count(&p.jobs, 540.0))
                .unwrap_or(0);
            let tabs_h = if tab_rows > 0 {
                (tab_rows as f32) * 26.0 + 2.0
            } else {
                0.0
            };
            let active_has_ports = proj_opt.is_some_and(|p| {
                let active_job = job_id
                    .as_ref()
                    .and_then(|jid| p.jobs.iter().find(|j| &j.id == jid))
                    .or_else(|| p.jobs.first());
                active_job.is_some_and(|j| !j.ports.is_empty()) || !p.ports.is_empty()
            });
            let ports_h = if active_has_ports { 26.0 } else { 0.0 };
            let total_h =
                (44.0 + tabs_h + ports_h + 76.0 + 184.0 + 24.0 + proc_count * 46.0 + 32.0)
                    .clamp(430.0, max_panel_h.min(740.0));
            (540.0, total_h, 16.0)
        }
        ViewMode::Settings => (520.0, 612.0_f32.min(max_panel_h), 16.0),
    }
}

pub fn format_ram(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2}G", mb / 1024.0)
    } else if mb >= 100.0 {
        format!("{:.0}M", mb)
    } else {
        format!("{:.1}M", mb)
    }
}

pub fn format_uptime(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_scale_by_mode() {
        let mut state = UiState::default();
        let snap = Snapshot::default();
        assert_eq!(
            target_dimensions(&state, &snap, DockPosition::TopCenter, 1040.0),
            (288.0, 30.0, 15.0)
        );
        assert_eq!(
            target_dimensions(&state, &snap, DockPosition::RightEdge, 1040.0),
            (42.0, 1028.0, 12.0)
        );
        state.mode = ViewMode::Expanded;
        let (w, h, r) = target_dimensions(&state, &snap, DockPosition::TopCenter, 1040.0);
        assert_eq!(w, 536.0);
        assert!(h >= 400.0);
        assert_eq!(r, 16.0);
    }

    #[test]
    fn turkish_search_matches_case_insensitively_and_scroll_tracks_selection() {
        let mk_job = |id: &str, title: &str| Job {
            id: JobId(id.to_string()),
            project: ProjectId("rule:özel işler".to_string()),
            project_name: "Özel İşler".to_string(),
            title: title.to_string(),
            subtitle: "Özel İşler › bot".to_string(),
            icon: None,
            color: None,
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: 10,
            status: ProjectStatus::Healthy,
            last_log: None,
            crash_summary: None,
            root: None,
            primary_key: ProcessKey::new(1, 1),
            match_hint: title.to_string(),
            log_path: None,
            custom_command: None,
            is_watched: true,
            can_kill: true,
            can_restart: false,
        };
        let jobs: Vec<Job> = (0..8)
            .map(|i| mk_job(&format!("j{i}"), &format!("İşçi {i} GÜNCELLEME")))
            .collect();
        let snap = Snapshot {
            projects: vec![ProjectGroup {
                id: ProjectId("rule:özel işler".to_string()),
                name: "Özel İşler".to_string(),
                root: None,
                git_branch: None,
                icon: None,
                color: None,
                pinned: false,
                status: ProjectStatus::Healthy,
                cpu: 0.0,
                ram: 0,
                ports: Vec::new(),
                jobs,
                watchdog_note: None,
                procs: Vec::new(),
                cpu_history: [0; 30],
                ram_history_mb: [0; 30],
            }],
            ..Default::default()
        };

        assert_eq!(filter_project_jobs(&snap, "özel").len(), 1);
        assert_eq!(filter_project_jobs(&snap, "iş").len(), 1);
        assert_eq!(filter_project_jobs(&snap, "İSTANBUL").len(), 0);
        assert_eq!(filter_project_jobs(&snap, "güncelleme").len(), 1);

        let mut ui = UiState {
            mode: ViewMode::Expanded,
            selected_index: 6,
            ..Default::default()
        };
        ensure_selected_job_visible(&mut ui, &snap, 160.0);
        assert!(ui.scroll_offset > 0.0);
        ui.selected_index = 0;
        ensure_selected_job_visible(&mut ui, &snap, 160.0);
        assert_eq!(ui.scroll_offset, 0.0);
    }

    #[test]
    fn pop_last_word_and_detail_tab_row_count_work() {
        let mut s = r"C:\Users\Dev\bot.py".to_string();
        pop_last_word(&mut s);
        assert_eq!(s, r"C:\Users\Dev\bot.");
        pop_last_word(&mut s);
        assert_eq!(s, r"C:\Users\Dev\");
        pop_last_word(&mut s);
        assert_eq!(s, r"C:\Users\");

        let mut spaces = "python server.py --port  ".to_string();
        pop_last_word(&mut spaces);
        assert_eq!(spaces, "python server.py --");

        let mk_job = |id: usize| Job {
            id: JobId(format!("j{id}")),
            project: ProjectId("p".to_string()),
            project_name: "P".to_string(),
            title: format!("Worker Service {id}"),
            subtitle: String::new(),
            icon: None,
            color: None,
            procs: Vec::new(),
            cpu: 0.0,
            ram: 0,
            ports: Vec::new(),
            uptime: 1,
            status: ProjectStatus::Healthy,
            last_log: None,
            crash_summary: None,
            root: None,
            primary_key: ProcessKey::new(1, 1),
            match_hint: String::new(),
            log_path: None,
            custom_command: None,
            is_watched: false,
            can_kill: true,
            can_restart: false,
        };
        let six_jobs: Vec<Job> = (1..=6).map(mk_job).collect();
        assert!(detail_tab_row_count(&six_jobs, 540.0) >= 2);
        assert_eq!(detail_tab_row_count(&six_jobs[..1], 540.0), 0);
    }

    #[test]
    fn stagger_row_progress_cascades_monotonically_and_settles_at_one() {
        let (a0, dy0) = stagger_row_progress(0.0, 0);
        assert_eq!((a0, dy0), (0.0, 8.0));

        // Earlier rows lead later rows during mid-entry.
        let (a_early, dy_early) = stagger_row_progress(0.35, 0);
        let (a_late, dy_late) = stagger_row_progress(0.35, 5);
        assert!(a_early > a_late);
        assert!(dy_early < dy_late);

        // Increasing enter_t monotonically increases alpha and shrinks offset.
        let mut prev_a = 0.0f32;
        for step in 0..=20 {
            let t = step as f32 / 20.0;
            let (a, dy) = stagger_row_progress(t, 3);
            assert!(a >= prev_a - 1e-6);
            assert!((0.0..=8.0001).contains(&dy));
            prev_a = a;
        }

        assert_eq!(stagger_row_progress(0.996, 8), (1.0, 0.0));
    }
}
