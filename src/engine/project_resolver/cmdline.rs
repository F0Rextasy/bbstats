use std::path::{Path, PathBuf};

use super::filters::{
    is_docker_process, is_generic_container_dir, is_generic_dir, is_ide_or_shell_name,
    is_user_home_dir, normalize_path_str, path_str_starts_with_dir,
};
use super::manifests::dir_name_or;
use crate::model::{Language, ProcessRole};

#[derive(Debug, Clone)]
pub struct ParsedCommand {
    pub role: ProcessRole,
    pub role_label: String,
    pub script_path: Option<PathBuf>,
    pub script_dir: Option<PathBuf>,
    pub script_file_name: Option<String>,
    pub module_name: Option<String>,
    pub mode_suffix: Option<String>,
    pub project_seed_dir: Option<PathBuf>,
    pub inferred_project_dir: Option<PathBuf>,
    pub explicit_project_name: Option<String>,
    pub canonical_entrypoint: Option<String>,
}

/// Unwraps `cmd /c` / `python` / `node` / `java` / `dotnet` command lines and extracts
/// script paths, `-m` module names, `--sources` mode suffixes, and project root candidates.
pub fn analyze_command_and_paths(
    lower_exe: &str,
    cmdline: Option<&str>,
    cwd: Option<&Path>,
    image_path: Option<&Path>,
    virtual_env: Option<&str>,
) -> ParsedCommand {
    let raw_args = cmdline.map(split_cmdline_args).unwrap_or_default();
    let unwrapped = unwrap_shell_wrapper(&raw_args);
    let args = &unwrapped.args;
    let effective_exe = unwrapped
        .inner_exe
        .as_deref()
        .unwrap_or(lower_exe)
        .to_ascii_lowercase();
    let lower_cmd = cmdline.unwrap_or("").to_ascii_lowercase();

    let mode_suffix = extract_mode_flag(args);

    // Extract script path (`.py`, `.js`, `.mjs`, `.cjs`, `.ts`, `.ps1`, `.bat`, `.cmd`, etc.)
    let script_path = find_script_path(args, cwd);
    let script_file_name = script_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let script_dir = script_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);

    let exe_base = effective_exe.strip_suffix(".exe").unwrap_or(&effective_exe);

    // Python `-m module.name`
    let module_name = if exe_base.starts_with("python") || exe_base == "uvicorn" {
        extract_python_module(args)
    } else {
        None
    };

    // Determine the best directory to start manifest walk + fallback project folder inference.
    let venv_parent = image_path
        .and_then(strip_venv_parent)
        .or_else(|| virtual_env.map(PathBuf::from).and_then(|p| strip_venv_parent(&p)));

    let node_modules_parent = script_path
        .as_deref()
        .and_then(strip_node_modules_parent);

    let project_seed_dir = venv_parent
        .clone()
        .or(node_modules_parent)
        .or_else(|| script_dir.clone())
        .or_else(|| cwd.map(Path::to_path_buf));

    let inferred_project_dir = infer_project_dir_from_paths(
        venv_parent.as_deref(),
        script_path.as_deref(),
        cwd,
    );

    let canonical_entrypoint = script_path
        .as_ref()
        .map(|p| format!("script:{}", normalize_path_str(p)))
        .or_else(|| {
            module_name
                .as_ref()
                .map(|m| format!("mod:{}", m.to_ascii_lowercase()))
        });

    // Determine role & role_label
    let has_arg_token = |needle: &str| {
        args.iter().any(|a| {
            let low = a.to_ascii_lowercase();
            let leaf = low.rsplit(['/', '\\']).next().unwrap_or(&low);
            let stem = leaf
                .strip_suffix(".js")
                .or_else(|| leaf.strip_suffix(".mjs"))
                .or_else(|| leaf.strip_suffix(".cjs"))
                .or_else(|| leaf.strip_suffix(".ts"))
                .or_else(|| leaf.strip_suffix(".cmd"))
                .unwrap_or(leaf);
            low == needle || stem == needle
        })
    };
    let script_or_mod_has_word = |needle: &str| {
        let check = |s: &str| {
            s.to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == needle)
        };
        script_file_name.as_deref().is_some_and(check)
            || module_name.as_deref().is_some_and(check)
    };

    if matches!(exe_base, "node" | "bun" | "deno") {
        let is_next = has_arg_token("next")
            || lower_cmd.contains("next/dist/bin/next")
            || lower_cmd.contains("next\\dist\\bin\\next");
        let is_vite = has_arg_token("vite")
            || lower_cmd.contains("vite/bin/vite")
            || lower_cmd.contains("vite\\bin\\vite");
        let (role, label) = if is_next && (has_arg_token("dev") || has_arg_token("start")) {
            (ProcessRole::DevServer, "Next.js Dev".to_string())
        } else if is_vite {
            (ProcessRole::DevServer, "Vite Dev".to_string())
        } else if script_or_mod_has_word("rotator") {
            (ProcessRole::ApiBackend, "Node Service".to_string())
        } else if script_or_mod_has_word("watchdog") {
            (ProcessRole::WorkerOrQueue, "Watchdog".to_string())
        } else {
            (ProcessRole::ApiBackend, "Node Service".to_string())
        };
        return ParsedCommand {
            role,
            role_label: label,
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir,
            inferred_project_dir,
            explicit_project_name: None,
            canonical_entrypoint,
        };
    }

    if exe_base.starts_with("python") || exe_base == "uvicorn" {
        let (role, label) = if exe_base == "uvicorn"
            || has_arg_token("uvicorn")
            || has_arg_token("fastapi")
            || script_or_mod_has_word("uvicorn")
            || script_or_mod_has_word("fastapi")
        {
            (ProcessRole::ApiBackend, "Python API".to_string())
        } else if script_or_mod_has_word("poller") || script_or_mod_has_word("bot") {
            (ProcessRole::WorkerOrQueue, "Python Bot".to_string())
        } else {
            (ProcessRole::WorkerOrQueue, "Python Worker".to_string())
        };
        let explicit = if inferred_project_dir.is_none() {
            module_name
                .as_deref()
                .and_then(|m| m.split('.').next())
                .map(|s| s.to_string())
        } else {
            None
        };
        return ParsedCommand {
            role,
            role_label: label,
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir,
            inferred_project_dir,
            explicit_project_name: explicit,
            canonical_entrypoint,
        };
    }

    if matches!(exe_base, "java" | "javaw") {
        let spring_name = args
            .iter()
            .find_map(|a| a.strip_prefix("-Dspring.application.name=").map(str::to_string));
        let jar_seed = args
            .windows(2)
            .find(|w| w[0] == "-jar")
            .and_then(|w| {
                let resolved = resolve_relative_path(&w[1], cwd);
                strip_build_output_dir(&resolved)
            });
        return ParsedCommand {
            role: ProcessRole::ApiBackend,
            role_label: "Spring / JVM".to_string(),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir: jar_seed.or(project_seed_dir),
            inferred_project_dir,
            explicit_project_name: spring_name,
            canonical_entrypoint,
        };
    }

    if exe_base == "dotnet" {
        let project_seed = args
            .windows(2)
            .find(|w| w[0] == "--project" || w[0] == "-p")
            .map(|w| resolve_relative_path(&w[1], cwd));
        return ParsedCommand {
            role: if lower_cmd.contains("watch") {
                ProcessRole::DevServer
            } else {
                ProcessRole::ApiBackend
            },
            role_label: ".NET Service".to_string(),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir: project_seed.or(project_seed_dir),
            inferred_project_dir,
            explicit_project_name: None,
            canonical_entrypoint,
        };
    }

    if matches!(
        exe_base,
        "postgres" | "mysqld" | "redis-server" | "mongod" | "surreal"
    ) {
        let label = exe_base.to_string();
        return ParsedCommand {
            role: ProcessRole::DatabaseOrCache,
            role_label: format!("Database ({label})"),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir,
            inferred_project_dir,
            explicit_project_name: None,
            canonical_entrypoint,
        };
    }

    if is_docker_process(&effective_exe) {
        return ParsedCommand {
            role: ProcessRole::Container,
            role_label: "Docker Engine".to_string(),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir,
            inferred_project_dir,
            explicit_project_name: Some("docker-engine".to_string()),
            canonical_entrypoint,
        };
    }

    if is_ide_or_shell_name(&effective_exe) {
        return ParsedCommand {
            role: ProcessRole::TerminalShell,
            role_label: effective_exe.trim_end_matches(".exe").to_string(),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir,
            inferred_project_dir,
            explicit_project_name: None,
            canonical_entrypoint,
        };
    }

    if let Some(stripped) = image_path.and_then(strip_build_output_dir) {
        return ParsedCommand {
            role: ProcessRole::ApiBackend,
            role_label: format!("{} (Binary)", effective_exe.trim_end_matches(".exe")),
            script_path,
            script_dir,
            script_file_name,
            module_name,
            mode_suffix,
            project_seed_dir: Some(stripped.clone()),
            inferred_project_dir: Some(stripped),
            explicit_project_name: None,
            canonical_entrypoint,
        };
    }

    ParsedCommand {
        role: ProcessRole::Unknown,
        role_label: effective_exe.trim_end_matches(".exe").to_string(),
        script_path,
        script_dir,
        script_file_name,
        module_name,
        mode_suffix,
        project_seed_dir,
        inferred_project_dir,
        explicit_project_name: None,
        canonical_entrypoint,
    }
}

