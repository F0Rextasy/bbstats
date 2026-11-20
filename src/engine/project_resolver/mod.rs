//! Project & Job inference pipeline:
//! 1. User rules (`%APPDATA%\bbstats\labels.toml`)
//! 2. Manifests (`package.json`, `pyproject.toml`, `Cargo.toml`, `README.md`, `compose.yaml`, `go.mod`, `.git`)
//! 3. Human-readable script/module title normalization
//! 4. Project root folder resolution (skipping `.venv`, `node_modules`, `Scripts`, `runtime`, `scripts`, `src`)
//! 5. Noise filtering, wrapper collapsing, log-redirect extraction, and secret redaction.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::engine::actions::is_hard_blacklisted_name;
use crate::engine::collector::CollectedProcess;
use crate::engine::labels::{expand_win_env_vars, LabelRule, LabelsStore, StateFileBinding};
use crate::engine::win_nt::{
    inspect_process_peb, list_wsl_distros_from_registry, query_docker_containers_pipe,
    DockerContainerSummary, PebInspection,
};
use crate::model::{JobId, Language, ProcessKey, ProcessRole, ProjectId};

mod cmdline;
mod filters;
mod manifests;
mod redact;

pub use cmdline::{analyze_command_and_paths, extract_log_redirect_path, format_job_title};
#[cfg(target_os = "windows")]
pub use cmdline::split_cmdline_args;
pub use manifests::{inspect_directory_manifests, tail_last_4kb_line};
pub use redact::sanitize_cmdline;

#[cfg(not(target_os = "windows"))]
use cmdline::coerce_platform_path_ref;
use cmdline::{
    build_subtitle_path, discover_script_log_path, is_skippable_intermediate_dir,
    normalize_project_display_name,
};
use filters::{
    is_docker_process, is_fast_skip_noise, is_generic_dir, is_helper_process,
    is_ide_or_sidecar_process, is_interactive_shell_process, is_known_dev_runtime,
    is_stop_directory, is_system_or_app_location, is_user_project_location, is_wsl_vm_name,
    normalize_project_id,
};
use manifests::dir_name_or;
use redact::strip_shell_redirects;

const MAX_WALK_LEVELS: usize = 6;
const PATH_CACHE_TTL: Duration = Duration::from_secs(300);
const PATH_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(45);
const MAX_PATH_CACHE_ENTRIES: usize = 512;
const MAX_LOG_TAIL_CACHE_ENTRIES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceSource {
    UserLabelRule,
    DockerComposeLabel,
    ManifestPackageJson,
    ManifestCargoToml,
    ManifestPyProject,
    ManifestReadme,
    ManifestCompose,
    ManifestGoMod,
    ManifestDotNetOrJvm,
    GitRoot,
    ParentInherited,
    CommandLineHeuristic,
    WslVm,
    FallbackExeName,
}

#[derive(Debug, Clone)]
pub struct RestartRecipe {
    pub raw_cmdline: String,
    pub working_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub image_name: String,
    pub working_dir: Option<PathBuf>,
    pub sanitized_cmdline: Option<String>,
    pub access_limited: bool,
    pub project_id: ProjectId,
    pub project_display_name: String,
    pub project_root: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub job_id: JobId,
    pub job_title: String,
    pub job_subtitle: String,
    pub job_icon: Option<String>,
    pub job_color: Option<String>,
    pub match_hint: String,
    pub log_path: Option<PathBuf>,
    pub inline_last_log: Option<String>,
    pub custom_command: Option<String>,
    pub is_watched: bool,
    pub role: ProcessRole,
    pub role_label: String,
    pub inference_source: InferenceSource,
    /// True for OS/driver/IDE/interactive-shell noise hidden in default Job view.
    pub is_ignored_noise: bool,
    /// True for `conhost.exe` or `cmd /c` wrappers that collapse into a child/parent job.
    pub is_wrapper_or_helper: bool,
    pub is_critical_system: bool,
    pub restart_recipe: Option<RestartRecipe>,
    pub auto_restart: bool,
}

#[derive(Debug, Clone)]
pub struct CachedRoot {
    pub project_name: String,
    pub root_path: PathBuf,
    pub git_branch: Option<String>,
    pub source: InferenceSource,
    cached_at: Instant,
}

#[derive(Debug, Clone)]
enum PathCacheEntry {
    Found(CachedRoot),
    Missing(Instant),
}

/// Caches last-line reads of `>> file.log` redirects (last 4 KB, max once per 5s when expanded).
pub struct LogTailCache {
    entries: HashMap<PathBuf, (Instant, Option<String>, Option<SystemTime>)>,
}

impl LogTailCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn read_last_line(&mut self, path: &Path, allow_io: bool) -> Option<String> {
        self.read_last_line_with_mtime(path, allow_io).0
    }

    pub fn read_last_line_with_mtime(
        &mut self,
        path: &Path,
        allow_io: bool,
    ) -> (Option<String>, Option<SystemTime>) {
        let now = Instant::now();
        if let Some((ts, cached, mtime)) = self.entries.get(path) {
            if !allow_io || now.saturating_duration_since(*ts) < Duration::from_secs(5) {
                return (cached.clone(), *mtime);
            }
        }
        if !allow_io {
            return (None, None);
        }
        let line = tail_last_4kb_line(path);
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        if !self.entries.contains_key(path) && self.entries.len() >= MAX_LOG_TAIL_CACHE_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (ts, _, _))| *ts)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries
            .insert(path.to_path_buf(), (now, line.clone(), mtime));
        (line, mtime)
    }
}


pub struct ProjectResolver {
    meta_cache: HashMap<ProcessKey, Arc<ProcessMetadata>>,
    ignored_keys: HashSet<ProcessKey>,
    path_cache: HashMap<PathBuf, PathCacheEntry>,
    docker_cache: Vec<DockerContainerSummary>,
    last_docker_poll: Option<Instant>,
    wsl_distros_cache: Vec<String>,
    last_wsl_poll: Option<Instant>,
    labels: LabelsStore,
    state_bindings: HashMap<u32, StateFileBinding>,
    last_state_poll: Option<Instant>,
    user_home: Option<PathBuf>,
    self_pid: u32,
    language: Language,
}

