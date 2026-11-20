//! User-defined naming rules and PID state-file bindings loaded from the user's `bbstats/labels.toml`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

use crate::model::fold_search_lower;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LabelRule {
    #[serde(rename = "match")]
    pub match_substr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub job: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_transient: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_restart: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LabelsFile {
    #[serde(default)]
    rule: Vec<LabelRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFileBinding {
    pub pid: u32,
    pub running: Option<bool>,
    pub next_run: Option<String>,
    pub last_log: Option<String>,
    pub mtime: Option<SystemTime>,
    pub rule: LabelRule,
    pub state_dir: PathBuf,
    pub log_file: Option<PathBuf>,
}

pub struct LabelsStore {
    path: Option<PathBuf>,
    rules: Vec<LabelRule>,
    last_mtime: Option<SystemTime>,
}

impl LabelsStore {
    pub fn new() -> Self {
        let path = default_labels_path();
        if let Some(p) = &path {
            seed_or_upgrade_default_labels(p);
        }
        let mut store = Self {
            path,
            rules: Vec::new(),
            last_mtime: None,
        };
        store.reload_if_changed(true);
        store
    }

    #[cfg(test)]
    pub fn from_rules_for_test(rules: Vec<LabelRule>) -> Self {
        Self {
            path: None,
            rules,
            last_mtime: None,
        }
    }

    pub fn rules(&self) -> &[LabelRule] {
        &self.rules
    }

    /// Checks `labels.toml` mtime and reloads if modified. Returns `true` if rules changed.
    pub fn reload_if_changed(&mut self, force: bool) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        if !force && mtime == self.last_mtime {
            return false;
        }
        self.last_mtime = mtime;

        let new_rules = match fs::read_to_string(path) {
            Ok(content) => match parse_labels_toml_strict(&content) {
                Ok(parsed) => parsed,
                Err(err) => {
                    crate::config::log_error(&format!(
                        "corrupt labels.toml at {}: {err}",
                        path.display()
                    ));
                    return false;
                }
            },
            Err(_) => Vec::new(),
        };
        let changed = new_rules != self.rules;
        self.rules = new_rules;
        changed || force
    }

    /// Finds the first matching rule (case-insensitive, separator-agnostic substring over cmdline, cwd, or image path).
    pub fn match_rule(&self, haystacks: &[&str]) -> Option<&LabelRule> {
        let folded_haystacks: Vec<String> = haystacks
            .iter()
            .map(|h| fold_search_lower(h).replace('\\', "/"))
            .collect();
        for rule in &self.rules {
            let needle = fold_search_lower(rule.match_substr.trim()).replace('\\', "/");
            if needle.is_empty() {
                continue;
            }
            if folded_haystacks.iter().any(|h| h.contains(&needle)) {
                return Some(rule);
            }
        }
        None
    }

    /// Matches only rules whose `match_substr` targets the script name itself (not just parent folder).
    pub fn match_specific_script_rule(&self, full_path: &str, file_name: &str) -> Option<&LabelRule> {
        let low_full = fold_search_lower(full_path).replace('\\', "/");
        let low_file = fold_search_lower(file_name);
        let stem = Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(fold_search_lower)
            .unwrap_or_default();
        for rule in &self.rules {
            let needle = fold_search_lower(rule.match_substr.trim()).replace('\\', "/");
            if needle.is_empty() {
                continue;
            }
            let needle_targets_script = (!stem.is_empty() && needle.contains(&stem))
                || needle.contains(&low_file);
            if needle_targets_script && (low_full.contains(&needle) || low_file.contains(&needle)) {
                return Some(rule);
            }
        }
        None
    }

    /// Reads any configured `pid_file` JSON files and returns active `PID -> StateFileBinding` hints.
    pub fn read_pid_bindings(&self) -> Vec<StateFileBinding> {
        let mut out = Vec::new();
        for rule in &self.rules {
            let Some(raw_pid_file) = &rule.pid_file else {
                continue;
            };
            let path = PathBuf::from(expand_win_env_vars(raw_pid_file));
            if let Some(binding) = parse_state_file_binding(&path, rule) {
                out.push(binding);
            }
        }
        out
    }

    /// Persists a new/updated rule to `%APPDATA%\bbstats\labels.toml` atomically,
    /// preserving existing optional metadata when incoming fields are `None`.
    pub fn save_rule(&mut self, rule: LabelRule) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("APPDATA path unavailable".to_string());
        };
        save_rule_to_path(path, rule)?;
        self.reload_if_changed(true);
        Ok(())
    }

    pub fn remove_rule(&mut self, match_substr: &str) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("APPDATA path unavailable".to_string());
        };
        remove_rule_from_path(path, match_substr)?;
        self.reload_if_changed(true);
        Ok(())
    }
}

