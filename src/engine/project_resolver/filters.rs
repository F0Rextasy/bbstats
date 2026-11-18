use std::path::Path;

use crate::model::ProcessRole;

pub(super) fn is_system_or_app_location(image_path: Option<&Path>, cwd: Option<&Path>) -> bool {
    for p in [image_path, cwd].into_iter().flatten() {
        let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if s.contains("\\windows\\")
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
        if s.contains("\\windows\\")
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
            let home_low = home.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
            if s.starts_with(&home_low) && s.len() > home_low.len() + 1 {
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
    if matches!(
        lower_exe,
        "code.exe"
            | "cursor.exe"
            | "windsurf.exe"
            | "antigravity.exe"
            | "devenv.exe"
            | "rust-analyzer.exe"
            | "clangd.exe"
            | "pylance.exe"
    ) || lower_exe.starts_with("language_server")
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
    if !matches!(
        lower_exe,
        "powershell.exe" | "pwsh.exe" | "cmd.exe" | "windowsterminal.exe" | "wt.exe" | "bash.exe"
    ) {
        return false;
    }
    let lower_cmd = cmdline.unwrap_or("").to_ascii_lowercase();
    if lower_exe == "cmd.exe" {
        return !lower_cmd.contains("/c ");
    }
    if lower_exe == "powershell.exe" || lower_exe == "pwsh.exe" {
        if lower_cmd.contains("-noexit") {
            return true;
        }
        return !lower_cmd.contains(".ps1") && !lower_cmd.contains("-file ");
    }
    true
}

pub(super) fn is_fast_skip_noise(lower_exe: &str, pid: u32, self_pid: u32, session_id: u32) -> bool {
    if pid <= 4 || pid == self_pid || lower_exe == "bbstats.exe" || lower_exe == "bbstats" {
        return true;
    }
    if session_id == 0 && !is_docker_process(lower_exe) && !is_known_dev_runtime(lower_exe) {
        return true;
    }
    matches!(
        lower_exe,
        "system idle process"
            | "system"
            | "registry"
            | "memcompression"
            | "smss.exe"
            | "csrss.exe"
            | "wininit.exe"
            | "services.exe"
            | "lsass.exe"
            | "winlogon.exe"
            | "dwm.exe"
            | "svchost.exe"
            | "sihost.exe"
            | "fontdrvhost.exe"
            | "runtimebroker.exe"
            | "searchindexer.exe"
            | "searchhost.exe"
            | "dllhost.exe"
            | "ctfmon.exe"
            | "taskhostw.exe"
            | "shellexperiencehost.exe"
            | "startmenuexperiencehost.exe"
            | "applicationframehost.exe"
            | "textinputhost.exe"
            | "lockapp.exe"
            | "msmpeng.exe"
            | "securityhealthservice.exe"
            | "widgets.exe"
            | "widgetservice.exe"
            | "audiodg.exe"
            | "spoolsv.exe"
            | "explorer.exe"
            | "conhost.exe"
            | "openconsole.exe"
            | "radeonsoftware.exe"
            | "amdrsserv.exe"
            | "atiesrxx.exe"
            | "atieclxx.exe"
            | "amdfendrsr.exe"
            | "nvcontainer.exe"
            | "nvdisplay.container.exe"
            | "chrome.exe"
            | "msedge.exe"
            | "msedgewebview2.exe"
            | "firefox.exe"
            | "discord.exe"
            | "spotify.exe"
            | "onedrive.exe"
            | "antigravity.exe"
            | "code.exe"
            | "cursor.exe"
            | "tern.exe"
            | "kitty.exe"
            | "alacritty.exe"
            | "wezterm-gui.exe"
    ) || lower_exe.starts_with("language_server")
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
    let s = dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    s.ends_with(":\\windows")
        || s.ends_with(":\\windows\\system32")
        || s.ends_with(":\\program files")
        || s.ends_with(":\\program files (x86)")
        || s.ends_with(":\\users")
        || s.ends_with("\\users")
        || s.ends_with("\\desktop")
        || s.ends_with("\\documents")
}

pub(super) fn is_generic_dir(dir: &Path) -> bool {
    let s = dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    s.ends_with("\\system32")
        || s.ends_with("\\windows")
        || s.ends_with("\\temp")
        || s.ends_with("\\tmp")
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
    matches!(
        lower_exe,
        "code.exe"
            | "cursor.exe"
            | "windsurf.exe"
            | "antigravity.exe"
            | "devenv.exe"
            | "windowsterminal.exe"
            | "wt.exe"
            | "tern.exe"
            | "kitty.exe"
            | "alacritty.exe"
            | "wezterm-gui.exe"
            | "pwsh.exe"
            | "powershell.exe"
            | "cmd.exe"
            | "explorer.exe"
    )
}

pub(super) fn is_helper_process(lower_exe: &str, role: ProcessRole) -> bool {
    matches!(
        lower_exe,
        "conhost.exe" | "openconsole.exe" | "esbuild.exe" | "rustc.exe"
    ) || matches!(role, ProcessRole::IdeTooling)
}