pub(super) struct UnwrappedCmd {
    inner_exe: Option<String>,
    args: Vec<String>,
}

pub(super) fn unwrap_shell_wrapper(args: &[String]) -> UnwrappedCmd {
    if args.is_empty() {
        return UnwrappedCmd {
            inner_exe: None,
            args: Vec::new(),
        };
    }
    let exe0 = args[0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    let shell_flag_pos = if exe0 == "cmd.exe" || exe0 == "cmd" {
        args.iter()
            .position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))
    } else if matches!(exe0.as_str(), "sh" | "bash" | "zsh" | "dash") {
        args.iter().position(|a| {
            a == "-c"
                || (a.starts_with('-')
                    && !a.starts_with("--")
                    && a.len() >= 2
                    && a.ends_with('c'))
        })
    } else {
        None
    };

    if let Some(c_idx) = shell_flag_pos {
        let rest = &args[c_idx + 1..];
        if !rest.is_empty() {
            // If `cmd /c "node foo.js --port 18911"` or `sh -c "node foo.js"`, split the quoted string too.
            let expanded = if rest.len() == 1 && rest[0].contains(' ') {
                split_cmdline_args(&rest[0])
            } else {
                rest.to_vec()
            };
            // Skip leading `ENV=VAL`, `env`, `exec` tokens when resolving the inner executable.
            let mut skip = 0usize;
            while let Some(tok) = expanded.get(skip) {
                let low = tok.to_ascii_lowercase();
                if low == "exec" || low == "env" || (tok.contains('=') && !tok.starts_with('-')) {
                    skip += 1;
                } else {
                    break;
                }
            }
            let effective_args = if skip < expanded.len() {
                expanded[skip..].to_vec()
            } else {
                expanded
            };
            let inner_exe = effective_args.first().and_then(|a| {
                a.rsplit(['/', '\\']).next().map(|s| {
                    let low = s.to_ascii_lowercase();
                    if low.ends_with(".exe") {
                        low
                    } else {
                        format!("{low}.exe")
                    }
                })
            });
            return UnwrappedCmd {
                inner_exe,
                args: effective_args,
            };
        }
    }

    UnwrappedCmd {
        inner_exe: None,
        args: args.to_vec(),
    }
}