fn load_existing_rules_strict(path: &Path) -> Result<Vec<LabelRule>, String> {
    match fs::read_to_string(path) {
        Ok(content) => parse_labels_toml_strict(&content).map_err(|err| {
            let bak = crate::config::backup_corrupt_toml(path);
            let msg = format!(
                "corrupt labels.toml ({err}); backed up to {}",
                bak.display()
            );
            crate::config::log_error(&msg);
            msg
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn remove_rule_from_path(path: &Path, match_substr: &str) -> Result<(), String> {
    let mut rules = load_existing_rules_strict(path)?;
    let before = rules.len();
    rules.retain(|r| !r.match_substr.eq_ignore_ascii_case(match_substr));
    if rules.len() == before {
        return Ok(());
    }
    write_rules_atomically(path, rules)
}

fn write_rules_atomically(path: &Path, rules: Vec<LabelRule>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let file = LabelsFile { rule: rules };
    let serialized = toml::to_string_pretty(&file).map_err(|e| format!("toml serialize: {e}"))?;
    let tmp_path = path.with_extension("toml.tmp");
    fs::write(&tmp_path, serialized).map_err(|e| format!("write tmp: {e}"))?;
    fs::rename(&tmp_path, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

pub fn save_rule_to_path(path: &Path, rule: LabelRule) -> Result<(), String> {
    let mut rules = load_existing_rules_strict(path)?;
    merge_or_prepend_rule(&mut rules, rule);
    write_rules_atomically(path, rules)
}

fn merge_or_prepend_rule(rules: &mut Vec<LabelRule>, rule: LabelRule) {
    if let Some(existing) = rules
        .iter_mut()
        .find(|r| r.match_substr.eq_ignore_ascii_case(&rule.match_substr))
    {
        existing.job = rule.job;
        if rule.project.is_some() {
            existing.project = rule.project;
        }
        if rule.icon.is_some() {
            existing.icon = rule.icon;
        }
        if rule.color.is_some() {
            existing.color = rule.color;
        }
        if rule.pid_file.is_some() {
            existing.pid_file = rule.pid_file;
        }
        if rule.pid_key.is_some() {
            existing.pid_key = rule.pid_key;
        }
        if rule.log_file.is_some() {
            existing.log_file = rule.log_file;
        }
        if rule.command.is_some() {
            existing.command = rule.command;
        }
        if rule.working_dir.is_some() {
            existing.working_dir = rule.working_dir;
        }
        if rule.watch.is_some() {
            existing.watch = rule.watch;
        }
        if rule.scan_dir.is_some() {
            existing.scan_dir = rule.scan_dir;
        }
        if rule.scheduled_task.is_some() {
            existing.scheduled_task = rule.scheduled_task;
        }
        if rule.hide_transient.is_some() {
            existing.hide_transient = rule.hide_transient;
        }
        if rule.auto_restart.is_some() {
            existing.auto_restart = rule.auto_restart;
        }
    } else {
        rules.insert(0, rule);
    }
}

pub fn parse_state_file_binding(path: &Path, rule: &LabelRule) -> Option<StateFileBinding> {
    const MAX_STATE_BYTES: u64 = 512 * 1024;
    let file = fs::File::open(path).ok()?;
    let mtime = file.metadata().and_then(|m| m.modified()).ok();
    let mut buf = Vec::new();
    file.take(MAX_STATE_BYTES).read_to_end(&mut buf).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&buf).ok()?;

    let pid_key = rule.pid_key.as_deref().unwrap_or("pid");
    let pid = root
        .get(pid_key)
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())?;
    if pid <= 4 {
        return None;
    }
    // Note: Do NOT skip when `running: false` — idle daemons between hourly scans
    // keep `"running": false` while their PID stays alive waiting for the next timer.
    let running = root.get("running").and_then(|v| v.as_bool());
    let next_run = extract_next_run(&root, "nextRun").or_else(|| extract_next_run(&root, "next_run"));
    let last_log = root
        .get("logs")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().rev().find_map(|entry| {
                entry
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.chars().take(120).collect())
            })
        });
    let state_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let log_file = rule
        .log_file
        .as_deref()
        .map(|p| PathBuf::from(expand_win_env_vars(p)));

    Some(StateFileBinding {
        pid,
        running,
        next_run,
        last_log,
        mtime,
        rule: rule.clone(),
        state_dir,
        log_file,
    })
}

fn extract_next_run(root: &serde_json::Value, key: &str) -> Option<String> {
    let val = root.get(key)?;
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    val.as_u64().map(format_unix_epoch_hhmm)
}

fn format_unix_epoch_hhmm(raw: u64) -> String {
    let epoch_secs = if raw > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    let offset_secs = local_utc_offset_secs();
    let local_secs = (epoch_secs as i64 + offset_secs).rem_euclid(86_400) as u64;
    let hh = local_secs / 3600;
    let mm = (local_secs % 3600) / 60;
    format!("{hh:02}:{mm:02}")
}

#[cfg(target_os = "windows")]
fn local_utc_offset_secs() -> i64 {
    unsafe {
        let utc = GetSystemTime();
        let local = GetLocalTime();
        let utc_min = (utc.wHour as i64) * 60 + (utc.wMinute as i64);
        let local_min = (local.wHour as i64) * 60 + (local.wMinute as i64);
        let mut diff = local_min - utc_min;
        if diff > 14 * 60 {
            diff -= 24 * 60;
        } else if diff < -12 * 60 {
            diff += 24 * 60;
        }
        diff * 60
    }
}

#[cfg(not(target_os = "windows"))]
fn local_utc_offset_secs() -> i64 {
    unsafe {
        let now: libc::time_t = libc::time(std::ptr::null_mut());
        let mut tm_local: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&now, &mut tm_local).is_null() {
            tm_local.tm_gmtoff as i64
        } else {
            0
        }
    }
}

