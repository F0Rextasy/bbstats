//! Direct2D view renderers for all Dynamic Island states (Job-first Obsidian Telemetry).

mod collapsed;
mod detail;
mod expanded;
mod peek;
mod settings;

use crate::config::{AppConfig, DockPosition};
use crate::model::{AlertKind, ProjectGroup, ProjectStatus, Snapshot};
use crate::platform::monitor::MonitorInfo;
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::HitMap;
use crate::ui::layout::{UiState, ViewMode};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};

pub struct MorphParams<'a> {
    pub prev: Option<(&'a ViewMode, f32, f32)>,
    pub target_w: f32,
    pub target_h: f32,
    pub morph_t: f32,
    pub bg_opacity: f32,
}

#[inline]
pub(super) fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

pub(super) fn status_color(status: ProjectStatus) -> D2D1_COLOR_F {
    match status {
        ProjectStatus::Healthy => ACCENT_LIVE,
        ProjectStatus::HighLoad => ACCENT_WARN,
        ProjectStatus::Crashed => ACCENT_DANGER,
        ProjectStatus::Stopped => TEXT_MUTED,
    }
}

/// Computes `(color, intensity)` for the top-edge glass ambient glow from current health.
pub fn status_ambient_glow(
    snap: &Snapshot,
    active_alert: Option<&crate::model::Alert>,
) -> Option<(D2D1_COLOR_F, f32)> {
    let any_crashed_job = snap
        .projects
        .iter()
        .flat_map(|p| p.jobs.iter())
        .any(|j| j.status == ProjectStatus::Crashed);
    if active_alert.is_some_and(|a| matches!(a.kind, AlertKind::Crash { .. })) || any_crashed_job
    {
        return Some((ACCENT_DANGER, 0.22));
    }

    let any_high_load = snap
        .projects
        .iter()
        .any(|p| p.status == ProjectStatus::HighLoad);
    if active_alert.is_some() || any_high_load || snap.total_dev_cpu >= 50.0 {
        return Some((ACCENT_WARN, 0.16));
    }

    if snap.total_jobs > 0 {
        return Some((ACCENT_LIVE, 0.11));
    }

    None
}

#[inline]
fn morph_crossfade_weights(t: f32, expanding: bool) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let (out_end, in_start) = if expanding { (0.28, 0.34) } else { (0.42, 0.52) };
    let out_a = (1.0 - t / out_end).clamp(0.0, 1.0);
    let in_a = ((t - in_start) / (1.0 - in_start)).clamp(0.0, 1.0);
    (out_a, in_a)
}