impl ProjectResolver {
    pub fn new(self_pid: u32) -> Self {
        let user_home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from);
        let labels = LabelsStore::new();
        let state_bindings: HashMap<u32, StateFileBinding> = labels
            .read_pid_bindings()
            .into_iter()
            .map(|b| (b.pid, b))
            .collect();
        Self {
            meta_cache: HashMap::with_capacity(64),
            ignored_keys: HashSet::with_capacity(384),
            path_cache: HashMap::with_capacity(128),
            docker_cache: Vec::new(),
            last_docker_poll: None,
            wsl_distros_cache: Vec::new(),
            last_wsl_poll: None,
            labels,
            state_bindings,
            last_state_poll: Some(Instant::now()),
            user_home,
            self_pid,
            language: Language::En,
        }
    }

    pub fn set_language(&mut self, lang: Language) {
        if self.language != lang {
            self.language = lang;
            self.meta_cache.clear();
            self.ignored_keys.clear();
        }
    }

    #[cfg(test)]
    pub fn set_labels_for_test(&mut self, rules: Vec<LabelRule>) {
        self.labels = LabelsStore::from_rules_for_test(rules);
        self.state_bindings = self
            .labels
            .read_pid_bindings()
            .into_iter()
            .map(|b| (b.pid, b))
            .collect();
        self.meta_cache.clear();
        self.ignored_keys.clear();
    }

    #[cfg(test)]
    pub fn insert_test_meta(&mut self, key: ProcessKey, meta: ProcessMetadata) {
        self.meta_cache.insert(key, Arc::new(meta));
    }

    pub fn save_label_rule(&mut self, rule: LabelRule) -> Result<(), String> {
        let res = self.labels.save_rule(rule);
        self.meta_cache.clear();
        self.ignored_keys.clear();
        res
    }

    pub fn remove_label_rule(&mut self, match_substr: &str) -> Result<(), String> {
        let res = self.labels.remove_rule(match_substr);
        self.meta_cache.clear();
        self.ignored_keys.clear();
        res
    }

    pub fn invalidate_all(&mut self) {
        self.meta_cache.clear();
        self.ignored_keys.clear();
        self.path_cache.clear();
        self.labels.reload_if_changed(true);
    }

    pub fn cached_meta(&self, key: &ProcessKey) -> Option<&Arc<ProcessMetadata>> {
        self.meta_cache.get(key)
    }

    pub fn state_binding_for_pid(&self, pid: u32) -> Option<&StateFileBinding> {
        self.state_bindings.get(&pid)
    }

    pub fn labels(&self) -> &LabelsStore {
        &self.labels
    }

    pub fn resolve_tick(
        &mut self,
        procs: &[CollectedProcess],
        allow_docker_pipe: bool,
        has_listening_port: &impl Fn(u32) -> bool,
    ) {
        // Auto-detect edits to %APPDATA%\bbstats\labels.toml immediately.
        let labels_changed = self.labels.reload_if_changed(false);
        if labels_changed {
            self.meta_cache.clear();
            self.ignored_keys.clear();
        }

        // Refresh PID state-file bindings (e.g. `auto-state.json`) every ~2s or on label change.
        if labels_changed
            || self
                .last_state_poll
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            let next_bindings: HashMap<u32, StateFileBinding> = self
                .labels
                .read_pid_bindings()
                .into_iter()
                .map(|b| (b.pid, b))
                .collect();
            if next_bindings != self.state_bindings {
                for pid in next_bindings.keys() {
                    self.meta_cache.retain(|k, _| k.pid != *pid);
                    self.ignored_keys.retain(|k| k.pid != *pid);
                }
                self.state_bindings = next_bindings;
            }
            self.last_state_poll = Some(Instant::now());
        }

        let active_by_pid: HashMap<u32, &CollectedProcess> =
            procs.iter().map(|p| (p.key.pid, p)).collect();

        self.meta_cache
            .retain(|k, _| active_by_pid.get(&k.pid).is_some_and(|p| p.key == *k));
        self.ignored_keys
            .retain(|k| active_by_pid.get(&k.pid).is_some_and(|p| p.key == *k));

        let docker_running = procs
            .iter()
            .any(|p| is_docker_process(&p.image_name.to_ascii_lowercase()));
        #[cfg(not(target_os = "windows"))]
        let docker_running = docker_running || crate::engine::win_nt::docker_socket_exists();
        if allow_docker_pipe && docker_running {
            let should_poll = self
                .last_docker_poll
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
            if should_poll {
                self.docker_cache = query_docker_containers_pipe();
                self.last_docker_poll = Some(Instant::now());
            }
        } else if !docker_running {
            self.docker_cache.clear();
        }

        let wsl_running = procs.iter().any(|p| is_wsl_vm_name(&p.image_name));
        if wsl_running {
            let should_poll = self
                .last_wsl_poll
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(15));
            if should_poll {
                self.wsl_distros_cache = list_wsl_distros_from_registry();
                self.last_wsl_poll = Some(Instant::now());
            }
        }

        // Map parent_pid -> child processes for wrapper collapsing (cmd /c -> node, conhost -> parent).
        let mut children_by_pid: HashMap<u32, Vec<&CollectedProcess>> = HashMap::new();
        for p in procs {
            if p.parent_pid != 0 && p.parent_pid != p.key.pid {
                children_by_pid.entry(p.parent_pid).or_default().push(p);
            }
        }

        // Evict previously ignored processes if they open a listening port or spawn a real child.
        self.ignored_keys.retain(|k| {
            if has_listening_port(k.pid) {
                return false;
            }
            if active_by_pid
                .get(&k.pid)
                .is_some_and(|p| p.image_name.eq_ignore_ascii_case("cmd.exe"))
                && children_by_pid.get(&k.pid).is_some_and(|kids| {
                    kids.iter().any(|c| {
                        !c.image_name.eq_ignore_ascii_case("conhost.exe")
                            && !c.image_name.eq_ignore_ascii_case("openconsole.exe")
                    })
                })
            {
                return false;
            }
            true
        });

        #[cfg(not(target_os = "windows"))]
        let prefetched_pebs = {
            let pids: Vec<u32> = procs
                .iter()
                .filter(|p| {
                    !self.ignored_keys.contains(&p.key)
                        && !self.meta_cache.contains_key(&p.key)
                        && (self.state_bindings.contains_key(&p.key.pid)
                            || has_listening_port(p.key.pid)
                            || !is_fast_skip_noise(
                                &p.image_name.to_ascii_lowercase(),
                                p.key.pid,
                                self.self_pid,
                                p.session_id,
                            ))
                })
                .map(|p| p.key.pid)
                .collect();
            crate::engine::win_nt::prefetch_unix_pebs(&pids)
        };
        #[cfg(target_os = "windows")]
        let prefetched_pebs: HashMap<u32, PebInspection> = HashMap::new();

        // First pass: resolve every newly seen ProcessKey individually.
        let mut newly_added = Vec::new();
        for proc in procs {
            if self.ignored_keys.contains(&proc.key) || self.meta_cache.contains_key(&proc.key) {
                continue;
            }
            let parent_name = active_by_pid
                .get(&proc.parent_pid)
                .filter(|p| p.key.create_time_100ns <= proc.key.create_time_100ns)
                .map(|p| p.image_name.as_str());
            let meta = self.resolve_single_process(
                proc,
                has_listening_port(proc.key.pid),
                parent_name,
                prefetched_pebs.get(&proc.key.pid),
            );
            newly_added.push(proc.key);
            self.meta_cache.insert(proc.key, Arc::new(meta));
        }

        // Second pass A: collapse `cmd /c` wrappers into their live child job (and propagate `>> log_path`!).
        for key in &newly_added {
            let Some(cur_proc) = active_by_pid.get(&key.pid).copied() else {
                continue;
            };
            let Some(cur_meta) = self.meta_cache.get(key).cloned() else {
                continue;
            };
            if cur_meta.image_name.eq_ignore_ascii_case("cmd.exe") {
                let Some(children) = children_by_pid.get(&key.pid) else {
                    continue;
                };
                if let Some(target_child) = children.iter().find(|c| {
                    c.key.create_time_100ns >= cur_proc.key.create_time_100ns
                        && !c.image_name.eq_ignore_ascii_case("conhost.exe")
                        && !c.image_name.eq_ignore_ascii_case("openconsole.exe")
                }) {
                    if let Some(child_meta) = self.meta_cache.get(&target_child.key).cloned() {
                        if !child_meta.is_ignored_noise {
                            let mut updated_wrapper = (*cur_meta).clone();
                            updated_wrapper.project_id = child_meta.project_id.clone();
                            updated_wrapper.project_display_name =
                                child_meta.project_display_name.clone();
                            updated_wrapper.project_root = child_meta.project_root.clone();
                            updated_wrapper.git_branch = child_meta.git_branch.clone();
                            updated_wrapper.job_id = child_meta.job_id.clone();
                            updated_wrapper.job_title = child_meta.job_title.clone();
                            updated_wrapper.job_subtitle = child_meta.job_subtitle.clone();
                            updated_wrapper.job_icon = child_meta.job_icon.clone();
                            updated_wrapper.job_color = child_meta.job_color.clone();
                            updated_wrapper.is_ignored_noise = false;
                            updated_wrapper.is_wrapper_or_helper = true;
                            self.meta_cache.insert(*key, Arc::new(updated_wrapper));

                            // Propagate `>> logfile` from `cmd /c ... >> logfile` to child if child had none.
                            if child_meta.log_path.is_none() && cur_meta.log_path.is_some() {
                                let mut updated_child = (*child_meta).clone();
                                updated_child.log_path = cur_meta.log_path.clone();
                                self.meta_cache
                                    .insert(target_child.key, Arc::new(updated_child));
                            }
                        }
                    }
                }
            } else if cur_meta.log_path.is_none() && !cur_meta.is_ignored_noise {
                if let Some(parent_proc) = active_by_pid.get(&cur_proc.parent_pid) {
                    if parent_proc.key.create_time_100ns <= cur_proc.key.create_time_100ns
                        && parent_proc.image_name.eq_ignore_ascii_case("cmd.exe")
                    {
                        if let Some(parent_meta) = self.meta_cache.get(&parent_proc.key).cloned() {
                            if parent_meta.log_path.is_some() {
                                let mut updated_child = (*cur_meta).clone();
                                updated_child.log_path = parent_meta.log_path.clone();
                                self.meta_cache.insert(*key, Arc::new(updated_child));
                            }
                        }
                    }
                }
            }
        }

        // Second pass B: attach `conhost.exe` / helpers / children of state-bound or same-project parents to parent Job.
        for &key in &newly_added {
            let Some(cur) = self.meta_cache.get(&key).cloned() else {
                continue;
            };
            let is_conhost = cur.image_name.eq_ignore_ascii_case("conhost.exe")
                || cur.image_name.eq_ignore_ascii_case("openconsole.exe");
            let ancestor_is_state_bound = {
                let mut cur_pid = key.pid;
                let mut found = false;
                for _ in 0..4 {
                    let Some(p) = active_by_pid.get(&cur_pid) else {
                        break;
                    };
                    if p.parent_pid == 0 || p.parent_pid == cur_pid {
                        break;
                    }
                    if self.state_bindings.contains_key(&p.parent_pid) {
                        found = true;
                        break;
                    }
                    cur_pid = p.parent_pid;
                }
                found
            };
            let should_inherit = is_conhost
                || cur.is_wrapper_or_helper
                || ancestor_is_state_bound
                || (!cur.is_ignored_noise
                    && matches!(cur.inference_source, InferenceSource::FallbackExeName));

            if !should_inherit {
                continue;
            }
            if let Some(inherited) = self.find_inherited_project(
                key,
                is_conhost || cur.is_wrapper_or_helper || ancestor_is_state_bound,
                &active_by_pid,
            ) {
                let mut updated = (*cur).clone();
                updated.project_id = inherited.project_id;
                updated.project_display_name = inherited.project_display_name;
                updated.project_root = inherited.project_root;
                updated.git_branch = inherited.git_branch;
                updated.job_id = inherited.job_id;
                updated.job_title = inherited.job_title;
                updated.job_subtitle = inherited.job_subtitle;
                updated.job_icon = inherited.job_icon;
                updated.job_color = inherited.job_color;
                updated.log_path = updated.log_path.or(inherited.log_path);
                updated.inline_last_log = updated.inline_last_log.or(inherited.inline_last_log);
                updated.is_ignored_noise = inherited.is_ignored_noise;
                if ancestor_is_state_bound {
                    updated.is_wrapper_or_helper = true;
                }
                updated.inference_source = InferenceSource::ParentInherited;
                self.meta_cache.insert(key, Arc::new(updated));
            }
        }

        // Move ignored background/OS processes from bulky `meta_cache` into compact `ignored_keys`.
        for key in newly_added {
            if self.meta_cache.get(&key).is_some_and(|m| m.is_ignored_noise) {
                self.meta_cache.remove(&key);
                self.ignored_keys.insert(key);
            }
        }
    }

    pub fn docker_containers(&self) -> &[DockerContainerSummary] {
        &self.docker_cache
    }

    fn resolve_single_process(
        &mut self,
        proc: &CollectedProcess,
        has_port: bool,
        parent_name: Option<&str>,
        prefetched_peb: Option<&PebInspection>,
    ) -> ProcessMetadata {
        let lower_name = proc.image_name.to_ascii_lowercase();
        let is_critical = proc.key.pid <= 4
            || proc.key.pid == self.self_pid
            || (proc.session_id == 0 && is_hard_blacklisted_name(&lower_name))
            || is_hard_blacklisted_name(&lower_name);

        // State-file PID binding takes precedence even when PEB/WMI cmdline is completely unreadable.
        if let Some(binding) = self.state_bindings.get(&proc.key.pid).cloned() {
            let owned_peb;
            let peb = match prefetched_peb {
                Some(p) => p,
                None => {
                    owned_peb = inspect_process_peb(proc.key.pid);
                    &owned_peb
                }
            };
            return self.resolve_from_state_binding(proc, &binding, peb, is_critical);
        }

        // Self & immediate OS/driver/browser/IDE noise fast-skip (zero PEB read!),
        // unless user explicitly added a `watch = true` rule for this process name.
        let image_rule_watched = !is_critical
            && self
                .labels
                .match_rule(&[&proc.image_name])
                .is_some_and(|r| r.watch == Some(true));
        if !has_port
            && !image_rule_watched
            && is_fast_skip_noise(&lower_name, proc.key.pid, self.self_pid, proc.session_id)
        {
            return ProcessMetadata {
                image_name: proc.image_name.clone(),
                working_dir: None,
                sanitized_cmdline: None,
                access_limited: false,
                project_id: ProjectId("system:ignored".to_string()),
                project_display_name: "Windows System".to_string(),
                project_root: None,
                git_branch: None,
                job_id: JobId(format!("sys:{}", proc.key.pid)),
                job_title: proc.image_name.clone(),
                job_subtitle: String::new(),
                job_icon: None,
                job_color: None,
                match_hint: proc.image_name.clone(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::Unknown,
                role_label: proc.image_name.clone(),
                inference_source: InferenceSource::FallbackExeName,
                is_ignored_noise: true,
                is_wrapper_or_helper: matches!(lower_name.as_str(), "conhost.exe" | "openconsole.exe"),
                is_critical_system: is_critical,
                restart_recipe: None,
                auto_restart: false,
            };
        }

        // WSL2 / Docker VM special entry
        if is_wsl_vm_name(&lower_name) {
            let distros = if self.wsl_distros_cache.is_empty() {
                "WSL2 / Docker VM".to_string()
            } else {
                format!("WSL2 ({})", self.wsl_distros_cache.join(", "))
            };
            return ProcessMetadata {
                image_name: proc.image_name.clone(),
                working_dir: None,
                sanitized_cmdline: None,
                access_limited: true,
                project_id: ProjectId("wsl:vmmem".to_string()),
                project_display_name: "WSL2 / Docker".to_string(),
                project_root: None,
                git_branch: None,
                job_id: JobId("wsl:vmmem".to_string()),
                job_title: distros.clone(),
                job_subtitle: "Virtual Machine".to_string(),
                job_icon: None,
                job_color: None,
                match_hint: "vmmemWSL".to_string(),
                log_path: None,
                inline_last_log: None,
                custom_command: None,
                is_watched: false,
                role: ProcessRole::Container,
                role_label: distros,
                inference_source: InferenceSource::WslVm,
                is_ignored_noise: false,
                is_wrapper_or_helper: false,
                is_critical_system: true,
                restart_recipe: None,
                auto_restart: false,
            };
        }

        let peb_owned;
        let peb = match prefetched_peb {
            Some(p) => p,
            None => {
                peb_owned = inspect_process_peb(proc.key.pid);
                &peb_owned
            }
        };
        self.resolve_from_peb(proc, has_port, parent_name, peb)
    }

    /// Resolves a process bound via `labels.toml` `pid_file` even when PEB cmdline/cwd are unreadable.
    pub fn resolve_from_state_binding(
        &self,
        proc: &CollectedProcess,
        binding: &StateFileBinding,
        peb: &crate::engine::win_nt::PebInspection,
        is_critical: bool,
    ) -> ProcessMetadata {
        let project_name = binding
            .rule
            .project
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| dir_name_or(&binding.state_dir, "Automation"));
        let project_id = if binding.state_dir.as_os_str().is_empty() {
            ProjectId(format!("rule:{}", project_name.to_ascii_lowercase()))
        } else {
            ProjectId(normalize_project_id(&binding.state_dir))
        };
        let job_title = binding.rule.job.clone();
        let folder_short = dir_name_or(&binding.state_dir, &project_name);
        let next_hint = binding
            .next_run
            .as_deref()
            .map(|nr| format!(" · {} {nr}", self.language.pick("next", "sonraki")))
            .unwrap_or_default();
        let job_subtitle = format!("{project_name} › ...\\{folder_short}{next_hint}");
        let job_id = JobId(format!(
            "{}::state:{}",
            project_id.0,
            binding.rule.match_substr.to_ascii_lowercase()
        ));

        let log_path = binding.log_file.clone().or_else(|| {
            ["auto-console.log", "service.log"]
                .iter()
                .map(|f| binding.state_dir.join(f))
                .find(|p| p.is_file())
        });

        let restart_recipe = if let Some(ref cmd) = binding.rule.command {
            let cwd = binding
                .rule
                .working_dir
                .as_deref()
                .map(|w| PathBuf::from(crate::engine::labels::expand_win_env_vars(w)))
                .or_else(|| peb.working_dir.clone())
                .unwrap_or_else(|| binding.state_dir.clone());
            Some(RestartRecipe {
                raw_cmdline: cmd.clone(),
                working_dir: cwd,
            })
        } else {
            match (peb.cmdline.as_ref(), peb.working_dir.as_ref()) {
                (Some(cmd), Some(cwd)) if !is_critical => Some(RestartRecipe {
                    raw_cmdline: cmd.clone(),
                    working_dir: cwd.clone(),
                }),
                _ => None,
            }
        };

        ProcessMetadata {
            image_name: proc.image_name.clone(),
            working_dir: peb.working_dir.clone().or_else(|| Some(binding.state_dir.clone())),
            sanitized_cmdline: peb.cmdline.as_deref().map(sanitize_cmdline),
            access_limited: !peb.peb_readable,
            project_id,
            project_display_name: project_name,
            project_root: Some(binding.state_dir.clone()),
            git_branch: None,
            job_id,
            job_title,
            job_subtitle,
            job_icon: binding.rule.icon.clone(),
            job_color: binding.rule.color.clone(),
            match_hint: binding.rule.match_substr.clone(),
            log_path,
            inline_last_log: binding.last_log.clone(),
            custom_command: binding.rule.command.clone(),
            is_watched: binding.rule.watch.unwrap_or(true),
            role: ProcessRole::WorkerOrQueue,
            role_label: binding.rule.job.clone(),
            inference_source: InferenceSource::UserLabelRule,
            is_ignored_noise: false,
            is_wrapper_or_helper: false,
            is_critical_system: is_critical,
            restart_recipe,
            auto_restart: binding.rule.auto_restart == Some(true),
        }
    }

    /// Pure resolution from an already-read `PebInspection` (directly unit-testable with real cmdlines!).
    pub fn resolve_from_peb(
        &mut self,
        proc: &CollectedProcess,
        has_port: bool,
        parent_name: Option<&str>,
        peb: &crate::engine::win_nt::PebInspection,
    ) -> ProcessMetadata {
        #[cfg(not(target_os = "windows"))]
        let normalized_peb = crate::engine::win_nt::PebInspection {
            image_path: peb.image_path.as_deref().map(coerce_platform_path_ref),
            working_dir: peb.working_dir.as_deref().map(coerce_platform_path_ref),
            cmdline: peb.cmdline.clone(),
            peb_readable: peb.peb_readable,
            virtual_env: peb.virtual_env.clone(),
            compose_project_env: peb.compose_project_env.clone(),
        };
        #[cfg(not(target_os = "windows"))]
        let peb = &normalized_peb;

        let lower_name = proc.image_name.to_ascii_lowercase();
        let is_critical = proc.key.pid <= 4
            || proc.key.pid == self.self_pid
            || (proc.session_id == 0 && is_hard_blacklisted_name(&lower_name))
            || is_hard_blacklisted_name(&lower_name);

        if let Some(binding) = self.state_bindings.get(&proc.key.pid).cloned() {
            return self.resolve_from_state_binding(proc, &binding, peb, is_critical);
        }

        let access_limited = !peb.peb_readable && proc.key.pid > 4;
        let sanitized_cmdline = peb.cmdline.as_deref().map(sanitize_cmdline);

        // Extract `>> logfile.log` redirect if present in cmdline.
        let mut log_path = peb
            .cmdline
            .as_deref()
            .and_then(|c| extract_log_redirect_path(c, peb.working_dir.as_deref()));

        // Check IDE sidecar / Antigravity / interactive shell noise.
        let is_ide_sidecar = is_ide_or_sidecar_process(
            &lower_name,
            peb.cmdline.as_deref(),
            peb.image_path.as_deref(),
            peb.working_dir.as_deref(),
        );
        let is_interactive_shell = is_interactive_shell_process(&lower_name, peb.cmdline.as_deref());
        let is_cmd_wrapper = lower_name == "cmd.exe"
            && peb
                .cmdline
                .as_deref()
                .is_some_and(|c| c.to_ascii_lowercase().contains("/c"));

        let parsed = analyze_command_and_paths(
            &lower_name,
            peb.cmdline.as_deref(),
            peb.working_dir.as_deref(),
            peb.image_path.as_deref(),
            peb.virtual_env.as_deref(),
        );

        let has_scheduled_env = peb
            .cmdline
            .as_deref()
            .is_some_and(|c| c.contains("REASON_SCHEDULED_TASK"));
        let is_scheduled_task = has_scheduled_env
            || (parent_name.is_some_and(|p| {
                let pl = p.to_ascii_lowercase();
                pl == "svchost.exe" || pl == "taskeng.exe" || pl == "taskhostw.exe"
            }) && !is_ide_sidecar
                && parsed.script_path.is_some());

        // 1. User rule (`labels.toml`) — highest priority!
        // Strip shell output redirection (`>> file.log`) before rule matching so log destinations
        // in another folder never hijack the script's own project identity.
        let cwd_str = peb
            .working_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let img_str = peb
            .image_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let script_str = parsed
            .script_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cmd_str = peb.cmdline.as_deref().unwrap_or("");
        let cmd_no_redir = strip_shell_redirects(cmd_str);
        let rule_match = self
            .labels
            .match_rule(&[
                &script_str,
                &cwd_str,
                &cmd_no_redir,
                &img_str,
                &proc.image_name,
            ])
            .cloned();

        if log_path.is_none() {
            log_path = discover_script_log_path(&parsed, peb.cmdline.as_deref(), peb.working_dir.as_deref());
        }

        // 2. Manifest lookup (`package.json`, `pyproject.toml`, `Cargo.toml`, `README.md`, `.git`)
        let seed_for_manifest = parsed
            .project_seed_dir
            .as_deref()
            .or(peb.working_dir.as_deref());
        let manifest_root = seed_for_manifest.and_then(|d| self.lookup_manifest_root(d));

        // 3 & 4. Resolve final Project name & root
        let (mut project_id, mut project_name, project_root, git_branch, mut source) =
            if let Some(root) = manifest_root {
                let name = normalize_project_display_name(&root.project_name);
                (
                    ProjectId(normalize_project_id(&root.root_path)),
                    name,
                    Some(root.root_path),
                    root.git_branch,
                    root.source,
                )
            } else if let Some(compose) = peb.compose_project_env.as_ref() {
                (
                    ProjectId(format!("docker:{}", compose.to_ascii_lowercase())),
                    compose.clone(),
                    peb.working_dir.clone(),
                    None,
                    InferenceSource::DockerComposeLabel,
                )
            } else if let Some(hint_dir) = parsed.inferred_project_dir.as_ref() {
                let raw_name = dir_name_or(hint_dir, &proc.image_name);
                let clean = normalize_project_display_name(&raw_name);
                (
                    ProjectId(normalize_project_id(hint_dir)),
                    clean,
                    Some(hint_dir.clone()),
                    None,
                    InferenceSource::CommandLineHeuristic,
                )
            } else if let Some(explicit) = parsed.explicit_project_name.as_ref() {
                let root_dir = parsed
                    .project_seed_dir
                    .clone()
                    .or_else(|| peb.working_dir.clone());
                let id = root_dir
                    .as_ref()
                    .map(|p| ProjectId(normalize_project_id(p)))
                    .unwrap_or_else(|| ProjectId(format!("hint:{}", explicit.to_ascii_lowercase())));
                (
                    id,
                    normalize_project_display_name(explicit),
                    root_dir,
                    None,
                    InferenceSource::CommandLineHeuristic,
                )
            } else if let Some(cwd) = peb.working_dir.as_ref().filter(|d| !is_generic_dir(d)) {
                let folder = dir_name_or(cwd, &proc.image_name);
                (
                    ProjectId(normalize_project_id(cwd)),
                    normalize_project_display_name(&folder),
                    Some(cwd.clone()),
                    None,
                    InferenceSource::CommandLineHeuristic,
                )
            } else {
                let base_name = proc
                    .image_name
                    .strip_suffix(".exe")
                    .or_else(|| proc.image_name.strip_suffix(".EXE"))
                    .unwrap_or(&proc.image_name)
                    .to_string();
                (
                    ProjectId(format!("exe:{lower_name}")),
                    base_name,
                    None,
                    None,
                    InferenceSource::FallbackExeName,
                )
            };

        // 3. Script/module -> human-readable Job title (stripping project & root-folder prefix!)
        let root_folder_name = project_root
            .as_deref()
            .or(peb.working_dir.as_deref())
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let mut job_title = format_job_title(
            &parsed,
            &project_name,
            root_folder_name,
            &proc.image_name,
            project_root.as_deref().or(peb.working_dir.as_deref()),
            is_scheduled_task,
            self.language,
        );
        let mut job_icon = None;
        let mut job_color = None;

        // Apply Priority 1: `labels.toml` rule overrides project, job title, icon, color!
        // Match against resolved manifest `project_name` too (e.g. `dsh-cli-agent` -> `Cli Agent`).
        let rule_match = rule_match.or_else(|| {
            self.labels
                .match_rule(&[&project_name, root_folder_name])
                .cloned()
        });
        if let Some(rule) = rule_match.as_ref() {
            if let Some(p_override) = rule.project.as_ref().filter(|s| !s.trim().is_empty()) {
                project_name = p_override.trim().to_string();
                if project_root.is_none() {
                    project_id = ProjectId(format!("rule:{}", project_name.to_ascii_lowercase()));
                }
            }
            if !rule.job.trim().is_empty() {
                job_title = rule.job.trim().to_string();
            }
            job_icon = rule.icon.clone();
            job_color = rule.color.clone();
            if log_path.is_none() {
                if let Some(ref lf) = rule.log_file {
                    let expanded = PathBuf::from(expand_win_env_vars(lf));
                    log_path = Some(expanded);
                }
            }
            source = InferenceSource::UserLabelRule;
        }

        // Build clean subtitle: `<Project> › <relative script dir>`
        let rel_dir = build_subtitle_path(
            project_root.as_deref(),
            parsed.script_dir.as_deref().or(peb.working_dir.as_deref()),
        );
        let mut job_subtitle = match rel_dir {
            Some(rel) if !rel.is_empty() => format!("{project_name} › {rel}"),
            _ => project_name.clone(),
        };

        // Canonical JobId collapses `cmd /c`, `.venv\Scripts\pythonw.exe` trampoline, and child
        // `pythonw.exe` / `node.exe` running the same script/entrypoint into ONE Job!
        let canonical_entry = parsed
            .canonical_entrypoint
            .clone()
            .unwrap_or_else(|| format!("{lower_name}:{}", job_title.to_ascii_lowercase()));
        let job_id = JobId(format!("{}::{}", project_id.0, canonical_entry));

        let custom_command = rule_match.as_ref().and_then(|r| r.command.clone());
        let is_watched = rule_match.as_ref().is_some_and(|r| r.watch == Some(true));
        let match_hint = parsed
            .script_file_name
            .clone()
            .or_else(|| parsed.module_name.clone())
            .or_else(|| rule_match.as_ref().map(|r| r.match_substr.clone()))
            .unwrap_or_else(|| {
                if is_watched {
                    proc.image_name.clone()
                } else {
                    project_name.clone()
                }
            });

        // Visibility check per spec:
        // Show ONLY if: matched user rule OR listening port OR scheduled task OR inside user project dir,
        // and NEVER show IDE sidecars, interactive shells, or Windows/driver services by default.
        let in_user_workspace = is_user_project_location(
            parsed.script_dir.as_deref(),
            peb.working_dir.as_deref(),
            peb.image_path.as_deref(),
            self.user_home.as_deref(),
        );
        let script_slashed = script_str.replace('/', "\\").to_ascii_lowercase();
        let img_slashed = img_str.replace('/', "\\").to_ascii_lowercase();
        let is_site_packages_helper = script_slashed.contains("\\site-packages\\")
            || script_slashed.contains("\\playwright\\driver\\")
            || img_slashed.contains("\\playwright\\driver\\");
        let is_helper = is_helper_process(&lower_name, parsed.role) || is_site_packages_helper;
        let is_wrapper_or_helper = (is_cmd_wrapper || is_helper) && !is_watched;

        let is_periodic_watchdog = rule_match
            .as_ref()
            .is_some_and(|r| r.hide_transient == Some(true))
            || (parsed
                .script_file_name
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case("watchdog.py"))
                && parsed
                    .script_dir
                    .as_deref()
                    .or(peb.working_dir.as_deref())
                    .is_some_and(|d| d.join("watchdog-state.json").is_file()));

        let is_dev_runtime = is_known_dev_runtime(&lower_name);
        let is_system_app = if is_dev_runtime {
            if parsed.script_dir.is_some() {
                is_system_or_app_location(parsed.script_dir.as_deref(), None)
            } else {
                is_system_or_app_location(None, peb.working_dir.as_deref())
            }
        } else {
            is_system_or_app_location(peb.image_path.as_deref(), peb.working_dir.as_deref())
        };

        let is_unreadable_notable = peb.cmdline.is_none()
            && !is_ide_sidecar
            && !is_interactive_shell
            && !is_system_app
            && is_known_dev_runtime(&lower_name);

        // Generic fallback when cmdline/cwd are unreadable (for All Procs / port-listening elevated services).
        if is_unreadable_notable && rule_match.is_none() && project_root.is_none() {
            let short_exe = proc
                .image_name
                .strip_suffix(".exe")
                .or_else(|| proc.image_name.strip_suffix(".EXE"))
                .unwrap_or(&proc.image_name);
            let parent_label = parent_name.unwrap_or(self.language.pick("unknown", "bilinmiyor"));
            job_title = format!(
                "{short_exe} · {}",
                self.language
                    .pick("command line unavailable", "komut satırı okunamadı")
            );
            job_subtitle = format!(
                "{} · {}: {parent_label} · PID {}",
                self.language.pick("Access restricted", "Erişim kısıtlı"),
                self.language.pick("parent", "üst"),
                proc.key.pid
            );
        }

        // Whitelist-first visibility:
        // Hide critical OS, C:\Windows, Program Files, WindowsApps, AppData, IDE sidecars, shells,
        // and unreadable processes without a state-file binding or listening port;
        // show ONLY user-folder / labels-matched / state-bound / dev-port jobs.
        let is_ignored_noise = if is_periodic_watchdog || is_critical {
            true
        } else if rule_match.is_some() {
            false
        } else if is_system_app || is_ide_sidecar || is_interactive_shell || is_cmd_wrapper || is_helper {
            true
        } else if in_user_workspace
            || (has_port && (is_known_dev_runtime(&lower_name) || is_docker_process(&lower_name)))
        {
            false
        } else {
            !is_scheduled_task
        };

        let restart_recipe = if let Some(ref cmd) = custom_command {
            let cwd = rule_match
                .as_ref()
                .and_then(|r| r.working_dir.as_deref())
                .map(|w| PathBuf::from(expand_win_env_vars(w)))
                .or_else(|| peb.working_dir.clone())
                .or_else(|| project_root.clone())
                .unwrap_or_else(|| PathBuf::from("."));
            Some(RestartRecipe {
                raw_cmdline: cmd.clone(),
                working_dir: cwd,
            })
        } else {
            match (peb.cmdline.as_ref(), peb.working_dir.as_ref()) {
                (Some(cmd), Some(cwd)) if !is_critical && !is_wrapper_or_helper => Some(RestartRecipe {
                    raw_cmdline: cmd.clone(),
                    working_dir: cwd.clone(),
                }),
                _ => None,
            }
        };

        ProcessMetadata {
            image_name: proc.image_name.clone(),
            working_dir: peb.working_dir.clone(),
            sanitized_cmdline,
            access_limited,
            project_id,
            project_display_name: project_name,
            project_root,
            git_branch,
            job_id,
            job_title,
            job_subtitle,
            job_icon,
            job_color,
            match_hint,
            log_path,
            inline_last_log: None,
            custom_command,
            is_watched,
            role: parsed.role,
            role_label: parsed.role_label,
            inference_source: source,
            is_ignored_noise,
            is_wrapper_or_helper,
            is_critical_system: is_critical,
            restart_recipe,
            auto_restart: rule_match.as_ref().is_some_and(|r| r.auto_restart == Some(true)),
        }
    }

    fn find_inherited_project(
        &self,
        start_key: ProcessKey,
        is_wrapper_or_conhost: bool,
        active_by_pid: &HashMap<u32, &CollectedProcess>,
    ) -> Option<ProcessMetadata> {
        let mut cur_key = start_key;
        for _ in 0..5 {
            let cur_proc = active_by_pid.get(&cur_key.pid)?;
            if cur_proc.parent_pid == 0 || cur_proc.parent_pid == cur_key.pid {
                break;
            }
            let parent_proc = active_by_pid.get(&cur_proc.parent_pid)?;
            if parent_proc.key.create_time_100ns > cur_key.create_time_100ns {
                break;
            }
            let Some(parent_meta) = self.meta_cache.get(&parent_proc.key) else {
                cur_key = parent_proc.key;
                continue;
            };
            let parent_has_project = !matches!(
                parent_meta.inference_source,
                InferenceSource::FallbackExeName | InferenceSource::ParentInherited
            );
            if !parent_meta.is_ignored_noise && (parent_has_project || is_wrapper_or_conhost) {
                return Some((**parent_meta).clone());
            }
            cur_key = parent_proc.key;
        }
        None
    }

    fn lookup_manifest_root(&mut self, seed_dir: &Path) -> Option<CachedRoot> {
        let mut cur = if seed_dir.is_file() {
            seed_dir.parent()?.to_path_buf()
        } else {
            seed_dir.to_path_buf()
        };

        for _ in 0..MAX_WALK_LEVELS {
            if is_stop_directory(&cur, self.user_home.as_deref()) {
                break;
            }

            // Skip intermediate container/script subdirectories so we don't treat `packages` or `scripts` as the project!
            if is_skippable_intermediate_dir(&cur) {
                let Some(parent) = cur.parent() else {
                    break;
                };
                cur = parent.to_path_buf();
                continue;
            }

            match self.path_cache.get(&cur) {
                Some(PathCacheEntry::Found(entry)) if entry.cached_at.elapsed() < PATH_CACHE_TTL => {
                    return Some(entry.clone());
                }
                Some(PathCacheEntry::Missing(since))
                    if since.elapsed() < PATH_NEGATIVE_CACHE_TTL =>
                {
                    let Some(parent) = cur.parent() else {
                        break;
                    };
                    cur = parent.to_path_buf();
                    continue;
                }
                _ => {}
            }

            if self.path_cache.len() >= MAX_PATH_CACHE_ENTRIES && !self.path_cache.contains_key(&cur)
            {
                self.path_cache.retain(|_, v| match v {
                    PathCacheEntry::Found(e) => e.cached_at.elapsed() < PATH_CACHE_TTL,
                    PathCacheEntry::Missing(since) => since.elapsed() < PATH_NEGATIVE_CACHE_TTL,
                });
                if self.path_cache.len() >= MAX_PATH_CACHE_ENTRIES {
                    self.path_cache.clear();
                }
            }

            if let Some(found) = inspect_directory_manifests(&cur) {
                self.path_cache
                    .insert(cur.clone(), PathCacheEntry::Found(found.clone()));
                return Some(found);
            } else {
                self.path_cache
                    .insert(cur.clone(), PathCacheEntry::Missing(Instant::now()));
            }

            let Some(parent) = cur.parent() else {
                break;
            };
            cur = parent.to_path_buf();
        }
        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::win_nt::PebInspection;

    fn mk_collected(pid: u32, ppid: u32, exe: &str) -> CollectedProcess {
        CollectedProcess {
            key: ProcessKey::new(pid, 1000),
            parent_pid: ppid,
            session_id: 1,
            image_name: exe.to_string(),
            cpu_percent: 1.0,
            private_ram_bytes: 16 * 1024 * 1024,
            uptime_secs: 120,
        }
    }

    #[test]
    fn resolves_all_spec_table_examples_without_raw_exe_titles() {
        let mut resolver = ProjectResolver::new(999_999);
        resolver.user_home = Some(PathBuf::from(r"C:\MockUsers\Dev"));
        resolver.set_labels_for_test(vec![
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
                match_substr: r".omp\proxy-watchdog".to_string(),
                project: Some("ProxyRouter".to_string()),
                job: "Watchdog".to_string(),
                ..Default::default()
            },
            LabelRule {
                match_substr: "cli-agent".to_string(),
                project: Some("Cli Agent".to_string()),
                job: String::new(),
                ..Default::default()
            },
        ]);

        let cases = [
            (
                "pythonw.exe",
                r"C:\MockUsers\Dev\Desktop\x\.venv\Scripts\pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_webhook_poller.py",
                r"C:\MockUsers\Dev\Desktop\x",
                Some(r"C:\MockUsers\Dev\Desktop\x\.venv\Scripts\pythonw.exe"),
                "X",
                "Webhook poller",
            ),
            (
                "pythonw.exe",
                r"pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_horizon_discovery.py --sources github",
                r"C:\MockUsers\Dev\Desktop\x",
                None,
                "X",
                "Horizon discovery (github)",
            ),
            (
                "pythonw.exe",
                r"pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\x_scheduler.py",
                r"C:\MockUsers\Dev\Desktop\x",
                None,
                "X",
                "Scheduler",
            ),
            (
                "pythonw.exe",
                r"pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_fast_lane_loop.py",
                r"C:\MockUsers\Dev\Desktop\x",
                None,
                "X",
                "Fast lane loop",
            ),
            (
                "pythonw.exe",
                r"pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_activity_monitor.py",
                r"C:\MockUsers\Dev\Desktop\x",
                None,
                "X",
                "Activity monitor",
            ),
            (
                "pythonw.exe",
                r"pythonw.exe C:\MockUsers\Dev\Desktop\x\runtime\scripts\run_x_own_reply_tracker.py",
                r"C:\MockUsers\Dev\Desktop\x",
                None,
                "X",
                "Own reply tracker",
            ),
            (
                "node.exe",
                r"cmd /c node C:\MockUsers\Dev\asd\edge-proxy\.token-rotator.js --port 18911",
                r"C:\MockUsers\Dev\asd\edge-proxy",
                None,
                "edge-proxy",
                "Token rotator",
            ),
            (
                "node.exe",
                r"cmd /c node packages\standalone\cli.mjs --port 18900",
                r"C:\MockUsers\Dev\.cli-agent",
                None,
                "Cli Agent",
                "Standalone service",
            ),
            (
                "node.exe",
                r"node C:\MockUsers\Dev\.omp\proxy-watchdog.js",
                r"C:\MockUsers\Dev\.omp",
                None,
                "ProxyRouter",
                "Watchdog",
            ),
            (
                "pythonw.exe",
                r"pythonw C:\MockUsers\Dev\Desktop\new\studio\service.py",
                r"C:\MockUsers\Dev\Desktop\new\studio",
                None,
                "studio",
                "Service",
            ),
            (
                "pythonw.exe",
                r"pythonw C:\MockUsers\Dev\.omp\local\worker-bot\auto_updater.py",
                r"C:\MockUsers\Dev\.omp\local\worker-bot",
                None,
                "WorkerBot",
                "Auto update botu",
            ),
            (
                "pythonw.exe",
                r"pythonw autoheal.py",
                r"C:\MockUsers\Dev\.omp\local\worker-bot",
                None,
                "WorkerBot",
                "Auto heal",
            ),
        ];

        for (idx, (exe, cmd, cwd, img, exp_proj, exp_job)) in cases.iter().enumerate() {
            let proc = mk_collected(1000 + idx as u32, 1, exe);
            let peb = PebInspection {
                image_path: img.map(PathBuf::from),
                working_dir: Some(PathBuf::from(cwd)),
                cmdline: Some(cmd.to_string()),
                peb_readable: true,
                ..Default::default()
            };
            let meta = resolver.resolve_from_peb(&proc, false, None, &peb);
            assert!(!meta.is_ignored_noise, "case {idx} should be visible");
            assert_eq!(meta.project_display_name, *exp_proj, "case {idx} project mismatch");
            assert_eq!(meta.job_title, *exp_job, "case {idx} job title mismatch");
            assert!(!meta.job_title.ends_with(".exe"), "title must never end with .exe");
        }

        // Also check fallback without labels.toml AND verify periodic watchdog scripts are hidden.
        resolver.set_labels_for_test(Vec::new());
        let omp_proc = mk_collected(2001, 1, "node.exe");
        let omp_peb = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\.omp")),
            cmdline: Some(r"node C:\Users\Dev\.omp\proxy-watchdog.js".to_string()),
            peb_readable: true,
            ..Default::default()
        };
        let omp_meta = resolver.resolve_from_peb(&omp_proc, false, None, &omp_peb);
        assert_eq!(omp_meta.project_display_name, "omp");
        assert_eq!(omp_meta.job_title, "Proxy watchdog");

        let wd_dir = std::env::temp_dir().join(format!("bbstats_wd_noise_{}", std::process::id()));
        let _ = fs::remove_dir_all(&wd_dir);
        fs::create_dir_all(&wd_dir).unwrap();
        fs::write(wd_dir.join("watchdog-state.json"), r#"{"last_run":"19:14"}"#).unwrap();
        let wd_proc = mk_collected(2002, 1, "pythonw.exe");
        let wd_peb = PebInspection {
            working_dir: Some(wd_dir.clone()),
            cmdline: Some(format!(r#"pythonw "{}" --restart"#, wd_dir.join("watchdog.py").display())),
            peb_readable: true,
            ..Default::default()
        };
        let wd_meta = resolver.resolve_from_peb(&wd_proc, false, None, &wd_peb);
        assert!(wd_meta.is_ignored_noise, "periodic watchdog.py with state must not flicker as a job row");
        let _ = fs::remove_dir_all(&wd_dir);
    }

    #[test]
    fn binds_pid_from_state_file_even_when_cmdline_unreadable_and_handles_generic_fallback() {
        let base = std::env::temp_dir().join(format!("bbstats_pid_bind_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let state_file = base.join("auto-state.json");
        fs::write(
            &state_file,
            r#"{"pid": 34968, "running": true, "nextRun": "19:30", "logs": [{"text": "19:14 checked 12 series"}]}"#,
        )
        .unwrap();

        let mut resolver = ProjectResolver::new(999_999);
        resolver.set_labels_for_test(vec![LabelRule {
            match_substr: r"worker-bot\auto_updater".to_string(),
            project: Some("WorkerBot".to_string()),
            job: "Auto update botu".to_string(),
            icon: Some("🤖".to_string()),
            color: Some("#F5B73B".to_string()),
            pid_file: Some(state_file.to_string_lossy().into_owned()),
            pid_key: Some("pid".to_string()),
            ..Default::default()
        }]);

        // PID 34968 with completely empty PEB (unreadable cmdline/cwd/image_path).
        let proc = mk_collected(34968, 1, "python.exe");
        let empty_peb = PebInspection::default();
        let meta = resolver.resolve_from_peb(&proc, false, None, &empty_peb);
        assert!(!meta.is_ignored_noise);
        assert_eq!(meta.project_display_name, "WorkerBot");
        assert_eq!(meta.job_title, "Auto update botu");
        assert_eq!(meta.inline_last_log.as_deref(), Some("19:14 checked 12 series"));
        assert!(meta.job_subtitle.contains("next 19:30"));

        // Generic unreadable cmdline fallback on a port-listening notable runtime process.
        let mut other = mk_collected(44444, 555, "python.exe");
        other.private_ram_bytes = 48 * 1024 * 1024;
        let other_meta = resolver.resolve_from_peb(&other, true, Some("svchost.exe"), &empty_peb);
        assert!(!other_meta.is_ignored_noise);
        assert!(other_meta.job_title.contains("command line unavailable"));

        // Switching language updates resolved subtitles & fallback titles in Turkish too.
        resolver.set_language(Language::Tr);
        let meta_tr = resolver.resolve_from_peb(&proc, false, None, &empty_peb);
        assert!(meta_tr.job_subtitle.contains("sonraki 19:30"));
        let other_tr = resolver.resolve_from_peb(&other, true, Some("svchost.exe"), &empty_peb);
        assert!(other_tr.job_title.contains("komut satırı okunamadı"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn hides_interactive_shells_ide_sidecars_and_conhost() {
        let mut resolver = ProjectResolver::new(999_999);

        let pwsh = mk_collected(10, 1, "pwsh.exe");
        let pwsh_peb = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\Desktop\x")),
            cmdline: Some("pwsh.exe -NoExit".to_string()),
            peb_readable: true,
            ..Default::default()
        };
        assert!(resolver.resolve_from_peb(&pwsh, false, None, &pwsh_peb).is_ignored_noise);

        let ls = mk_collected(11, 1, "language_server_windows_x64.exe");
        let ls_peb = PebInspection {
            working_dir: Some(PathBuf::from(r"C:\Users\Dev\.antigravity")),
            cmdline: Some("language_server_windows_x64.exe".to_string()),
            peb_readable: true,
            ..Default::default()
        };
        assert!(resolver.resolve_from_peb(&ls, false, None, &ls_peb).is_ignored_noise);

        let conhost = mk_collected(12, 10, "conhost.exe");
        let con_peb = PebInspection::default();
        assert!(resolver.resolve_from_peb(&conhost, false, None, &con_peb).is_ignored_noise);
    }

    #[test]
    fn extracts_and_tails_log_redirect_last_4kb() {
        let base = std::env::temp_dir().join(format!("bbstats_log_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let log_file = base.join("rotator.log");
        fs::write(&log_file, "line 1\nline 2\n18:52 poll ok\n\n").unwrap();

        let extracted = extract_log_redirect_path(
            r"cmd /c node .token-rotator.js --port 18911 >> rotator.log 2>&1",
            Some(&base),
        );
        assert_eq!(extracted, Some(log_file.clone()));

        let mut tail_cache = LogTailCache::new();
        assert_eq!(tail_cache.read_last_line(&log_file, false), None);
        assert_eq!(
            tail_cache.read_last_line(&log_file, true).as_deref(),
            Some("18:52 poll ok")
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitize_cmdline_masks_secrets_and_urls() {
        let raw = r#"node server.js --token=abc12345 --api-key "sk-secret-99" -p mypass https://admin:s3cr3t@db.local:5432/app --port 3000"#;
        let clean = sanitize_cmdline(raw);
        assert!(!clean.contains("abc12345"));
        assert!(!clean.contains("sk-secret-99"));
        assert!(!clean.contains("mypass"));
        assert!(!clean.contains("s3cr3t"));
        assert!(clean.contains("--token=***"));
        assert!(clean.contains("--api-key ***"));
        assert!(clean.contains("https://admin:***@db.local:5432/app"));
        assert!(clean.contains("--port 3000"));
    }

    #[test]
    fn manifest_walk_resolves_package_cargo_pyproject_readme_and_git() {
        let base = std::env::temp_dir().join(format!("bbstats_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let sub = base.join("apps").join("web").join("src");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            base.join("package.json"),
            r#"{"name":"@acme/monorepo","workspaces":["apps/*"]}"#,
        )
        .unwrap();
        fs::write(
            base.join("apps").join("web").join("package.json"),
            r#"{"name":"@acme/web"}"#,
        )
        .unwrap();

        let mut resolver = ProjectResolver::new(999_999);
        let root = resolver
            .lookup_manifest_root(&sub)
            .expect("should find monorepo package.json");
        assert_eq!(root.source, InferenceSource::ManifestPackageJson);
        assert_eq!(root.project_name, "acme/monorepo");

        let readme_dir = base.join("x_folder");
        fs::create_dir_all(readme_dir.join("runtime").join("scripts")).unwrap();
        fs::write(readme_dir.join("README.md"), "# X\n\nAutomation suite\n").unwrap();
        let readme_root = resolver
            .lookup_manifest_root(&readme_dir.join("runtime").join("scripts"))
            .expect("should skip runtime/scripts and find README # X");
        assert_eq!(readme_root.source, InferenceSource::ManifestReadme);
        assert_eq!(readme_root.project_name, "X");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn stores_ignored_processes_in_compact_hashset_instead_of_meta_cache() {
        let mut resolver = ProjectResolver::new(999_999);
        let svchost = mk_collected(101, 4, "svchost.exe");
        let svchost_key = svchost.key;
        let dwm = mk_collected(202, 4, "dwm.exe");
        let dwm_key = dwm.key;

        resolver.resolve_tick(&[svchost, dwm], false, &|_| false);

        assert!(resolver.ignored_keys.contains(&svchost_key));
        assert!(resolver.ignored_keys.contains(&dwm_key));
        assert!(resolver.cached_meta(&svchost_key).is_none());
        assert!(resolver.cached_meta(&dwm_key).is_none());

        // Evict `dwm` when it disappears from active processes.
        let svchost2 = mk_collected(101, 4, "svchost.exe");
        resolver.resolve_tick(&[svchost2], false, &|_| false);
        assert!(resolver.ignored_keys.contains(&svchost_key));
        assert!(!resolver.ignored_keys.contains(&dwm_key));
    }

    #[test]
    fn discovers_watch_and_supervised_logs_without_redirect() {
        let base = std::env::temp_dir().join(format!("bbstats_disc_log_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let scripts = base.join("runtime").join("scripts");
        let logs = base.join("runtime").join("logs");
        fs::create_dir_all(&scripts).unwrap();
        fs::create_dir_all(&logs).unwrap();
        let sup_log = logs.join("fast_lane_loop_supervised.log");
        fs::write(&sup_log, "{\"ok\":true}\n").unwrap();

        let watch_dir = base.join("new2");
        fs::create_dir_all(&watch_dir).unwrap();
        let watch_log = watch_dir.join("watch.log");
        fs::write(&watch_log, "[TG] listening\n").unwrap();

        let mut resolver = ProjectResolver::new(999_999);
        resolver.user_home = Some(base.clone());

        let p_x = mk_collected(301, 1, "pythonw.exe");
        let peb_x = PebInspection {
            working_dir: Some(base.clone()),
            cmdline: Some(format!(
                "pythonw.exe {}",
                scripts.join("run_x_fast_lane_loop.py").display()
            )),
            peb_readable: true,
            ..Default::default()
        };
        let meta_x = resolver.resolve_from_peb(&p_x, false, None, &peb_x);
        assert_eq!(meta_x.log_path.as_deref(), Some(sup_log.as_path()));

        let p_w = mk_collected(302, 1, "pythonw.exe");
        let peb_w = PebInspection {
            working_dir: Some(watch_dir.clone()),
            cmdline: Some(format!(
                "pythonw.exe -u {} watch",
                watch_dir.join("main.py").display()
            )),
            peb_readable: true,
            ..Default::default()
        };
        let meta_w = resolver.resolve_from_peb(&p_w, false, None, &peb_w);
        assert_eq!(meta_w.log_path.as_deref(), Some(watch_log.as_path()));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn evicts_ignored_key_when_listening_port_opens_later() {
        let mut resolver = ProjectResolver::new(999_999);
        let node = mk_collected(4444, 4, "node.exe");
        let key = node.key;

        // Tick 1: node has no listening port and no readable user cwd -> ignored.
        resolver.ignored_keys.insert(key);
        // Tick 2: node starts listening -> evicted from ignored_keys and resolved.
        resolver.resolve_tick(&[node], false, &|pid| pid == 4444);

        assert!(!resolver.ignored_keys.contains(&key));
        assert!(resolver.cached_meta(&key).is_some_and(|m| !m.is_ignored_noise));
    }
}
