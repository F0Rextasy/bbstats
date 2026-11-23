//! Discovery of known-but-stopped automation jobs (configured via `labels.toml`
//! `scan_dir` / `pid_file` / `scheduled_task`) — runs ONLY while expanded (60s TTL).

use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::engine::labels::{expand_win_env_vars, LabelsStore};
use crate::engine::project_resolver::tail_last_4kb_line;
use crate::model::{JobId, Language, ProjectId};

const SCAN_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct KnownStoppedJob {
    pub job_id: JobId,
    pub project_id: ProjectId,
    pub project_name: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub root: PathBuf,
    pub match_hint: String,
    pub last_log: Option<String>,
    pub log_path: Option<PathBuf>,
    pub custom_command: Option<String>,
    pub is_watched: bool,
}

#[derive(Debug, Clone, Default)]
pub struct KnownJobsSnapshot {
    pub jobs: Vec<KnownStoppedJob>,
    pub watchdog_notes: HashMap<ProjectId, String>,
}

pub struct KnownJobsScanner {
    last_scan: Option<Instant>,
    last_lang: Option<Language>,
    cached: KnownJobsSnapshot,
}

impl KnownJobsScanner {
    pub fn new() -> Self {
        Self {
            last_scan: None,
            last_lang: None,
            cached: KnownJobsSnapshot::default(),
        }
    }

    pub fn get_or_refresh(
        &mut self,
        allow_io: bool,
        labels: &LabelsStore,
        lang: Language,
    ) -> &KnownJobsSnapshot {
        let lang_changed = self.last_lang != Some(lang);
        if allow_io
            && (lang_changed
                || self
                    .last_scan
                    .is_none_or(|t| t.elapsed() >= SCAN_INTERVAL))
        {
            self.cached = scan_known_jobs_from_env(labels, lang);
            self.last_scan = Some(Instant::now());
            self.last_lang = Some(lang);
        }
        &self.cached
    }
}

fn scan_known_jobs_from_env(labels: &LabelsStore, lang: Language) -> KnownJobsSnapshot {
    scan_known_jobs_with_root(labels, None, lang)
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, seen: &mut HashSet<String>, dir: PathBuf) {
    if !dir.is_dir() {
        return;
    }
    let norm = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    if seen.insert(norm) {
        dirs.push(dir);
    }
}

/// Scans configured `scan_dir` / `pid_file` parent directories and `default_root/*` subdirectories.
pub fn scan_known_jobs_with_root(
    labels: &LabelsStore,
    default_root: Option<&Path>,
    lang: Language,
) -> KnownJobsSnapshot {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if let Some(root) = default_root {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                push_unique_dir(&mut dirs, &mut seen, entry.path());
            }
        }
    }

    for rule in labels.rules() {
        if let Some(ref sd) = rule.scan_dir {
            push_unique_dir(&mut dirs, &mut seen, PathBuf::from(expand_win_env_vars(sd)));
        } else if let Some(ref pf) = rule.pid_file {
            let expanded = PathBuf::from(expand_win_env_vars(pf));
            if let Some(parent) = expanded.parent() {
                push_unique_dir(&mut dirs, &mut seen, parent.to_path_buf());
            }
        }
    }

    let mut snap = KnownJobsSnapshot::default();
    for dir in dirs {
        scan_known_dir(&dir, labels, lang, &mut snap);
    }
    snap
}

