use std::path::Path;

use crate::model::ProcessRole;

pub(super) fn path_str_starts_with_dir(path_norm: &str, dir_norm: &str) -> bool {
    let dir_trimmed = dir_norm.trim_end_matches('\\');
    if dir_trimmed.is_empty() {
        return false;
    }
    let path_trimmed = path_norm.trim_end_matches('\\');
    path_trimmed == dir_trimmed
        || (path_trimmed.starts_with(dir_trimmed)
            && path_trimmed.as_bytes().get(dir_trimmed.len()) == Some(&b'\\'))
}

fn has_unix_system_prefix(s: &str) -> bool {
    s.starts_with("\\system\\")
        || s == "\\system"
        || s.starts_with("\\library\\")
        || s == "\\library"
        || s.starts_with("\\usr\\")
        || s == "\\usr"
        || s.starts_with("\\bin\\")
        || s == "\\bin"
        || s.starts_with("\\sbin\\")
        || s == "\\sbin"
        || s.starts_with("\\applications\\")
        || s == "\\applications"
        || s.starts_with("\\private\\var\\")
        || s.starts_with("\\opt\\homebrew\\")
        || s.contains("\\library\\caches\\")
        || s.contains("\\library\\application support\\")
}

fn is_excluded_system_path(s: &str) -> bool {
    has_unix_system_prefix(s)
        || s.contains("\\windows\\")
        || s.ends_with("\\windows")
        || s.contains("\\systemapps\\")
        || s.contains("\\program files")
        || s.contains("\\windowsapps\\")
        || s.contains("\\programdata\\")
        || s.ends_with("\\programdata")
        || s.contains("\\appdata\\")
        || s.contains("\\.vscode")
        || s.contains("\\.antigravity")
        || s.contains("\\.gemini")
}

pub(super) fn is_system_or_app_location(image_path: Option<&Path>, cwd: Option<&Path>) -> bool {
    for p in [image_path, cwd].into_iter().flatten() {
        let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if is_excluded_system_path(&s) {
            return true;
        }
    }
    false
}

pub(super) fn is_user_project_location(
    script_dir: Option<&Path>,
    cwd: Option<&Path>,
    image_path: Option<&Path>,
    user_home: Option<&Path>,
) -> bool {
    let candidates = [script_dir, cwd, image_path.and_then(Path::parent)];
    for path_opt in candidates {
        let Some(p) = path_opt else {
            continue;
        };
        let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if is_excluded_system_path(&s)
            || s.contains("\\.cache")
            || s.contains("\\.cargo\\")
            || s.contains("\\.rustup\\")
        {
            continue;
        }
        let s_norm = normalize_path_str(p);
        let is_home = match user_home {
            Some(home) => s_norm == normalize_path_str(home),
            None => is_user_home_dir(p),
        };
        if is_home || is_generic_dir(p) {
            continue;
        }
        if let Some(home) = user_home {
            let home_norm = normalize_path_str(home);
            if s_norm != home_norm && path_str_starts_with_dir(&s_norm, &home_norm) {
                return true;
            }
        }
        if s.contains("\\desktop\\")
            || s.contains("\\documents\\")
            || s.contains("\\projects\\")
            || s.contains("\\dev\\")
            || s.contains("\\work\\")
            || s.contains("\\repos\\")
            || s.contains("\\src\\")
        {
            return true;
        }
    }
    false
}

pub(super) fn is_user_home_dir(dir: &Path) -> bool {
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        if normalize_path_str(dir) == normalize_path_str(Path::new(&home)) {
            return true;
        }
    }
    false
}

pub(super) fn is_generic_container_dir(dir: &Path) -> bool {
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "desktop" | "documents" | "downloads" | "users" | "projects" | "dev" | "work" | "repos"
    )
}

pub(super) fn is_ide_or_sidecar_process(
    lower_exe: &str,
    cmdline: Option<&str>,
    image_path: Option<&Path>,
    cwd: Option<&Path>,
) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    if matches!(
        base,
        "code"
            | "cursor"
            | "windsurf"
            | "antigravity"
            | "devenv"
            | "rust-analyzer"
            | "clangd"
            | "pylance"
    ) || base.starts_with("language_server")
    {
        return true;
    }

    let lower_cmd = cmdline.unwrap_or("").to_ascii_lowercase();
    if lower_cmd.contains("tsserver.js")
        || lower_cmd.contains("eslint")
        || lower_cmd.contains("prettier")
        || lower_cmd.contains("vscode-languageserver")
        || lower_cmd.contains("tailwindcss-language-server")
        || lower_cmd.contains(".antigravity")
        || lower_cmd.contains(".vscode")
        || lower_cmd.contains(".gemini")
    {
        return true;
    }

    for p in [image_path, cwd].into_iter().flatten() {
        let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if s.contains("\\antigravity\\")
            || s.contains("\\.antigravity\\")
            || s.contains("\\.vscode\\")
            || s.contains("\\.cursor\\")
            || s.contains("\\.gemini\\")
        {
            return true;
        }
    }
    false
}

