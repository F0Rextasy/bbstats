//! `exelban/stats`-inspired macOS Charcoal Translucent Glass design tokens.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[inline]
pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

pub const BG_PILL: D2D1_COLOR_F = rgba(18, 20, 26, 0.94);
pub const BG_SURFACE: D2D1_COLOR_F = rgba(20, 22, 28, 0.90);
pub const BG_SURFACE_OPAQUE: D2D1_COLOR_F = rgba(20, 22, 28, 0.97);
pub const BG_WELL: D2D1_COLOR_F = rgba(0, 0, 0, 0.28);
pub const BG_CARD: D2D1_COLOR_F = rgba(255, 255, 255, 0.035);
pub const BG_ELEVATED: D2D1_COLOR_F = rgba(255, 255, 255, 0.065);
pub const BG_ELEVATED_HOVER: D2D1_COLOR_F = rgba(255, 255, 255, 0.115);
pub const BG_OVERLAY: D2D1_COLOR_F = rgba(20, 21, 24, 0.97);
pub const BG_TRACK: D2D1_COLOR_F = rgba(255, 255, 255, 0.11);

/// Parses `#RRGGBB` hex color strings from `labels.toml`.
pub fn parse_hex_color(hex: &str) -> Option<D2D1_COLOR_F> {
    let s = hex.trim().strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(rgba(r, g, b, 1.0))
}

pub const BORDER_OUTER_DARK: D2D1_COLOR_F = rgba(0, 0, 0, 0.68);
pub const BORDER_SPECULAR_TOP: D2D1_COLOR_F = rgba(255, 255, 255, 0.22);
pub const BORDER_SPECULAR_BOT: D2D1_COLOR_F = rgba(255, 255, 255, 0.06);
pub const BORDER_SUBTLE: D2D1_COLOR_F = rgba(255, 255, 255, 0.09);
pub const BORDER_FOCUS: D2D1_COLOR_F = rgba(10, 132, 255, 1.0);

pub const TEXT_PRIMARY: D2D1_COLOR_F = rgba(245, 245, 247, 1.0);
pub const TEXT_SECONDARY: D2D1_COLOR_F = rgba(174, 174, 178, 1.0);
pub const TEXT_MUTED: D2D1_COLOR_F = rgba(142, 142, 147, 1.0);

// Apple macOS system palette (matching exelban/stats)
pub const ACCENT_LIVE: D2D1_COLOR_F = rgba(48, 209, 88, 1.0);
pub const ACCENT_LIVE_GLOW: D2D1_COLOR_F = rgba(48, 209, 88, 0.22);
pub const ACCENT_WARN: D2D1_COLOR_F = rgba(255, 159, 10, 1.0);
pub const ACCENT_WARN_BG: D2D1_COLOR_F = rgba(255, 159, 10, 0.16);
pub const ACCENT_DANGER: D2D1_COLOR_F = rgba(255, 69, 58, 1.0);
pub const ACCENT_DANGER_BG: D2D1_COLOR_F = rgba(255, 69, 58, 0.18);
pub const ACCENT_INFO: D2D1_COLOR_F = rgba(10, 132, 255, 1.0);
pub const ACCENT_INFO_BG: D2D1_COLOR_F = rgba(10, 132, 255, 0.16);
pub const ACCENT_RAM: D2D1_COLOR_F = rgba(191, 90, 242, 1.0);
pub const ACCENT_TEAL: D2D1_COLOR_F = rgba(100, 210, 255, 1.0);

/// Palette cycle for project color swatches & segmented RAM bar (exelban/stats style).
pub const PROJECT_PALETTE: [D2D1_COLOR_F; 6] = [
    ACCENT_INFO,
    ACCENT_LIVE,
    ACCENT_WARN,
    ACCENT_RAM,
    ACCENT_TEAL,
    ACCENT_DANGER,
];

pub const FONT_UI: &str = "Segoe UI Variable Text";
pub const FONT_DISPLAY: &str = "Segoe UI Variable Display";
pub const FONT_MONO: &str = "Segoe UI Variable Text";

/// Critical-damped and alert spring parameters for island morphing.
#[derive(Debug, Clone, Copy)]
pub struct SpringSpec {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

pub const SPRING_MORPH: SpringSpec = SpringSpec {
    stiffness: 440.0,
    damping: 42.0,
    mass: 1.0,
};

pub const SPRING_ALERT: SpringSpec = SpringSpec {
    stiffness: 520.0,
    damping: 26.0,
    mass: 1.0,
};