fn scan_known_dir(dir: &Path, labels: &LabelsStore, lang: Language, snap: &mut KnownJobsSnapshot) {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("local")
        .to_string();
    let norm_dir = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    let proj_id = ProjectId(format!("path:{norm_dir}"));

    let fallback_log = ["auto-console.log", "service.log", "bot.log"]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file());

    let mut matched_task_name = find_scheduled_task_for_dir(&norm_dir, labels);

    if let Ok(files) = fs::read_dir(dir) {
        for f in files.flatten() {
            let fpath = f.path();
            if !fpath.is_file() {
                continue;
            }
            let fname = fpath
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let lower = fname.to_ascii_lowercase();

            // Periodic watchdog scripts are reported via watchdog_notes, not as Stopped job rows.
            if lower == "watchdog.py" || lower == "watchdog.js" || lower.starts_with("start-") {
                continue;
            }
            if !matches!(
                fpath
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref(),
                Some("py" | "pyw" | "js" | "mjs" | "sh" | "ps1")
            ) {
                continue;
            }

            let full_str = fpath.to_string_lossy().into_owned();
            // Only emit scripts explicitly registered in labels.toml (specific script match, not folder catch-all)
            // so helper/benchmark scripts never flood Stopped rows.
            let Some(rule) = labels.match_specific_script_rule(&full_str, &fname) else {
                continue;
            };
            if matched_task_name.is_none() {
                matched_task_name = rule.scheduled_task.clone();
            }

            let project_name = rule
                .project
                .as_ref()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| humanize_folder_name(&dir_name));

            let title = if !rule.job.trim().is_empty() {
                rule.job.clone()
            } else {
                let stem = fpath.file_stem().and_then(|s| s.to_str()).unwrap_or(&fname);
                humanize_folder_name(stem)
            };

            let job_id = JobId(format!(
                "{}::script:{}",
                proj_id.0,
                full_str.replace('/', "\\").to_ascii_lowercase()
            ));

            let rule_log = rule
                .log_file
                .as_deref()
                .map(|p| PathBuf::from(expand_win_env_vars(p)));
            let effective_log = rule_log.or_else(|| {
                if lower.contains("updater") || lower.contains("service") {
                    fallback_log.clone()
                } else {
                    None
                }
            });
            let last_log = effective_log.as_deref().and_then(tail_last_4kb_line);
            let sep = std::path::MAIN_SEPARATOR;

            snap.jobs.push(KnownStoppedJob {
                job_id,
                project_id: proj_id.clone(),
                project_name: project_name.clone(),
                title,
                subtitle: format!("{project_name} › ...{sep}{dir_name}"),
                icon: rule.icon.clone(),
                color: rule.color.clone(),
                root: dir.to_path_buf(),
                match_hint: fname,
                last_log,
                log_path: effective_log,
                custom_command: rule.command.clone(),
                is_watched: rule.watch.unwrap_or(false),
            });
        }
    }

    if let Some(note) = read_watchdog_state_note(dir, lang) {
        snap.watchdog_notes.insert(proj_id, note);
    } else if let Some(task_name) = matched_task_name {
        if let Some(note) = query_scheduled_task_note(dir, &task_name, lang) {
            snap.watchdog_notes.insert(proj_id, note);
        }
    }
}

fn find_scheduled_task_for_dir(norm_dir: &str, labels: &LabelsStore) -> Option<String> {
    for rule in labels.rules() {
        let Some(ref task) = rule.scheduled_task else {
            continue;
        };
        if let Some(ref sd) = rule.scan_dir {
            let s = expand_win_env_vars(sd)
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_ascii_lowercase();
            if s == norm_dir {
                return Some(task.clone());
            }
        }
        if let Some(ref pf) = rule.pid_file {
            let p = PathBuf::from(expand_win_env_vars(pf));
            if let Some(parent) = p.parent() {
                let s = parent
                    .to_string_lossy()
                    .replace('/', "\\")
                    .trim_end_matches('\\')
                    .to_ascii_lowercase();
                if s == norm_dir {
                    return Some(task.clone());
                }
            }
        }
    }
    None
}

fn format_watchdog_note(lang: Language, hhmm: Option<String>) -> String {
    match hhmm {
        Some(t) => format!("Watchdog · {} {t}", lang.pick("last run", "son çalışma")),
        None => lang.pick("Watchdog active", "Watchdog aktif").to_string(),
    }
}