pub(super) fn is_interactive_shell_process(lower_exe: &str, cmdline: Option<&str>) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    if !matches!(
        base,
        "powershell" | "pwsh" | "cmd" | "windowsterminal" | "wt" | "bash" | "zsh" | "sh" | "fish"
    ) {
        return false;
    }
    let args = super::cmdline::split_cmdline_args(cmdline.unwrap_or(""));
    let rest = if args.len() > 1 { &args[1..] } else { &[][..] };
    if base == "cmd" {
        let has_c = rest.iter().any(|a| {
            let low = a.to_ascii_lowercase();
            low == "/c" || low.starts_with("/c")
        });
        return !has_c;
    }
    if base == "powershell" || base == "pwsh" {
        if rest.iter().any(|a| a.eq_ignore_ascii_case("-noexit")) {
            return true;
        }
        let has_exec = rest.iter().any(|a| {
            let low = a.to_ascii_lowercase();
            low.ends_with(".ps1")
                || matches!(
                    low.as_str(),
                    "-file" | "-f" | "-command" | "-c" | "-encodedcommand" | "-enc" | "-e"
                )
        });
        return !has_exec;
    }
    if matches!(base, "bash" | "zsh" | "sh" | "fish") {
        let has_exec = rest.iter().any(|a| {
            if a == "-c"
                || (a.starts_with('-')
                    && !a.starts_with("--")
                    && a.len() >= 2
                    && a.ends_with('c'))
            {
                return true;
            }
            !a.starts_with('-') && !a.is_empty()
        });
        return !has_exec;
    }
    true
}

pub(super) fn is_fast_skip_noise(lower_exe: &str, pid: u32, self_pid: u32, session_id: u32) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    if pid <= 4 || pid == self_pid || base == "bbstats" {
        return true;
    }
    if session_id == 0 && !is_docker_process(lower_exe) && !is_known_dev_runtime(lower_exe) {
        return true;
    }
    matches!(
        base,
        "system idle process"
            | "system"
            | "registry"
            | "memcompression"
            | "smss"
            | "csrss"
            | "wininit"
            | "services"
            | "lsass"
            | "winlogon"
            | "dwm"
            | "svchost"
            | "sihost"
            | "fontdrvhost"
            | "runtimebroker"
            | "searchindexer"
            | "searchhost"
            | "dllhost"
            | "ctfmon"
            | "taskhostw"
            | "shellexperiencehost"
            | "startmenuexperiencehost"
            | "applicationframehost"
            | "textinputhost"
            | "lockapp"
            | "msmpeng"
            | "securityhealthservice"
            | "widgets"
            | "widgetservice"
            | "audiodg"
            | "spoolsv"
            | "explorer"
            | "conhost"
            | "openconsole"
            | "radeonsoftware"
            | "amdrsserv"
            | "atiesrxx"
            | "atieclxx"
            | "amdfendrsr"
            | "nvcontainer"
            | "nvdisplay.container"
            | "chrome"
            | "msedge"
            | "msedgewebview2"
            | "firefox"
            | "discord"
            | "spotify"
            | "onedrive"
            | "antigravity"
            | "code"
            | "cursor"
            | "tern"
            | "kitty"
            | "alacritty"
            | "wezterm-gui"
            | "launchd"
            | "kernel_task"
            | "windowserver"
            | "loginwindow"
            | "sysmond"
            | "usereventagent"
            | "mdworker"
            | "mdworker_shared"
            | "mds"
            | "mds_stores"
            | "spotlight"
            | "finder"
            | "dock"
            | "systemuiserver"
            | "cfprefsd"
            | "distnoted"
            | "trustd"
            | "nsurlsessiond"
            | "controlcenter"
            | "cloudd"
            | "bird"
            | "terminal"
            | "iterm2"
            | "warp"
            | "ghostty"
    ) || base.starts_with("language_server")
}


