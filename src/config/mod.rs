//! Application configuration persisted in the user's config directory (`bbstats/config.toml`).

pub mod autostart;

use crate::model::{Language, SortMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

/// Screen edge where the Dynamic Island pill is docked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    /// Top-center of the target monitor; expands downwards and symmetrically.
    #[default]
    TopCenter,
    /// Right edge of the target monitor, vertically centered; expands leftwards.
    RightEdge,
}

/// Global hotkey preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyPreset {
    #[default]
    AltShiftB,
    CtrlAltB,
    AltSpace,
    CtrlShiftP,
    Disabled,
}

impl HotkeyPreset {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn label(self, lang: Language) -> &'static str {
        match self {
            Self::AltShiftB => "Alt + Shift + B",
            Self::CtrlAltB => "Ctrl + Alt + B",
            Self::AltSpace => "Alt + Space",
            Self::CtrlShiftP => "Ctrl + Shift + P",
            Self::Disabled => lang.pick("Disabled", "Kapalı"),
        }
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn next(self) -> Self {
        match self {
            Self::AltShiftB => Self::CtrlAltB,
            Self::CtrlAltB => Self::AltSpace,
            Self::AltSpace => Self::CtrlShiftP,
            Self::CtrlShiftP => Self::Disabled,
            Self::Disabled => Self::AltShiftB,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_cpu_warn() -> f32 {
    85.0
}

fn default_ram_warn_mb() -> u64 {
    1536
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default)]
    pub language: Language,
    pub dock_position: DockPosition,
    pub monitor_index: usize,
    pub always_on_top: bool,
    pub acrylic_enabled: bool,
    /// Opacity when collapsed/unclicked (0.0 = fully transparent HUD stats only, up to 0.95).
    pub idle_opacity: f32,
    /// Opacity when expanded/hovered.
    pub glass_opacity: f32,
    pub hide_on_fullscreen: bool,
    #[serde(default = "default_true")]
    pub notify_on_crash: bool,
    #[serde(default)]
    pub wsl_bridge_enabled: bool,
    #[serde(default = "default_cpu_warn")]
    pub cpu_warn_percent: f32,
    #[serde(default = "default_ram_warn_mb")]
    pub ram_warn_mb: u64,
    pub hover_peek_delay_ms: u32,
    pub autostart: bool,
    pub hotkey: HotkeyPreset,
    pub collapsed_poll_ms: u64,
    pub expanded_poll_ms: u64,
    pub sort_mode: SortMode,
    pub pinned_projects: Vec<String>,
    pub hidden_projects: Vec<String>,
    pub project_order: Vec<String>,
    pub job_order: Vec<String>,
    pub project_aliases: HashMap<String, String>,
    #[serde(skip)]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub load_error: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            language: Language::En,
            dock_position: DockPosition::TopCenter,
            monitor_index: 0,
            always_on_top: true,
            acrylic_enabled: true,
            idle_opacity: 0.14,
            glass_opacity: 0.88,
            hide_on_fullscreen: true,
            notify_on_crash: true,
            wsl_bridge_enabled: false,
            cpu_warn_percent: default_cpu_warn(),
            ram_warn_mb: default_ram_warn_mb(),
            hover_peek_delay_ms: 190,
            autostart: false,
            hotkey: HotkeyPreset::AltShiftB,
            collapsed_poll_ms: 2500,
            expanded_poll_ms: 600,
            sort_mode: SortMode::Custom,
            pinned_projects: Vec::new(),
            hidden_projects: Vec::new(),
            project_order: Vec::new(),
            job_order: Vec::new(),
            project_aliases: HashMap::new(),
            load_error: None,
        }
    }
}