fn read_watchdog_state_note(dir: &Path, lang: Language) -> Option<String> {
    let state_path = dir.join("watchdog-state.json");
    if !state_path.is_file() {
        return None;
    }
    let content = fs::read_to_string(&state_path).ok()?;
    let hhmm = extract_hhmm_from_json(&content);
    Some(format_watchdog_note(lang, hhmm))
}

fn query_scheduled_task_note(dir: &Path, task_name: &str, lang: Language) -> Option<String> {
    if task_name.trim().is_empty() || !dir.is_dir() {
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        // Fallback: query Windows Task Scheduler once per 60s without creating a console window,
        // using `/FO CSV /NH /V` so the "Last Run Time" column (index 5) is locale-independent
        // and immune to Turkish/OEM CP857 codepage garbling.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("schtasks.exe");
        cmd.args(["/Query", "/TN", task_name.trim(), "/FO", "CSV", "/NH", "/V"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);

        if let Some(stdout_bytes) = run_command_bounded(cmd, Duration::from_millis(1500)) {
            let text = String::from_utf8_lossy(&stdout_bytes);
            let hhmm = parse_schtasks_last_run_hhmm(&text);
            return Some(format_watchdog_note(lang, hhmm));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("launchctl");
        cmd.args(["list", task_name.trim()])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(stdout_bytes) = run_command_bounded(cmd, Duration::from_millis(1500)) {
            if !stdout_bytes.is_empty() {
                return Some(format!("LaunchAgent · {}", task_name.trim()));
            }
        }
    }

    if dir.join("watchdog.py").is_file() || dir.join("watchdog.js").is_file() {
        Some(format_watchdog_note(lang, None))
    } else {
        None
    }
}

#[cfg(any(target_os = "windows", test))]
fn parse_schtasks_last_run_hhmm(text: &str) -> Option<String> {
    // 1. Locale-independent CSV `/FO CSV /NH /V`: column 5 is always "Last Run Time".
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('"') {
            let cols = split_csv_quoted_line(trimmed);
            if let Some(col) = cols.get(5) {
                if let Some(hhmm) = extract_hhmm_from_str(col) {
                    return Some(hhmm);
                }
            }
        }
    }
    // 2. Fallback for LIST format (with Turkish-safe folding).
    text.lines().find_map(|line| {
        let folded = crate::model::fold_search_lower(line);
        if folded.contains("last run time") || folded.contains("son calisma") || folded.contains("son çalışma") {
            extract_hhmm_from_str(line)
        } else {
            None
        }
    })
}

#[cfg(any(target_os = "windows", test))]
fn split_csv_quoted_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_q && chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_q = !in_q;
                }
            }
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run_command_bounded(mut cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
    const MAX_STDOUT_BYTES: u64 = 256 * 1024;
    let mut child = cmd.spawn().ok()?;
    // Drain stdout concurrently on a helper thread so verbose output cannot fill
    // the OS pipe buffer and deadlock `try_wait()`.
    let stdout_handle = child.stdout.take().map(|out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.take(MAX_STDOUT_BYTES).read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout_bytes = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    if status.is_some_and(|st| st.success()) {
        Some(stdout_bytes)
    } else {
        None
    }
}

fn extract_hhmm_from_json(text: &str) -> Option<String> {
    // Prefer the last `"t"` or `"last_run"` timestamp in the JSON so earlier alert timestamps are skipped.
    if let Some(pos) = text.rfind("\"t\"").or_else(|| text.rfind("\"last_run\"")) {
        if let Some(hhmm) = extract_hhmm_from_str(&text[pos..]) {
            return Some(hhmm);
        }
    }
    extract_hhmm_from_str(text)
}

fn extract_hhmm_from_str(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for i in 0..bytes.len().saturating_sub(4) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2] == b':'
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4].is_ascii_digit()
        {
            let hh: u8 = std::str::from_utf8(&bytes[i..i + 2]).ok()?.parse().ok()?;
            let mm: u8 = std::str::from_utf8(&bytes[i + 3..i + 5]).ok()?.parse().ok()?;
            if hh < 24 && mm < 60 {
                return Some(format!("{hh:02}:{mm:02}"));
            }
        }
    }
    None
}

