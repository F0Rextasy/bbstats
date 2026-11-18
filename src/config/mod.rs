//! Application configuration persisted in the user's config directory (`bbstats/config.toml`).

pub mod autostart;

use crate::model::{Language, SortMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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
    AltSpace,
    CtrlShiftP,
}

impl HotkeyPreset {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn label(self) -> &'static str {
        match self {
            Self::AltSpace => "Alt + Space",
            Self::CtrlShiftP => "Ctrl + Shift + P",
        }
    }
}

fn default_true() -> bool {
    true
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
            hover_peek_delay_ms: 190,
            autostart: false,
            hotkey: HotkeyPreset::AltSpace,
            collapsed_poll_ms: 2500,
            expanded_poll_ms: 600,
            sort_mode: SortMode::Custom,
            pinned_projects: Vec::new(),
            hidden_projects: Vec::new(),
            project_order: Vec::new(),
            job_order: Vec::new(),
            project_aliases: HashMap::new(),
        }
    }
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

    /// Loads config from `AppConfig::config_path()`, or returns defaults if missing/invalid.
    pub fn load() -> Self {
        let path = Self::config_path();
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(raw) => toml::from_str::<AppConfig>(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        cfg.idle_opacity = cfg.idle_opacity.clamp(0.0, 0.95);
        cfg.glass_opacity = cfg.glass_opacity.clamp(0.25, 0.98);
        cfg.hover_peek_delay_ms = cfg.hover_peek_delay_ms.clamp(80, 600);
        cfg.autostart = autostart::is_autostart_enabled();
        cfg
    }

    /// Atomically writes config to `AppConfig::config_path()`.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
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
            std::fs::rename(&tmp, &path).map_err(|e| format!("rename config failed: {e}"))?;
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
}
