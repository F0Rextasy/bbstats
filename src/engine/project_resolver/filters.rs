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

pub(super) fn is_system_or_app_location(image_path: Option<&Path>, cwd: Option<&Path>) -> bool {
    for p in [image_path, cwd].into_iter().flatten() {
        let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if has_unix_system_prefix(&s)
            || s.contains("\\windows\\")
            || s.ends_with("\\windows")
            || s.contains("\\systemapps\\")
            || s.contains("\\program files")
            || s.contains("\\windowsapps\\")
            || s.contains("\\programdata\\")
            || s.contains("\\appdata\\")
            || s.contains("\\.vscode")
            || s.contains("\\.antigravity")
            || s.contains("\\.gemini")
        {
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
        if has_unix_system_prefix(&s)
            || s.contains("\\windows\\")
            || s.ends_with("\\windows")
            || s.contains("\\program files")
            || s.contains("\\programdata")
            || s.contains("\\appdata\\")
            || s.contains("\\.vscode")
            || s.contains("\\.antigravity")
            || s.contains("\\.gemini")
            || s.contains("\\.cache")
            || s.contains("\\.cargo\\")
            || s.contains("\\.rustup\\")
        {
            continue;
        }
        if is_user_home_dir(p) || is_generic_dir(p) {
            continue;
        }
        if let Some(home) = user_home {
            let s_norm = normalize_path_str(p);
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
    let lower_cmd = cmdline.unwrap_or("").to_ascii_lowercase();
    if base == "cmd" {
        return !lower_cmd.contains("/c ");
    }
    if base == "powershell" || base == "pwsh" {
        if lower_cmd.contains("-noexit") {
            return true;
        }
        return !lower_cmd.contains(".ps1") && !lower_cmd.contains("-file ");
    }
    if matches!(base, "bash" | "zsh" | "sh" | "fish") {
        return !lower_cmd.contains(" -c ");
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
        if dir == home {
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
        || s == "\\applications"
        || s == "\\library"
        || s == "\\system"
        || s == "\\volumes"
        || s.ends_with("\\desktop")
        || s.ends_with("\\documents")
}

pub(super) fn is_generic_dir(dir: &Path) -> bool {
    let s = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    s.ends_with("\\system32")
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
    matches!(
        lower_exe,
        "node.exe"
            | "bun.exe"
            | "deno.exe"
            | "python.exe"
            | "python3.exe"
            | "pythonw.exe"
            | "uvicorn.exe"
            | "cargo.exe"
            | "rustc.exe"
            | "java.exe"
            | "javaw.exe"
            | "dotnet.exe"
            | "go.exe"
            | "air.exe"
            | "postgres.exe"
            | "mysqld.exe"
            | "redis-server.exe"
            | "mongod.exe"
            | "node"
            | "bun"
            | "deno"
            | "python"
            | "python3"
            | "pythonw"
            | "uvicorn"
            | "cargo"
            | "rustc"
            | "java"
            | "dotnet"
            | "go"
            | "air"
            | "postgres"
            | "mysqld"
            | "redis-server"
            | "mongod"
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