/// Appends a bounded diagnostic entry to `bbstats.log` inside the config directory.
pub fn log_error(msg: &str) {
    let Some(dir) = AppConfig::config_path().parent().map(Path::to_path_buf) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let log_path = dir.join("bbstats.log");
    const MAX_LOG_BYTES: u64 = 128 * 1024;
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() >= MAX_LOG_BYTES {
            let _ = std::fs::rename(&log_path, dir.join("bbstats.log.old"));
        }
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// Backs up a corrupt TOML file to `*.toml.bak` without overwriting user edits.
pub(crate) fn backup_corrupt_toml(path: &Path) -> PathBuf {
    let bak = path.with_extension("toml.bak");
    let _ = std::fs::copy(path, &bak);
    bak
}

impl AppConfig {
    /// Returns the OS-appropriate config file path (`bbstats/config.toml`).
    pub fn config_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        #[cfg(target_os = "macos")]
        let base = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
            .unwrap_or_else(|| PathBuf::from("."));

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));

        base.join("bbstats").join("config.toml")
    }

    /// Loads config from `AppConfig::config_path()`, backing up corrupt files instead of silently resetting them.
    pub fn load() -> Self {
        let mut cfg = Self::load_from_path(&Self::config_path());
        cfg.autostart = autostart::is_autostart_enabled();
        cfg
    }

    pub fn load_from_path(path: &Path) -> Self {
        let mut cfg = match std::fs::read_to_string(path) {
            Ok(raw) if !raw.trim().is_empty() => match toml::from_str::<AppConfig>(&raw) {
                Ok(parsed) => parsed,
                Err(err) => {
                    let bak = backup_corrupt_toml(path);
                    let msg = format!(
                        "config.toml parse error ({}); backed up to {}",
                        err,
                        bak.display()
                    );
                    log_error(&msg);
                    Self {
                        load_error: Some(msg),
                        ..Self::default()
                    }
                }
            },
            _ => Self::default(),
        };
        let def = Self::default();
        cfg.idle_opacity = if cfg.idle_opacity.is_finite() {
            cfg.idle_opacity.clamp(0.0, 0.95)
        } else {
            def.idle_opacity
        };
        cfg.glass_opacity = if cfg.glass_opacity.is_finite() {
            cfg.glass_opacity.clamp(0.25, 0.98)
        } else {
            def.glass_opacity
        };
        cfg.hover_peek_delay_ms = cfg.hover_peek_delay_ms.clamp(80, 600);
        cfg.collapsed_poll_ms = cfg.collapsed_poll_ms.clamp(500, 30_000);
        cfg.expanded_poll_ms = cfg.expanded_poll_ms.clamp(250, 10_000);
        cfg.cpu_warn_percent = if cfg.cpu_warn_percent.is_finite() {
            cfg.cpu_warn_percent.clamp(50.0, 100.0)
        } else {
            default_cpu_warn()
        };
        cfg.ram_warn_mb = cfg.ram_warn_mb.clamp(256, 32_768);
        cfg
    }

    pub fn to_sync_cmd(&self) -> crate::model::EngineCmd {
        crate::model::EngineCmd::SyncConfigState {
            pinned_projects: self.pinned_projects.clone(),
            hidden_projects: self.hidden_projects.clone(),
            project_aliases: self.project_aliases.clone(),
            project_order: self.project_order.clone(),
            job_order: self.job_order.clone(),
            sort_mode: self.sort_mode,
            language: self.language,
            wsl_bridge_enabled: self.wsl_bridge_enabled,
            cpu_warn_percent: self.cpu_warn_percent,
            ram_warn_mb: self.ram_warn_mb,
        }
    }

    /// Atomically writes config to `AppConfig::config_path()`, refusing to overwrite a corrupt file on disk.
    pub fn save(&self) -> Result<(), String> {
        self.save_to_path(&Self::config_path())
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Ok(existing) = std::fs::read_to_string(path) {
            if !existing.trim().is_empty() && toml::from_str::<AppConfig>(&existing).is_err() {
                let bak = backup_corrupt_toml(path);
                let msg = format!(
                    "Refusing to overwrite corrupt config.toml (preserved at {})",
                    bak.display()
                );
                log_error(&msg);
                return Err(msg);
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create_dir_all failed: {e}"))?;
        }
        let serialized =
            toml::to_string_pretty(self).map_err(|e| format!("toml serialize failed: {e}"))?;

        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, serialized.as_bytes())
            .map_err(|e| format!("write tmp config failed: {e}"))?;

        #[cfg(target_os = "windows")]
        {
            let src_wide: Vec<u16> = tmp
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let dst_wide: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
            unsafe {
                MoveFileExW(
                    PCWSTR(src_wide.as_ptr()),
                    PCWSTR(dst_wide.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
                .map_err(|e| format!("MoveFileExW failed: {e}"))?;
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            std::fs::rename(&tmp, path).map_err(|e| format!("rename config failed: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_position_roundtrip() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.language, Language::En);
        assert_eq!(cfg.dock_position, DockPosition::TopCenter);
        assert!(cfg.always_on_top);
        assert!(cfg.idle_opacity <= 0.20);
        assert_eq!(SortMode::Custom.label_for(Language::En), "Custom");
        assert_eq!(SortMode::Custom.label_for(Language::Tr), "Özel");

        cfg.language = Language::Tr;
        cfg.dock_position = DockPosition::RightEdge;
        cfg.always_on_top = false;
        cfg.idle_opacity = 0.0;
        cfg.sort_mode = SortMode::Cpu;
        cfg.project_aliases
            .insert("path:c:\\foo".to_string(), "My App".to_string());
        let s = toml::to_string_pretty(&cfg).unwrap();
        assert!(s.contains("right_edge"));
        assert!(s.contains("language = \"tr\""));
        let decoded: AppConfig = toml::from_str(&s).unwrap();
        assert_eq!(decoded.language, Language::Tr);
        assert_eq!(decoded.dock_position, DockPosition::RightEdge);
        assert!(!decoded.always_on_top);
        assert_eq!(decoded.idle_opacity, 0.0);
        assert_eq!(decoded.sort_mode, SortMode::Cpu);
        assert_eq!(
            decoded.project_aliases.get("path:c:\\foo").map(String::as_str),
            Some("My App")
        );
    }

    #[test]
    fn corrupt_config_is_backed_up_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("bbstats_cfg_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.toml");
        let bad_toml = "language = [broken\n";
        std::fs::write(&cfg_path, bad_toml).unwrap();

        let loaded = AppConfig::load_from_path(&cfg_path);
        assert!(loaded.load_error.is_some());
        let bak_path = dir.join("config.toml.bak");
        assert_eq!(std::fs::read_to_string(&bak_path).unwrap(), bad_toml);

        // Refusing to overwrite preserves the user's broken file until fixed.
        assert!(loaded.save_to_path(&cfg_path).is_err());
        assert_eq!(std::fs::read_to_string(&cfg_path).unwrap(), bad_toml);

        // NaN opacities must fall back to finite defaults rather than poisoning render math.
        let nan_path = dir.join("nan.toml");
        std::fs::write(&nan_path, "idle_opacity = nan\nglass_opacity = inf\n").unwrap();
        let nan_cfg = AppConfig::load_from_path(&nan_path);
        assert!(nan_cfg.idle_opacity.is_finite());
        assert!(nan_cfg.glass_opacity.is_finite());
        assert_eq!(nan_cfg.hotkey, HotkeyPreset::AltShiftB);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