/// Converts script filename / module name / generic entrypoint into a clean human Job title:
/// - `run_x_webhook_poller.py` (project `X`) -> `"Webhook poller"`
/// - `run_x_horizon_discovery.py --sources github` -> `"Horizon discovery (github)"`
/// - `x_scheduler.py` -> `"Scheduler"`
/// - `.token-rotator.js` -> `"Token rotator"`
/// - `packages\standalone\cli.mjs` -> `"Standalone service"`
/// - `proxy-watchdog.js` -> `"Proxy watchdog"`
/// - `studio\service.py` -> `"Service"`
/// - Never returns raw `node.exe` / `pythonw.exe` / `cmd.exe`; last resort is `"node · <folder>"`.
pub fn format_job_title(
    parsed: &ParsedCommand,
    project_name: &str,
    folder_name: &str,
    exe_name: &str,
    fallback_dir: Option<&Path>,
    is_scheduled_task: bool,
    lang: Language,
) -> String {
    let base_title = if let Some(script_path) = &parsed.script_path {
        let stem = script_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let clean_stem = stem.trim_start_matches('.');
        if is_generic_entrypoint_name(clean_stem) {
            if let Some(parent_name) = script_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .filter(|p| !is_skippable_dir_name(p) && !p.eq_ignore_ascii_case(project_name))
            {
                let norm_parent = humanize_identifier(parent_name, "", "");
                format!("{norm_parent} {}", lang.pick("service", "servisi"))
            } else {
                humanize_identifier(stem, project_name, folder_name)
            }
        } else {
            humanize_identifier(stem, project_name, folder_name)
        }
    } else if let Some(mod_name) = &parsed.module_name {
        let last_seg = mod_name.rsplit('.').next().unwrap_or(mod_name);
        humanize_identifier(last_seg, project_name, folder_name)
    } else if is_scheduled_task {
        lang.pick("Scheduled task", "Zamanlanmış görev").to_string()
    } else {
        let short_exe = exe_name
            .strip_suffix(".exe")
            .or_else(|| exe_name.strip_suffix(".EXE"))
            .unwrap_or(exe_name);
        if let Some(dir) = fallback_dir {
            let folder = dir_name_or(dir, project_name);
            if short_exe.eq_ignore_ascii_case(&folder) {
                humanize_identifier(short_exe, "", "")
            } else {
                format!("{short_exe} · {folder}")
            }
        } else if short_exe.eq_ignore_ascii_case(project_name) {
            humanize_identifier(short_exe, "", "")
        } else {
            format!("{short_exe} · {project_name}")
        }
    };

    match &parsed.mode_suffix {
        Some(mode) if !mode.is_empty() => format!("{base_title} ({mode})"),
        _ => base_title,
    }
}