pub fn expand_win_env_vars(raw: &str) -> String {
    let mut s = raw.to_string();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let sep = if cfg!(windows) { '\\' } else { '/' };
            s = format!("{}{}{}", home.to_string_lossy(), sep, rest);
        }
    }
    for var in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "HOME"] {
        let token = format!("%{var}%");
        if s.contains(&token) {
            if let Ok(val) = std::env::var(var) {
                s = s.replace(&token, &val);
            }
        }
        let dollar_token = format!("${var}");
        if s.contains(&dollar_token) {
            if let Ok(val) = std::env::var(var) {
                s = s.replace(&dollar_token, &val);
            }
        }
    }
    s
}

pub fn default_labels_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|app| Path::new(&app).join("bbstats").join("labels.toml"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| {
            Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join("bbstats")
                .join("labels.toml")
        })
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|base| base.join("bbstats").join("labels.toml"))
    }
}

pub fn parse_labels_toml_strict(content: &str) -> Result<Vec<LabelRule>, String> {
    toml::from_str::<LabelsFile>(content)
        .map(|f| f.rule)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub fn parse_labels_toml(content: &str) -> Vec<LabelRule> {
    parse_labels_toml_strict(content).unwrap_or_default()
}

fn seed_or_upgrade_default_labels(path: &Path) {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = "# bbstats custom project/job rules (see assets/labels.example.toml)\n";
        let tmp = path.with_extension("toml.tmp");
        if fs::write(&tmp, content).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labels_toml_and_matches_case_insensitively() {
        let toml_text = r##"
[[rule]]
match = "worker-bot"
project = "WorkerBot"
job = "Auto update botu"
icon = "🤖"
color = "#F5B73B"
"##;
        let rules = parse_labels_toml(toml_text);
        assert_eq!(rules.len(), 1);
        let store = LabelsStore::from_rules_for_test(rules);

        let hit = store
            .match_rule(&[r"cmd /c node C:\Users\Dev\projects\Worker-Bot\bot.js"])
            .expect("should match worker-bot case-insensitively");
        assert_eq!(hit.project.as_deref(), Some("WorkerBot"));
        assert_eq!(hit.job, "Auto update botu");
        assert_eq!(hit.icon.as_deref(), Some("🤖"));
        assert_eq!(hit.color.as_deref(), Some("#F5B73B"));
    }

    #[test]
    fn parses_state_file_pid_binding_with_idle_running_false_numeric_next_run_and_brackets_in_logs() {
        let base = std::env::temp_dir().join(format!("bbstats_state_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let state_file = base.join("auto-state.json");
        fs::write(
            &state_file,
            r#"{"pid": 34968, "running": false, "intervalSec": 3600, "nextRun": 1794849728, "logs": [{"text": "[19:14] checked 12 series"}, {"text": "[19:15] idle ok"}]}"#,
        )
        .unwrap();

        let rule = LabelRule {
            match_substr: "worker-bot".to_string(),
            project: Some("WorkerBot".to_string()),
            job: "Auto update botu".to_string(),
            icon: Some("🤖".to_string()),
            pid_file: Some(state_file.to_string_lossy().into_owned()),
            pid_key: Some("pid".to_string()),
            ..Default::default()
        };
        let binding = parse_state_file_binding(&state_file, &rule).unwrap();
        assert_eq!(binding.pid, 34968);
        assert_eq!(binding.running, Some(false));
        assert!(binding.next_run.as_deref().is_some_and(|s| s.len() == 5 && s.contains(':')));
        assert_eq!(binding.last_log.as_deref(), Some("[19:15] idle ok"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn save_rule_preserves_existing_metadata_when_optional_fields_none() {
        let base = std::env::temp_dir().join(format!("bbstats_save_rule_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join("labels.toml");

        let seed = LabelRule {
            match_substr: r"worker-bot\auto_updater".to_string(),
            project: Some("WorkerBot".to_string()),
            job: "Auto update botu".to_string(),
            icon: Some("🤖".to_string()),
            color: Some("#F5B73B".to_string()),
            pid_file: Some(r"C:\state.json".to_string()),
            pid_key: Some("pid".to_string()),
            log_file: Some(r"C:\console.log".to_string()),
            scan_dir: Some(r"C:\bots\worker".to_string()),
            scheduled_task: Some("WorkerWatchdog".to_string()),
            hide_transient: Some(true),
            ..Default::default()
        };
        save_rule_to_path(&path, seed).unwrap();

        // Simulate UI right-click Rename sending `project: None, icon: None, color: None`.
        let rename = LabelRule {
            match_substr: r"worker-bot\auto_updater".to_string(),
            job: "Worker Updater".to_string(),
            ..Default::default()
        };
        save_rule_to_path(&path, rename).unwrap();

        let reloaded = parse_labels_toml(&fs::read_to_string(&path).unwrap());
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].job, "Worker Updater");
        assert_eq!(reloaded[0].project.as_deref(), Some("WorkerBot"));
        assert_eq!(reloaded[0].icon.as_deref(), Some("🤖"));
        assert_eq!(reloaded[0].color.as_deref(), Some("#F5B73B"));
        assert_eq!(reloaded[0].pid_file.as_deref(), Some(r"C:\state.json"));
        assert_eq!(reloaded[0].pid_key.as_deref(), Some("pid"));
        assert_eq!(reloaded[0].log_file.as_deref(), Some(r"C:\console.log"));
        assert_eq!(reloaded[0].scan_dir.as_deref(), Some(r"C:\bots\worker"));
        assert_eq!(reloaded[0].scheduled_task.as_deref(), Some("WorkerWatchdog"));
        assert_eq!(reloaded[0].hide_transient, Some(true));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn custom_watch_and_command_roundtrip_and_remove() {
        let base = std::env::temp_dir().join(format!("bbstats_watch_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join("labels.toml");

        let custom = LabelRule {
            match_substr: "my_worker.py".to_string(),
            project: Some("My Jobs".to_string()),
            job: "Worker".to_string(),
            command: Some("python my_worker.py".to_string()),
            working_dir: Some(r"C:\work".to_string()),
            watch: Some(true),
            ..Default::default()
        };
        save_rule_to_path(&path, custom).unwrap();
        let loaded = parse_labels_toml(&fs::read_to_string(&path).unwrap());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].watch, Some(true));
        assert_eq!(loaded[0].command.as_deref(), Some("python my_worker.py"));

        remove_rule_from_path(&path, "my_worker.py").unwrap();
        let after = parse_labels_toml(&fs::read_to_string(&path).unwrap());
        assert!(after.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn corrupt_labels_toml_is_backed_up_and_never_overwritten() {
        let base =
            std::env::temp_dir().join(format!("bbstats_corrupt_labels_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join("labels.toml");
        let corrupt = "[[rule]]\nmatch = [unclosed";
        fs::write(&path, corrupt).unwrap();

        let err = save_rule_to_path(
            &path,
            LabelRule {
                match_substr: "foo.py".to_string(),
                job: "Foo".to_string(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("corrupt labels.toml"));
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        assert_eq!(fs::read_to_string(base.join("labels.toml.bak")).unwrap(), corrupt);

        let _ = fs::remove_dir_all(&base);
    }
}