pub(super) fn normalize_path_str(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

pub(super) fn normalize_project_id(path: &Path) -> String {
    format!("path:{}", normalize_path_str(path))
}

pub(super) fn is_stop_directory(dir: &Path, user_home: Option<&Path>) -> bool {
    if dir.parent().is_none() {
        return true;
    }
    if let Some(home) = user_home {
        if normalize_path_str(dir) == normalize_path_str(home) {
            return true;
        }
    }
    let s = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    s.is_empty()
        || s.ends_with(":\\windows")
        || s.ends_with(":\\windows\\system32")
        || s.ends_with(":\\program files")
        || s.ends_with(":\\program files (x86)")
        || s.ends_with(":\\users")
        || s == "\\users"
        || s == "\\home"
        || s == "\\applications"
        || s == "\\library"
        || s == "\\system"
        || s == "\\volumes"
        || s.ends_with("\\desktop")
        || s.ends_with("\\documents")
}

pub(super) fn is_generic_dir(dir: &Path) -> bool {
    if dir.parent().is_none() {
        return true;
    }
    let s = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    s.is_empty()
        || s.ends_with("\\system32")
        || s.ends_with("\\windows")
        || s.ends_with("\\temp")
        || s.ends_with("\\tmp")
        || s == "\\tmp"
        || s == "\\var\\tmp"
        || s == "\\private\\tmp"
        || s == "\\etc"
        || s == "\\usr"
}

pub(super) fn is_wsl_vm_name(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    low == "vmmemwsl" || low == "vmmem" || low == "vmmem.exe" || low == "wslhost.exe"
}

pub(super) fn is_docker_process(lower_exe: &str) -> bool {
    matches!(
        lower_exe,
        "dockerd.exe"
            | "com.docker.backend.exe"
            | "docker.exe"
            | "com.docker.build.exe"
            | "dockerd"
            | "com.docker.backend"
            | "docker"
            | "com.docker.build"
    )
}

pub(super) fn is_known_dev_runtime(lower_exe: &str) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    if base.starts_with("python") {
        return true;
    }
    matches!(
        base,
        "node"
            | "bun"
            | "deno"
            | "uvicorn"
            | "cargo"
            | "rustc"
            | "java"
            | "javaw"
            | "dotnet"
            | "go"
            | "air"
            | "postgres"
            | "mysqld"
            | "redis-server"
            | "mongod"
            | "surreal"
    )
}

pub(super) fn is_ide_or_shell_name(lower_exe: &str) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    matches!(
        base,
        "code"
            | "cursor"
            | "windsurf"
            | "antigravity"
            | "devenv"
            | "windowsterminal"
            | "wt"
            | "tern"
            | "kitty"
            | "alacritty"
            | "wezterm-gui"
            | "terminal"
            | "iterm2"
            | "warp"
            | "ghostty"
            | "pwsh"
            | "powershell"
            | "cmd"
            | "bash"
            | "zsh"
            | "sh"
            | "fish"
            | "explorer"
    )
}

pub(super) fn is_helper_process(lower_exe: &str, role: ProcessRole) -> bool {
    let base = lower_exe.strip_suffix(".exe").unwrap_or(lower_exe);
    matches!(base, "conhost" | "openconsole" | "esbuild" | "rustc")
        || matches!(role, ProcessRole::IdeTooling)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_prefix_and_stop_directory_handle_boundaries_and_case() {
        assert!(path_str_starts_with_dir(r"c:\users\dev\app", r"c:\users\dev"));
        assert!(!path_str_starts_with_dir(r"c:\users\dev2\app", r"c:\users\dev"));
        assert!(path_str_starts_with_dir(r"c:\users\dev\", r"c:\users\dev"));

        assert!(is_stop_directory(Path::new(r"c:\users\dev"), Some(Path::new(r"C:\Users\Dev"))));
        assert!(is_stop_directory(Path::new("/home"), None));
        assert!(is_generic_dir(Path::new(r"C:\")));
        assert!(is_generic_dir(Path::new("/")));
    }

    #[test]
    fn interactive_shell_allows_script_and_command_flags() {
        assert!(is_interactive_shell_process("cmd.exe", Some("cmd.exe")));
        assert!(!is_interactive_shell_process("cmd.exe", Some("cmd.exe /c node a.js")));
        assert!(!is_interactive_shell_process("cmd.exe", Some(r#"cmd.exe /c"node a.js""#)));
        assert!(is_interactive_shell_process("pwsh.exe", Some("pwsh.exe -NoExit -File a.ps1")));
        assert!(!is_interactive_shell_process("pwsh.exe", Some("pwsh.exe -Command node a.js")));
        assert!(!is_interactive_shell_process("bash", Some("bash -lc 'npm run dev'")));
        assert!(!is_interactive_shell_process("sh", Some("sh /opt/app/worker.sh")));
        assert!(is_interactive_shell_process("zsh", Some("zsh -l")));
    }

    #[test]
    fn dev_runtime_and_user_project_location_work_cross_platform() {
        assert!(is_known_dev_runtime("python3.12"));
        assert!(is_known_dev_runtime("surreal"));
        assert!(is_known_dev_runtime("javaw.exe"));
        assert!(is_fast_skip_noise("svchost.exe", 500, 100, 0));
        assert!(!is_fast_skip_noise("dockerd.exe", 500, 100, 0));
        assert!(!is_user_project_location(
            Some(Path::new("/tmp/fakehome")),
            None,
            None,
            Some(Path::new("/tmp/fakehome"))
        ));
        assert!(is_user_project_location(
            Some(Path::new("/tmp/fakehome/myapp")),
            None,
            None,
            Some(Path::new("/tmp/fakehome"))
        ));
    }
}


