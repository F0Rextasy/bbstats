//! Cross-platform login autostart management:
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! - macOS: `~/Library/LaunchAgents/io.github.bbstats.plist`

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
    };

    const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "bbstats";

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn is_autostart_enabled() -> bool {
        let subkey = to_wide(RUN_SUBKEY);
        let value = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        unsafe {
            let res = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            );
            if res != ERROR_SUCCESS {
                return false;
            }
            let query = RegQueryValueExW(
                hkey,
                PCWSTR(value.as_ptr()),
                None,
                None,
                None,
                None,
            );
            let _ = RegCloseKey(hkey);
            query == ERROR_SUCCESS
        }
    }

    pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
        let subkey = to_wide(RUN_SUBKEY);
        let value = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        unsafe {
            let res = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                0,
                KEY_WRITE,
                &mut hkey,
            );
            if res != ERROR_SUCCESS {
                return Err(format!("RegOpenKeyExW failed: {:?}", res));
            }

            let op_result = if enabled {
                let exe = std::env::current_exe()
                    .map_err(|e| format!("current_exe failed: {e}"))?;
                let quoted = format!("\"{}\"", exe.display());
                let wide_val = to_wide(&quoted);
                let bytes = std::slice::from_raw_parts(
                    wide_val.as_ptr() as *const u8,
                    wide_val.len() * 2,
                );
                RegSetValueExW(hkey, PCWSTR(value.as_ptr()), 0, REG_SZ, Some(bytes))
            } else {
                let del = RegDeleteValueW(hkey, PCWSTR(value.as_ptr()));
                if del == ERROR_SUCCESS || del.0 == 2 {
                    // ERROR_FILE_NOT_FOUND (2) is fine when disabling
                    ERROR_SUCCESS
                } else {
                    del
                }
            };

            let _ = RegCloseKey(hkey);
            if op_result != ERROR_SUCCESS {
                return Err(format!("Registry operation failed: {:?}", op_result));
            }
            let _ = PWSTR::null();
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::PathBuf;

    const LAUNCH_AGENT_LABEL: &str = "io.github.bbstats";

    fn launch_agent_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        #[cfg(target_os = "macos")]
        return Some(
            PathBuf::from(home)
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCH_AGENT_LABEL}.plist")),
        );
        #[cfg(not(target_os = "macos"))]
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("autostart")
                .join("bbstats.desktop"),
        );
    }

    pub fn is_autostart_enabled() -> bool {
        let Some(path) = launch_agent_path() else {
            return false;
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return false;
        };
        #[cfg(target_os = "macos")]
        {
            raw.contains("<key>RunAtLoad</key>") && raw.contains("<true/>")
        }
        #[cfg(not(target_os = "macos"))]
        {
            !raw.trim().is_empty()
        }
    }

    pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
        let path = launch_agent_path().ok_or_else(|| "HOME not set".to_string())?;
        if !enabled {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("remove autostart file failed: {e}")),
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create_dir_all failed: {e}"))?;
            }
            let exe =
                std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
            #[cfg(target_os = "macos")]
            let body = format!(
                concat!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
                    "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
                    "<plist version=\"1.0\">\n<dict>\n",
                    "  <key>Label</key><string>{label}</string>\n",
                    "  <key>ProgramArguments</key>\n  <array>\n",
                    "    <string>{exe}</string>\n",
                    "    <string>--daemon</string>\n",
                    "  </array>\n",
                    "  <key>RunAtLoad</key><true/>\n",
                    "</dict>\n</plist>\n"
                ),
                label = LAUNCH_AGENT_LABEL,
                exe = xml_escape(&exe.display().to_string()),
            );
            #[cfg(not(target_os = "macos"))]
            let body = format!(
                "[Desktop Entry]\nType=Application\nName=bbstats\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
                exe.display()
            );
            std::fs::write(&path, body).map_err(|e| format!("write autostart failed: {e}"))
        }
    }

    #[cfg(target_os = "macos")]
    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

/// Returns whether launch-on-login is currently enabled.
pub fn is_autostart_enabled() -> bool {
    imp::is_autostart_enabled()
}

/// Enables or disables automatic launch of `bbstats` on user login.
pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    imp::set_autostart_enabled(enabled)
}