pub(super) fn humanize_identifier(raw: &str, project_name: &str, folder_name: &str) -> String {
    let mut s = raw.trim().trim_start_matches('.');

    // Strip `run_` or `run-` prefix (e.g. `run_x_webhook_poller` -> `x_webhook_poller`).
    if let Some(rest) = s.strip_prefix("run_").or_else(|| s.strip_prefix("run-")) {
        s = rest;
    }

    // Strip leading `<project>_` or `<folder>_` prefix case-insensitively (e.g. `x_scheduler` -> `scheduler`).
    for candidate in [project_name, folder_name] {
        let clean = candidate.trim_start_matches('.').to_ascii_lowercase();
        if !clean.is_empty() {
            let low = s.to_ascii_lowercase();
            let mut stripped = false;
            for sep in ['_', '-'] {
                let prefix = format!("{clean}{sep}");
                if low.starts_with(&prefix) && s.len() > prefix.len() {
                    s = &s[prefix.len()..];
                    stripped = true;
                    break;
                }
            }
            if stripped {
                break;
            }
        }
    }

    let spaced = s.replace(['_', '-'], " ");
    let collapsed: Vec<&str> = spaced.split_whitespace().collect();
    if collapsed.is_empty() {
        return raw.to_string();
    }
    let joined = collapsed.join(" ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => raw.to_string(),
    }
}

pub(super) fn is_generic_entrypoint_name(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "cli" | "index" | "main" | "__main__" | "app" | "server" | "entry" | "start"
    )
}