#[inline]
fn anchor_x(dock: DockPosition, container_w: f32, view_w: f32) -> f32 {
    match dock {
        DockPosition::RightEdge => container_w - view_w,
        DockPosition::TopCenter => (container_w - view_w) * 0.5,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_mode_content(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    view_w: f32,
    view_h: f32,
    mode: &ViewMode,
    state: &mut UiState,
    snap: &Snapshot,
    cfg: &AppConfig,
    monitors: &[MonitorInfo],
    active_alert: Option<&crate::model::Alert>,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    match mode {
        ViewMode::Collapsed => {
            collapsed::render_collapsed(ctx, fonts, view_w, view_h, snap, cfg, active_alert, hit_map)
        }
        ViewMode::HoverPeek => {
            peek::render_hover_peek(ctx, fonts, view_w, view_h, snap, cfg, hit_map)
        }
        ViewMode::Expanded => {
            expanded::render_expanded(ctx, fonts, view_w, view_h, state, snap, cfg, hit_map)
        }
        ViewMode::Detail { project_id, job_id } => {
            let pid_clone = project_id.clone();
            let jid_clone = job_id.clone();
            detail::render_detail(
                ctx,
                fonts,
                view_w,
                view_h,
                state,
                snap,
                cfg,
                &pid_clone,
                jid_clone.as_ref(),
                hit_map,
            )
        }
        ViewMode::Settings => {
            settings::render_settings(ctx, fonts, view_w, view_h, state, cfg, monitors, hit_map)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_ui(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    radius: f32,
    morph: MorphParams<'_>,
    state: &mut UiState,
    snap: &Snapshot,
    cfg: &AppConfig,
    monitors: &[MonitorInfo],
    acrylic_effective: bool,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    hit_map.clear();

    let active_alert = snap.alerts.iter().find(|a| !a.dismissed);

    let mut base_col = match (&state.mode, morph.prev) {
        (ViewMode::Collapsed, None) => BG_PILL,
        _ => {
            if acrylic_effective {
                BG_SURFACE
            } else {
                BG_SURFACE_OPAQUE
            }
        }
    };
    base_col.a = if active_alert.is_some() && matches!(state.mode, ViewMode::Collapsed) {
        morph.bg_opacity.max(0.88)
    } else {
        morph.bg_opacity
    };

    let tint_border = if let Some(alert) = active_alert {
        match alert.kind {
            AlertKind::Crash { .. } => {
                if matches!(state.mode, ViewMode::Collapsed) {
                    base_col = rgba(26, 12, 16, morph.bg_opacity.max(0.90));
                }
                Some(ACCENT_DANGER)
            }
            AlertKind::HighCpu { .. } | AlertKind::MemorySpike { .. } => {
                if matches!(state.mode, ViewMode::Collapsed) {
                    base_col = rgba(24, 19, 10, morph.bg_opacity.max(0.90));
                }
                Some(ACCENT_WARN)
            }
        }
    } else {
        None
    };

    let shadow_strength = ((base_col.a - 0.12) / 0.72).clamp(0.0, 1.0);
    ctx.draw_ambient_shadow(w, h, radius, shadow_strength)?;
    ctx.draw_glass_shell(w, h, radius, base_col, tint_border)?;

    let inner_clip = rect(1.0, 1.0, (w - 1.0).max(2.0), (h - 1.0).max(2.0));
    let inner_r = (radius - 1.0).max(2.0);
    let ambient = status_ambient_glow(snap, active_alert);

    ctx.with_rounded_clip(inner_clip, inner_r, || {
        if base_col.a > 0.10 {
            if let Some((col, intensity)) = ambient {
                let glow_scale = (base_col.a / 0.85).clamp(0.35, 1.0);
                ctx.draw_top_ambient_glow(w, h, col, intensity * glow_scale)?;
            }
        }

        if let Some((prev_mode, pw, ph)) = morph.prev.filter(|_| morph.morph_t < 0.995) {
            let t = morph.morph_t.clamp(0.0, 1.0);
            let tw = morph.target_w.max(40.0);
            let th = morph.target_h.max(24.0);
            let expanding = tw * th >= pw * ph;

            let (out_a, in_a) = morph_crossfade_weights(t, expanding);
            if out_a > 0.02 {
                let fit_s = if matches!(prev_mode, ViewMode::Collapsed) {
                    (w / pw.max(1.0)).clamp(0.88, 1.04)
                } else {
                    (w / pw.max(1.0)).clamp(0.76, 1.0)
                };
                let off_x = anchor_x(cfg.dock_position, w, pw * fit_s);
                let mut dummy_hits = HitMap::default();
                let mut prev_state = state.clone();
                prev_state.content_enter_t = 1.0;
                ctx.with_transformed_layer(off_x, 0.0, fit_s, fit_s, out_a, || {
                    render_mode_content(
                        ctx,
                        fonts,
                        pw,
                        ph,
                        prev_mode,
                        &mut prev_state,
                        snap,
                        cfg,
                        monitors,
                        active_alert,
                        &mut dummy_hits,
                    )
                })?;
            }

            if in_a > 0.02 {
                let cur_mode = state.mode.clone();
                let fit_s = if matches!(cur_mode, ViewMode::Collapsed) {
                    let s = (w / tw.max(1.0)).clamp(0.92, 1.0);
                    s * (0.95 + 0.05 * in_a)
                } else {
                    (w / tw.max(1.0)).clamp(0.78, 1.0)
                };
                let off_x = anchor_x(cfg.dock_position, w, tw * fit_s);
                state.content_enter_t = if expanding { in_a } else { 1.0 };
                ctx.with_transformed_layer(off_x, 0.0, fit_s, fit_s, in_a, || {
                    render_mode_content(
                        ctx,
                        fonts,
                        tw,
                        th,
                        &cur_mode,
                        state,
                        snap,
                        cfg,
                        monitors,
                        active_alert,
                        hit_map,
                    )
                })?;
                hit_map.transform(off_x, 0.0, fit_s);
            }
        } else {
            let cur_mode = state.mode.clone();
            state.content_enter_t = 1.0;
            render_mode_content(
                ctx,
                fonts,
                w,
                h,
                &cur_mode,
                state,
                snap,
                cfg,
                monitors,
                active_alert,
                hit_map,
            )?;
        }

        if base_col.a > 0.10 {
            if let Some(cursor) = state.hover_pos_dip {
                let accent = ambient.map(|(c, _)| c).unwrap_or(ACCENT_INFO);
                ctx.draw_cursor_specular(w, h, radius, cursor, accent)?;
            }
        }

        Ok(())
    })
}

pub(super) fn load_color(pct: f32, normal: D2D1_COLOR_F) -> D2D1_COLOR_F {
    if pct >= 85.0 {
        ACCENT_DANGER
    } else if pct >= 60.0 {
        ACCENT_WARN
    } else {
        normal
    }
}


pub(super) fn project_swatch_color(proj: &ProjectGroup, idx: usize) -> D2D1_COLOR_F {
    proj.color
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or(PROJECT_PALETTE[idx % PROJECT_PALETTE.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morph_crossfade_schedule_has_zero_simultaneous_text_overlap() {
        for expanding in [true, false] {
            for step in 0..=100 {
                let t = step as f32 / 100.0;
                let (out_a, in_a) = morph_crossfade_weights(t, expanding);
                assert!(
                    !(out_a > 0.0 && in_a > 0.0),
                    "overlapping text fade at t={t}, expanding={expanding}: out={out_a}, in={in_a}"
                );
            }
        }
    }

    #[test]
    fn status_ambient_glow_prioritizes_crash_then_warn_then_live() {
        use crate::model::{Alert, Job, JobId, ProcessKey, ProjectId};

        let empty = Snapshot::default();
        assert!(status_ambient_glow(&empty, None).is_none());

        let healthy = Snapshot {
            total_jobs: 1,
            ..Default::default()
        };
        let live_glow = status_ambient_glow(&healthy, None).unwrap();
        assert_eq!(live_glow.0.g, ACCENT_LIVE.g);

        let high_cpu = Snapshot {
            total_jobs: 1,
            total_dev_cpu: 62.0,
            ..Default::default()
        };
        let warn_glow = status_ambient_glow(&high_cpu, None).unwrap();
        assert_eq!(warn_glow.0.r, ACCENT_WARN.r);
        assert_eq!(warn_glow.0.g, ACCENT_WARN.g);

        let crash_job = Snapshot {
            total_jobs: 1,
            total_dev_cpu: 62.0,
            projects: vec![ProjectGroup {
                id: ProjectId("p".to_string()),
                name: "P".to_string(),
                root: None,
                git_branch: None,
                icon: None,
                color: None,
                pinned: false,
                status: ProjectStatus::Crashed,
                cpu: 0.0,
                ram: 0,
                ports: Vec::new(),
                jobs: vec![Job {
                    id: JobId("j".to_string()),
                    project: ProjectId("p".to_string()),
                    project_name: "P".to_string(),
                    title: "Worker".to_string(),
                    subtitle: String::new(),
                    icon: None,
                    color: None,
                    procs: Vec::new(),
                    cpu: 0.0,
                    ram: 0,
                    ports: Vec::new(),
                    uptime: 0,
                    status: ProjectStatus::Crashed,
                    last_log: None,
                    crash_summary: None,
                    root: None,
                    primary_key: ProcessKey::new(1, 1),
                    match_hint: String::new(),
                    log_path: None,
                    custom_command: None,
                    is_watched: false,
                    can_kill: false,
                    can_restart: true,
                }],
                watchdog_note: None,
                procs: Vec::new(),
                cpu_history: [0; 30],
                ram_history_mb: [0; 30],
            }],
            ..Default::default()
        };
        let crash_alert = Alert {
            id: 1,
            project_id: ProjectId("p".to_string()),
            project_name: "P".to_string(),
            kind: AlertKind::Crash {
                pid: 1,
                process_name: "node.exe".to_string(),
                exit_code: 1,
                can_restart: true,
            },
            message: "crashed".to_string(),
            timestamp_ms: 0,
            dismissed: false,
        };
        let danger_glow = status_ambient_glow(&crash_job, Some(&crash_alert)).unwrap();
        assert_eq!(danger_glow.0.r, ACCENT_DANGER.r);
        assert!(danger_glow.1 > live_glow.1);
    }
}

