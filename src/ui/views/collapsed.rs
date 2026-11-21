use super::{load_color, project_swatch_color, rect};
use crate::config::{AppConfig, DockPosition};
use crate::model::{AlertKind, Language, ProjectStatus, Snapshot};
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::format_ram;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_collapsed(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    snap: &Snapshot,
    cfg: &AppConfig,
    active_alert: Option<&crate::model::Alert>,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    let mut id_gen = 1u32;
    let mut next_id = || {
        let v = id_gen;
        id_gen += 1;
        v
    };

    hit_map.push(next_id(), rect(0.0, 0.0, w, h), HitAction::ExpandPanel);

    let shadow = cfg.idle_opacity < 0.45;
    let crashed_count = snap
        .projects
        .iter()
        .flat_map(|p| p.jobs.iter())
        .filter(|j| j.status == ProjectStatus::Crashed)
        .count();

    if cfg.dock_position == DockPosition::RightEdge && w <= 72.0 {
        return render_collapsed_vertical_rail(
            ctx,
            fonts,
            w,
            h,
            snap,
            cfg.language,
            active_alert,
            crashed_count,
            shadow,
            hit_map,
            &mut next_id,
        );
    }

    if let Some(alert) = active_alert {
        let (dot_col, icon_str) = match alert.kind {
            AlertKind::Crash { .. } => (ACCENT_DANGER, "!"),
            _ => (ACCENT_WARN, "^"),
        };
        ctx.draw_status_dot(16.0, h * 0.5, 4.0, dot_col, true)?;
        ctx.draw_text(
            icon_str,
            &fonts.mono_badge,
            rect(11.0, 0.0, 21.0, h),
            BG_OVERLAY,
            false,
        )?;
        let can_restart = matches!(alert.kind, AlertKind::Crash { can_restart: true, .. });
        let msg = format!("{} · {}", alert.project_name, alert.message);
        ctx.draw_text(
            &msg,
            &fonts.pill_label,
            rect(28.0, 0.0, if can_restart { w - 72.0 } else { w - 34.0 }, h),
            TEXT_PRIMARY,
            true,
        )?;

        if can_restart {
            let r_btn = rect(w - 66.0, 5.0, w - 34.0, h - 5.0);
            ctx.fill_rounded(r_btn, 6.0, ACCENT_DANGER_BG)?;
            ctx.draw_text("↻", &fonts.mono_badge, r_btn, TEXT_PRIMARY, false)?;
            hit_map.push(
                next_id(),
                r_btn,
                HitAction::RestartAlertProject(alert.project_id.clone(), alert.id),
            );
        }

        let x_btn = rect(w - 28.0, 5.0, w - 8.0, h - 5.0);
        ctx.fill_rounded(x_btn, 6.0, BG_ELEVATED)?;
        ctx.draw_text("×", &fonts.mono_badge, x_btn, TEXT_SECONDARY, false)?;
        hit_map.push(next_id(), x_btn, HitAction::DismissAlert(alert.id));
        return Ok(());
    }

    // TopCenter exelban/stats modular strip: pure stats (zero wordy clutter)
    let dot_col = if crashed_count > 0 {
        ACCENT_DANGER
    } else if snap.projects.iter().any(|p| p.status == ProjectStatus::HighLoad) {
        ACCENT_WARN
    } else if snap.total_jobs > 0 {
        ACCENT_LIVE
    } else {
        TEXT_MUTED
    };

    // Module 1: Status dot + active job count + top project swatches
    ctx.draw_status_dot(15.0, h * 0.5, 3.6, dot_col, snap.total_jobs > 0)?;
    ctx.draw_text_hud(
        &format!("{}", snap.total_jobs),
        &fonts.pill_mono,
        rect(22.0, 0.0, 40.0, h),
        if crashed_count > 0 { ACCENT_DANGER } else { TEXT_PRIMARY },
        true,
        shadow,
    )?;

    for (idx, proj) in snap
        .projects
        .iter()
        .filter(|p| p.jobs.iter().any(|j| j.status != ProjectStatus::Stopped))
        .take(3)
        .enumerate()
    {
        let sx = 41.0 + idx as f32 * 6.5;
        let col = project_swatch_color(proj, idx);
        ctx.fill_rounded(rect(sx, h * 0.5 - 2.5, sx + 4.5, h * 0.5 + 2.5), 1.5, col)?;
    }

    ctx.draw_vline(65.0, 7.0, h - 7.0)?;

    // Module 2: exelban/stats mini CPU bar chart + tabular %
    let cpu_col = load_color(snap.total_dev_cpu, ACCENT_INFO);
    ctx.draw_mini_bars(
        rect(72.0, 7.0, 114.0, h - 7.0),
        &snap.system_cpu_history[14..24],
        100.0,
        cpu_col,
        rgba(255, 255, 255, if shadow { 0.16 } else { 0.08 }),
    )?;
    let cpu_disp = if snap.total_dev_cpu > 0.05 {
        snap.total_dev_cpu
    } else {
        snap.system_cpu
    };
    ctx.draw_text_hud(
        &format!("{:.0}%", cpu_disp),
        &fonts.pill_mono,
        rect(118.0, 0.0, 154.0, h),
        TEXT_PRIMARY,
        true,
        shadow,
    )?;

    ctx.draw_vline(156.0, 7.0, h - 7.0)?;

    // Module 3: exelban/stats RAM ring gauge + usage
    let ram_denom = (snap.system_ram_total_bytes as f32 * 0.20).max(512.0 * 1024.0 * 1024.0);
    let ram_bytes = if snap.total_dev_ram_bytes > 0 {
        snap.total_dev_ram_bytes
    } else {
        snap.system_ram_used_bytes
    };
    let ram_ratio = if snap.total_dev_ram_bytes > 0 {
        (snap.total_dev_ram_bytes as f32 / ram_denom).clamp(0.08, 1.0)
    } else if snap.system_ram_total_bytes > 0 {
        (snap.system_ram_used_bytes as f32 / snap.system_ram_total_bytes as f32).clamp(0.08, 1.0)
    } else {
        0.1
    };
    ctx.draw_ring_gauge(
        170.0,
        h * 0.5,
        6.8,
        2.3,
        ram_ratio,
        rgba(255, 255, 255, if shadow { 0.22 } else { 0.12 }),
        ACCENT_RAM,
    )?;
    ctx.draw_text_hud(
        &format_ram(ram_bytes),
        &fonts.pill_mono,
        rect(182.0, 0.0, 232.0, h),
        TEXT_PRIMARY,
        true,
        shadow,
    )?;

    ctx.draw_vline(234.0, 7.0, h - 7.0)?;

    // Module 4: System CPU mini readout
    ctx.draw_text_hud(
        &format!("S:{:.0}%", snap.system_cpu),
        &fonts.row_secondary,
        rect(240.0, 0.0, w - 8.0, h),
        TEXT_SECONDARY,
        true,
        shadow,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_collapsed_vertical_rail<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    snap: &Snapshot,
    lang: Language,
    active_alert: Option<&crate::model::Alert>,
    crashed_count: usize,
    shadow: bool,
    hit_map: &mut HitMap,
    next_id: &mut F,
) -> windows::core::Result<()> {
    let dot_col = if crashed_count > 0 || active_alert.is_some() {
        ACCENT_DANGER
    } else if snap.projects.iter().any(|p| p.status == ProjectStatus::HighLoad) {
        ACCENT_WARN
    } else if snap.total_jobs > 0 {
        ACCENT_LIVE
    } else {
        TEXT_MUTED
    };

    // Top: live status orb + active job count + project swatches
    ctx.draw_status_dot(w * 0.5, 16.0, 4.0, dot_col, snap.total_jobs > 0 || active_alert.is_some())?;
    ctx.draw_text_hud(
        &format!("{}", snap.total_jobs),
        &fonts.title_center,
        rect(2.0, 24.0, w - 2.0, 42.0),
        if crashed_count > 0 { ACCENT_DANGER } else { TEXT_PRIMARY },
        true,
        shadow,
    )?;
    ctx.draw_text_hud(
        lang.pick("JOBS", "İŞ"),
        &fonts.micro_lbl,
        rect(2.0, 40.0, w - 2.0, 51.0),
        TEXT_MUTED,
        false,
        shadow,
    )?;

    let mut y = 58.0;
    let max_swatches = if h > 520.0 { 8 } else if h > 400.0 { 5 } else { 3 };
    let max_proj_ram = snap
        .projects
        .iter()
        .map(|p| p.ram)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    for (idx, proj) in snap
        .projects
        .iter()
        .filter(|p| !p.jobs.is_empty())
        .take(max_swatches)
        .enumerate()
    {
        let col = if proj.status == ProjectStatus::Crashed {
            ACCENT_DANGER
        } else if proj.status == ProjectStatus::Stopped {
            TEXT_MUTED
        } else {
            project_swatch_color(proj, idx)
        };
        let bar_w = if h > 520.0 {
            let ratio = (proj.ram as f32 / max_proj_ram).clamp(0.28, 1.0);
            24.0 * ratio
        } else {
            12.0
        };
        let x0 = (w - bar_w) * 0.5;
        ctx.fill_rounded(
            rect(8.0, y, w - 8.0, y + 5.0),
            2.5,
            rgba(255, 255, 255, if shadow { 0.14 } else { 0.06 }),
        )?;
        ctx.fill_rounded(rect(x0, y, x0 + bar_w, y + 5.0), 2.5, col)?;
        y += 9.5;
    }

    // Center telemetry modules (exelban/stats vertical stack)
    let mid_top = (h * 0.5 - 88.0).max(y + 12.0);
    if mid_top + 115.0 < h - 48.0 {
        ctx.draw_hairline(8.0, mid_top, w - 8.0)?;
        let mut my = mid_top + 8.0;

        // CPU module: micro label + mini bar history + percentage
        ctx.draw_text_hud(
            "CPU",
            &fonts.micro_lbl,
            rect(2.0, my, w - 2.0, my + 11.0),
            TEXT_MUTED,
            false,
            shadow,
        )?;
        my += 12.0;
        let cpu_disp = if snap.total_dev_cpu > 0.05 {
            snap.total_dev_cpu
        } else {
            snap.system_cpu
        };
        ctx.draw_mini_bars(
            rect(7.0, my, w - 7.0, my + 22.0),
            &snap.system_cpu_history[16..24],
            100.0,
            load_color(cpu_disp, ACCENT_INFO),
            rgba(255, 255, 255, if shadow { 0.16 } else { 0.08 }),
        )?;
        my += 24.0;
        ctx.draw_text_hud(
            &format!("{:.0}%", cpu_disp),
            &fonts.caption_center,
            rect(2.0, my, w - 2.0, my + 16.0),
            TEXT_PRIMARY,
            true,
            shadow,
        )?;
        my += 22.0;

        // RAM module: micro label + vertical gauge bar + formatted RAM
        ctx.draw_text_hud(
            "RAM",
            &fonts.micro_lbl,
            rect(2.0, my, w - 2.0, my + 11.0),
            TEXT_MUTED,
            false,
            shadow,
        )?;
        my += 13.0;
        let ram_bytes = if snap.total_dev_ram_bytes > 0 {
            snap.total_dev_ram_bytes
        } else {
            snap.system_ram_used_bytes
        };
        let ram_denom = (snap.system_ram_total_bytes as f32 * 0.25).max(512.0 * 1024.0 * 1024.0);
        let ram_ratio = if snap.total_dev_ram_bytes > 0 {
            (snap.total_dev_ram_bytes as f32 / ram_denom).clamp(0.08, 1.0)
        } else if snap.system_ram_total_bytes > 0 {
            (snap.system_ram_used_bytes as f32 / snap.system_ram_total_bytes as f32).clamp(0.08, 1.0)
        } else {
            0.1
        };
        ctx.draw_vertical_bar(
            rect(w * 0.5 - 4.0, my, w * 0.5 + 4.0, my + 34.0),
            ram_ratio,
            rgba(255, 255, 255, if shadow { 0.18 } else { 0.10 }),
            ACCENT_RAM,
        )?;
        my += 36.0;
        ctx.draw_text_hud(
            &format_ram(ram_bytes),
            &fonts.caption_center,
            rect(1.0, my, w - 1.0, my + 16.0),
            TEXT_PRIMARY,
            true,
            shadow,
        )?;
        my += 22.0;

        // Optional System CPU/RAM dual bar module if vertical space allows
        if my + 48.0 < h - 56.0 {
            ctx.draw_text_hud(
                "SYS",
                &fonts.micro_lbl,
                rect(2.0, my, w - 2.0, my + 11.0),
                TEXT_MUTED,
                false,
                shadow,
            )?;
            my += 13.0;
            let sys_ram_ratio = if snap.system_ram_total_bytes > 0 {
                snap.system_ram_used_bytes as f32 / snap.system_ram_total_bytes as f32
            } else {
                0.2
            };
            ctx.draw_vertical_bar(
                rect(w * 0.5 - 7.0, my, w * 0.5 - 2.0, my + 24.0),
                (snap.system_cpu / 100.0).clamp(0.06, 1.0),
                rgba(255, 255, 255, 0.12),
                ACCENT_INFO,
            )?;
            ctx.draw_vertical_bar(
                rect(w * 0.5 + 2.0, my, w * 0.5 + 7.0, my + 24.0),
                sys_ram_ratio.clamp(0.06, 1.0),
                rgba(255, 255, 255, 0.12),
                ACCENT_TEAL,
            )?;
            my += 26.0;
            ctx.draw_text_hud(
                &format!("{:.0}%", snap.system_cpu),
                &fonts.micro_lbl,
                rect(2.0, my, w - 2.0, my + 12.0),
                TEXT_SECONDARY,
                true,
                shadow,
            )?;
        }
    }

    // Bottom expand chevron & quick settings icon
    let exp_r = rect(6.0, h - 52.0, w - 6.0, h - 30.0);
    ctx.draw_text_hud("‹", &fonts.title_center, exp_r, TEXT_SECONDARY, false, shadow)?;

    let cfg_r = rect(6.0, h - 28.0, w - 6.0, h - 6.0);
    ctx.draw_text_hud("⚙", &fonts.mono_badge, cfg_r, TEXT_MUTED, false, shadow)?;
    hit_map.push(next_id(), cfg_r, HitAction::OpenSettings);

    Ok(())
}