pub(super) fn extract_mode_flag(args: &[String]) -> Option<String> {
    for win in args.windows(2) {
        if matches!(win[0].as_str(), "--sources" | "--source" | "--profile" | "--target")
            && !win[1].starts_with('-')
            && !win[1].starts_with('>')
        {
            return Some(win[1].clone());
        }
    }
    for arg in args {
        if let Some(val) = arg
            .strip_prefix("--sources=")
            .or_else(|| arg.strip_prefix("--source="))
            .or_else(|| arg.strip_prefix("--profile="))
            .or_else(|| arg.strip_prefix("--target="))
        {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

pub(super) fn extract_python_module(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == "-m")
        .map(|w| w[1].clone())
        .filter(|m| !m.is_empty() && !m.starts_with('-'))
}

pub(super) fn find_script_path(args: &[String], cwd: Option<&Path>) -> Option<PathBuf> {
    const SCRIPT_EXTS: &[&str] = &[
        ".py", ".pyw", ".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".ps1", ".bat", ".cmd",
    ];
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') || arg.starts_with('>') || arg == "2>&1" {
            continue;
        }
        let lower = arg.to_ascii_lowercase();
        if SCRIPT_EXTS.iter().any(|ext| lower.ends_with(ext)) {
            return Some(resolve_relative_path(arg, cwd));
        }
    }
    None
}