fn humanize_folder_name(raw: &str) -> String {
    let spaced = raw.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::labels::LabelRule;

    #[test]
    fn scans_configured_and_local_dirs_for_stopped_jobs_and_watchdog_note() {
        let base = std::env::temp_dir().join(format!("bbstats_known_dir_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let bot_dir = base.join("worker-bot");
        let custom_dir = std::env::temp_dir().join(format!("bbstats_custom_scan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&custom_dir);
        fs::create_dir_all(&bot_dir).unwrap();
        fs::create_dir_all(&custom_dir).unwrap();

        fs::write(bot_dir.join("auto_updater.py"), "# updater").unwrap();
        fs::write(bot_dir.join("autoheal.py"), "# heal").unwrap();
        fs::write(bot_dir.join("watchdog.py"), "# watchdog").unwrap();
        fs::write(bot_dir.join("start-auto-bot.cmd"), "pythonw auto_updater.py").unwrap();
        // Helper scripts in the same directory must NOT flood Stopped jobs:
        fs::write(bot_dir.join("adblock.py"), "# helper").unwrap();
        fs::write(bot_dir.join("bench_runner.py"), "# bench").unwrap();
        fs::write(bot_dir.join("check.py"), "# check").unwrap();
        fs::write(bot_dir.join("auto-console.log"), "19:10 checking updates\n19:14 ok\n").unwrap();
        fs::write(
            bot_dir.join("watchdog-state.json"),
            r#"{"last_run":"2026-11-16T19:14:05"}"#,
        )
        .unwrap();

        fs::write(custom_dir.join("sync_job.py"), "# sync").unwrap();

        let labels = LabelsStore::from_rules_for_test(vec![
            LabelRule {
                match_substr: r"worker-bot\auto_updater".to_string(),
                project: Some("WorkerBot".to_string()),
                job: "Auto update botu".to_string(),
                icon: Some("🤖".to_string()),
                color: Some("#F5B73B".to_string()),
                ..Default::default()
            },
            LabelRule {
                match_substr: r"worker-bot\autoheal".to_string(),
                project: Some("WorkerBot".to_string()),
                job: "Auto heal".to_string(),
                icon: Some("🤖".to_string()),
                ..Default::default()
            },
            LabelRule {
                match_substr: "sync_job.py".to_string(),
                project: Some("CustomScan".to_string()),
                job: "Sync Worker".to_string(),
                scan_dir: Some(custom_dir.to_string_lossy().into_owned()),
                ..Default::default()
            },
        ]);
        let snap = scan_known_jobs_with_root(&labels, Some(&base), Language::En);

        assert_eq!(snap.jobs.len(), 3, "watchdog.py and start-*.cmd must not be job rows");
        let titles: Vec<&str> = snap.jobs.iter().map(|j| j.title.as_str()).collect();
        assert!(titles.contains(&"Auto update botu"));
        assert!(titles.contains(&"Auto heal"));
        assert!(titles.contains(&"Sync Worker"));
        for job in snap.jobs.iter().filter(|j| j.project_name == "WorkerBot") {
            assert_eq!(job.icon.as_deref(), Some("🤖"));
            if job.title == "Auto update botu" {
                assert_eq!(job.last_log.as_deref(), Some("19:14 ok"));
            }
        }

        let note = snap.watchdog_notes.values().next().unwrap();
        assert_eq!(note, "Watchdog · last run 19:14");
        let snap_tr = scan_known_jobs_with_root(&labels, Some(&base), Language::Tr);
        assert_eq!(
            snap_tr.watchdog_notes.values().next().unwrap(),
            "Watchdog · son çalışma 19:14"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&custom_dir);
    }
}