/// Probes well-known script-adjacent and `logs/` / `outputs/` log files for scripts
/// launched without explicit shell `>>` redirection.
pub(super) fn discover_script_log_path(
    parsed: &ParsedCommand,
    cmdline: Option<&str>,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    let sdir = parsed.script_dir.as_deref();
    if let Some(dir) = sdir {
        for fname in ["auto-console.log", "service.log", "rotator.log"] {
            let cand = dir.join(fname);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    let stem = parsed
        .script_path
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(|s| s.trim_start_matches('.'))
        .filter(|s| !s.is_empty());

    if let (Some(dir), Some(st)) = (sdir, stem) {
        let same_dir = dir.join(format!("{st}.log"));
        if same_dir.is_file() {
            return Some(same_dir);
        }
        if cmdline.is_some_and(|c| c.split_whitespace().any(|t| t == "watch" || t == "--watch")) {
            let watch_log = dir.join("watch.log");
            if watch_log.is_file() {
                return Some(watch_log);
            }
        }
    }

    let mut log_dirs: Vec<PathBuf> = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !log_dirs.contains(&p) {
            log_dirs.push(p);
        }
    };
    if let Some(dir) = sdir {
        push_unique(dir.join("logs"));
        if let Some(parent) = dir.parent() {
            push_unique(parent.join("logs"));
            push_unique(parent.join("outputs"));
        }
    }
    if let Some(seed) = parsed.project_seed_dir.as_deref().or(cwd) {
        push_unique(seed.join("logs"));
        push_unique(seed.join("outputs"));
    }

    if let Some(st) = stem {
        let st_low = st.to_ascii_lowercase();
        let mut names = vec![format!("{st_low}.log"), format!("{st_low}_supervised.log")];
        if let Some(no_run) = st_low
            .strip_prefix("run_")
            .or_else(|| st_low.strip_prefix("run-"))
        {
            names.push(format!("{no_run}.log"));
            names.push(format!("{no_run}_supervised.log"));
            if let Some((_, rest)) = no_run.split_once('_') {
                if !rest.is_empty() {
                    names.push(format!("{rest}.log"));
                    names.push(format!("{rest}_supervised.log"));
                }
            }
        }
        if matches!(st_low.as_str(), "service" | "cli" | "main") {
            names.push("service.log".to_string());
        }
        for ldir in &log_dirs {
            for fname in &names {
                let cand = ldir.join(fname);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }

    None
}

/// Extracts a log file path from `>> file.log`, `> file.log`, `1>>`, or `1>` redirections in a command line.
pub fn extract_log_redirect_path(cmdline: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let tokens = split_cmdline_args(cmdline);
    let mut i = 0usize;
    while i < tokens.len() {
        let tok = &tokens[i];
        let candidate = if matches!(tok.as_str(), ">>" | "1>>" | ">" | "1>") {
            tokens.get(i + 1).map(String::as_str)
        } else if tok.starts_with("2>") {
            None
        } else {
            tok.strip_prefix(">>")
                .or_else(|| tok.strip_prefix("1>>"))
                .or_else(|| tok.strip_prefix("1>"))
                .or_else(|| tok.strip_prefix('>'))
                .filter(|rest| !rest.is_empty())
        };

        if let Some(raw) = candidate {
            let trimmed = raw.trim_matches(['"', '\'']);
            if !trimmed.is_empty()
                && !trimmed.eq_ignore_ascii_case("nul")
                && !trimmed.eq_ignore_ascii_case("\\\\.\\nul")
                && trimmed != "/dev/null"
                && !trimmed.starts_with('&')
            {
                return Some(resolve_relative_path(trimmed, cwd));
            }
        }
        i += 1;
    }
    None
}

/// Infers the project directory from `.venv` parent, script path, or CWD,
/// skipping intermediate folders like `.venv`, `node_modules`, `Scripts`, `runtime`, `scripts`, `src`, `packages`.
pub(super) fn infer_project_dir_from_paths(
    venv_parent: Option<&Path>,
    script_path: Option<&Path>,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(vp) = venv_parent {
        if !is_generic_dir(vp) {
            return Some(vp.to_path_buf());
        }
    }

    if let Some(sp) = script_path {
        if let Some(nm) = strip_node_modules_parent(sp) {
            return Some(nm);
        }
        let mut cur = sp.parent();
        // If `packages\standalone\cli.mjs` with cwd `.cli-agent`, prefer cwd if cwd is an ancestor `.dot`/project dir.
        if let Some(cwd_dir) = cwd.filter(|d| !is_generic_dir(d)) {
            let cwd_norm = normalize_path_str(cwd_dir);
            let sp_norm = normalize_path_str(sp);
            if path_str_starts_with_dir(&sp_norm, &cwd_norm)
                && !is_user_home_dir(cwd_dir)
                && !is_generic_container_dir(cwd_dir)
            {
                return Some(cwd_dir.to_path_buf());
            }
        }

        while let Some(dir) = cur {
            if is_generic_dir(dir) || is_user_home_dir(dir) || is_generic_container_dir(dir) {
                break;
            }
            let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !is_skippable_dir_name(name) && !is_monorepo_subpackage_dir(dir) {
                return Some(dir.to_path_buf());
            }
            cur = dir.parent();
        }
    }

    cwd.filter(|d| !is_generic_dir(d) && !is_user_home_dir(d))
        .map(Path::to_path_buf)
}

pub(super) fn is_monorepo_subpackage_dir(dir: &Path) -> bool {
    dir.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .is_some_and(|n| matches!(n.to_ascii_lowercase().as_str(), "packages" | "apps"))
}

pub(super) fn is_skippable_intermediate_dir(dir: &Path) -> bool {
    let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    is_skippable_dir_name(name) || is_monorepo_subpackage_dir(dir)
}

pub(super) fn is_skippable_dir_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".venv"
            | "venv"
            | ".env"
            | "env"
            | "node_modules"
            | ".bin"
            | "scripts"
            | "runtime"
            | "src"
            | "dist"
            | "build"
            | "bin"
            | "packages"
            | "apps"
            | "lib"
            | "target"
    )
}

pub(super) fn normalize_project_display_name(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('.');
    if trimmed.len() == 1 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        trimmed.to_ascii_uppercase()
    } else if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn build_subtitle_path(project_root: Option<&Path>, script_or_cwd: Option<&Path>) -> Option<String> {
    let dir = script_or_cwd?;
    let sep = std::path::MAIN_SEPARATOR;
    let sep_s = sep.to_string();
    if let Some(root) = project_root {
        if let Ok(rel) = dir.strip_prefix(root) {
            let s = rel.to_string_lossy().replace(['/', '\\'], &sep_s);
            if !s.is_empty() {
                return Some(format!("...{sep}{s}"));
            }
            return None;
        }
        let dir_norm = normalize_path_str(dir);
        let root_norm = normalize_path_str(root);
        if dir_norm == root_norm {
            return None;
        }
        if path_str_starts_with_dir(&dir_norm, &root_norm) {
            let raw_dir = dir.to_string_lossy().replace('/', "\\");
            let trimmed_raw = raw_dir.trim_end_matches('\\');
            if trimmed_raw.len() > root_norm.len() + 1 {
                let rel = &trimmed_raw[root_norm.len() + 1..];
                let s = rel.replace('\\', &sep_s);
                if !s.is_empty() {
                    return Some(format!("...{sep}{s}"));
                }
            }
        }
    }
    let components: Vec<String> = dir
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if components.is_empty() {
        None
    } else {
        Some(format!("...{sep}{}", components.join(&sep_s)))
    }
}

fn split_cmdline_inner(s: &str, preserve_quotes: bool) -> Vec<String> {
    let mut res = Vec::new();
    let mut cur = String::new();
    let mut in_double = false;
    let mut in_single = false;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if in_double && chars.peek() == Some(&'"') => {
                let q = chars.next().unwrap();
                if preserve_quotes {
                    cur.push('\\');
                }
                cur.push(q);
            }
            '"' if !in_single => {
                in_double = !in_double;
                if preserve_quotes {
                    cur.push(ch);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                if preserve_quotes {
                    cur.push(ch);
                }
            }
            c if c.is_whitespace() && !in_double && !in_single => {
                if !cur.is_empty() {
                    res.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        res.push(cur);
    }
    res
}

pub(super) fn split_cmdline_preserving_quotes(s: &str) -> Vec<String> {
    split_cmdline_inner(s, true)
}

pub fn split_cmdline_args(s: &str) -> Vec<String> {
    split_cmdline_inner(s, false)
}

/// Converts Windows `\` separators to native `/` on non-Windows hosts so `Path` ops work uniformly.
pub(super) fn coerce_platform_path(raw: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(raw)
    }
    #[cfg(not(target_os = "windows"))]
    {
        if raw.contains('\\') {
            PathBuf::from(raw.replace('\\', "/"))
        } else {
            PathBuf::from(raw)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn coerce_platform_path_ref(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    coerce_platform_path(&s)
}

fn is_cross_platform_absolute(raw: &str, p: &Path) -> bool {
    if p.is_absolute() {
        return true;
    }
    let bytes = raw.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

pub(super) fn resolve_relative_path(raw: &str, cwd: Option<&Path>) -> PathBuf {
    let p = coerce_platform_path(raw);
    if is_cross_platform_absolute(raw, &p) {
        p
    } else if let Some(base) = cwd {
        coerce_platform_path(&base.to_string_lossy()).join(p)
    } else {
        p
    }
}

pub(super) fn strip_node_modules_parent(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.eq_ignore_ascii_case("node_modules") {
                return p
                    .parent()
                    .filter(|par| !par.as_os_str().is_empty())
                    .map(Path::to_path_buf);
            }
        }
        cur = p.parent();
    }
    None
}

pub(super) fn strip_venv_parent(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            let low = name.to_ascii_lowercase();
            if matches!(low.as_str(), ".venv" | "venv" | ".env" | "env") {
                return p.parent().map(Path::to_path_buf);
            }
        }
        cur = p.parent();
    }
    None
}

pub(super) fn strip_build_output_dir(path: &Path) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    let lower = s.replace('/', "\\").to_ascii_lowercase();
    for marker in [
        "\\target\\debug\\",
        "\\target\\release\\",
        "\\target\\debug",
        "\\target\\release",
        "\\bin\\debug\\",
        "\\bin\\release\\",
        "\\build\\libs\\",
        "\\target\\",
    ] {
        if let Some(idx) = lower.find(marker) {
            let root = &s[..idx];
            if !root.is_empty() {
                return Some(coerce_platform_path(root));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cmdline_handles_double_single_and_escaped_quotes() {
        assert_eq!(
            split_cmdline_args(r#"node "C:\My App\server.js" --port 3000"#),
            vec!["node", r"C:\My App\server.js", "--port", "3000"]
        );
        assert_eq!(
            split_cmdline_args(r#"sh -c 'node "/path/with spaces/app.js"'"#),
            vec!["sh", "-c", r#"node "/path/with spaces/app.js""#]
        );
        assert_eq!(
            split_cmdline_preserving_quotes("node app.js --api-key 'sk-secret part2'"),
            vec!["node", "app.js", "--api-key", "'sk-secret part2'"]
        );
        assert_eq!(
            split_cmdline_args(r#"app --msg "say \"hi\"""#),
            vec!["app", "--msg", r#"say "hi""#]
        );
        assert!(split_cmdline_args("   ").is_empty());
    }

    #[test]
    fn unwrap_shell_wrapper_handles_cmd_bash_lc_and_env_prefixes() {
        let u1 = unwrap_shell_wrapper(&[
            "cmd.exe".to_string(),
            "/c".to_string(),
            "node app.js --port 8080".to_string(),
        ]);
        assert_eq!(u1.inner_exe.as_deref(), Some("node.exe"));

        let u2 = unwrap_shell_wrapper(&[
            "bash".to_string(),
            "-lc".to_string(),
            "NODE_ENV=prod exec node server.js".to_string(),
        ]);
        assert_eq!(u2.inner_exe.as_deref(), Some("node.exe"));
        assert_eq!(u2.args[0], "node");

        let u3 = unwrap_shell_wrapper(&["python.exe".to_string(), "app.py".to_string()]);
        assert_eq!(u3.inner_exe, None);
    }

    #[test]
    fn analyze_command_avoids_path_substring_false_positives_and_resolves_jar() {
        let p_node = analyze_command_and_paths(
            "node.exe",
            Some(r"node C:\dev\invite-service\server.js"),
            Some(Path::new(r"C:\dev\invite-service")),
            None,
            None,
        );
        assert_eq!(p_node.role, ProcessRole::ApiBackend);
        assert_eq!(p_node.role_label, "Node Service");

        let p_vite = analyze_command_and_paths(
            "node.exe",
            Some(r"node C:\dev\web\node_modules\vite\bin\vite.js"),
            Some(Path::new(r"C:\dev\web")),
            None,
            None,
        );
        assert_eq!(p_vite.role, ProcessRole::DevServer);
        assert_eq!(p_vite.role_label, "Vite Dev");

        let p_java = analyze_command_and_paths(
            "java",
            Some("java -Dspring.application.name=orders -jar target/orders.jar"),
            Some(Path::new(r"C:\dev\orders-api")),
            None,
            None,
        );
        assert_eq!(p_java.explicit_project_name.as_deref(), Some("orders"));
        assert_eq!(
            p_java.project_seed_dir.as_deref(),
            Some(Path::new(r"C:\dev\orders-api"))
        );
    }

    #[test]
    fn extract_mode_flag_and_log_redirect_and_node_modules_and_root_guard() {
        assert_eq!(
            extract_mode_flag(&["app".into(), "--profile=prod".into()]).as_deref(),
            Some("prod")
        );
        assert_eq!(
            extract_mode_flag(&["app".into(), "--target=staging".into()]).as_deref(),
            Some("staging")
        );
        assert_eq!(
            extract_log_redirect_path("sh -c 'node app.js >> /dev/null 2>&1'", Some(Path::new("/app"))),
            None
        );
        assert_eq!(
            extract_log_redirect_path("node app.js > out.log", Some(Path::new("/app"))),
            Some(PathBuf::from("/app").join("out.log"))
        );
        assert_eq!(
            strip_node_modules_parent(Path::new(r"C:\dev\clean_node_modules\index.js")),
            None
        );
        assert!(strip_node_modules_parent(Path::new(r"C:\dev\app\node_modules\.bin\vite.js")).is_some());
        assert_eq!(
            infer_project_dir_from_paths(None, Some(Path::new(r"C:\scripts\worker.py")), None),
            None
        );
        assert_eq!(
            infer_project_dir_from_paths(None, Some(Path::new("/scripts/worker.py")), None),
            None
        );
    }

    #[test]
    fn format_job_title_localizes_service_suffix() {
        let parsed = ParsedCommand {
            role: ProcessRole::ApiBackend,
            role_label: "Node Service".into(),
            script_path: Some(PathBuf::from(r"C:\dev\acme\packages\standalone\cli.mjs")),
            script_dir: Some(PathBuf::from(r"C:\dev\acme\packages\standalone")),
            script_file_name: Some("cli.mjs".into()),
            module_name: None,
            mode_suffix: None,
            project_seed_dir: None,
            inferred_project_dir: None,
            explicit_project_name: None,
            canonical_entrypoint: None,
        };
        assert_eq!(
            format_job_title(&parsed, "Acme", "acme", "node.exe", None, false, Language::En),
            "Standalone service"
        );
        assert_eq!(
            format_job_title(&parsed, "Acme", "acme", "node.exe", None, false, Language::Tr),
            "Standalone servisi"
        );
    }
}


