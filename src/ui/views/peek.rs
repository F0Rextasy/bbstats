use super::rect;
use crate::config::AppConfig;
use crate::model::{ProjectGroup, Snapshot};
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::{format_ram, select_peek_projects};

pub(super) fn render_hover_peek(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    snap: &Snapshot,
    cfg: &AppConfig,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    let mut id_gen = 100u32;
    let mut next_id = || {
        let v = id_gen;
        id_gen += 1;
        v
    };

    hit_map.push(next_id(), rect(0.0, 0.0, w, h), HitAction::ExpandPanel);
    let lang = cfg.language;

    // Minimal macOS popover header
    ctx.draw_status_dot(18.0, 19.0, 3.8, ACCENT_LIVE, snap.total_jobs > 0)?;
    let hdr = format!(
        "{} {} · {} {}",
        snap.total_jobs,
        lang.plural(snap.total_jobs as usize, "Job", "Jobs", "İş"),
        snap.total_projects,
        lang.plural(snap.total_projects as usize, "Project", "Projects", "Proje")
    );
    ctx.draw_text(&hdr, &fonts.title, rect(30.0, 4.0, 220.0, 34.0), TEXT_PRIMARY, true)?;

    let total_str = format!(
        "CPU {:.1}%  ·  {}",
        snap.total_dev_cpu,
        format_ram(snap.total_dev_ram_bytes)
    );
    ctx.draw_text(
        &total_str,
        &fonts.mono_metric,
        rect(220.0, 4.0, w - 14.0, 34.0),
        TEXT_SECONDARY,
        true,
    )?;
    ctx.draw_section_divider(
        14.0,
        w - 14.0,
        42.0,
        lang.pick("TOP ACTIVE PROJECTS", "EN AKTİF PROJELER"),
        &fonts.section_hdr_center,
    )?;

    let sorted: Vec<&ProjectGroup> = select_peek_projects(snap);

    let total_ram_max = snap.total_dev_ram_bytes.max(1) as f32;

    if sorted.is_empty() {
        ctx.draw_text(
            lang.pick("No active background jobs detected", "Aktif arka plan işi bulunamadı"),
            &fonts.row_secondary,
            rect(16.0, 52.0, w - 16.0, 80.0),
            TEXT_MUTED,
            false,
        )?;
    } else {
        for (idx, proj) in sorted.iter().take(3).enumerate() {
            let y = 52.0 + idx as f32 * 34.0;
            let row_rect = rect(10.0, y, w - 10.0, y + 32.0);
            let swatch = super::project_swatch_color(proj, idx);
            ctx.fill_rounded(rect(16.0, y + 12.0, 24.0, y + 20.0), 2.5, swatch)?;

            let name_txt = if proj.pinned {
                format!("★ {}", proj.name)
            } else {
                proj.name.clone()
            };
            ctx.draw_text(
                &name_txt,
                &fonts.row_primary,
                rect(32.0, y, 180.0, y + 32.0),
                TEXT_PRIMARY,
                false,
            )?;

            ctx.draw_text(
                &format!(
                    "{} {}",
                    proj.jobs.len(),
                    lang.plural(proj.jobs.len(), "job", "jobs", "iş")
                ),
                &fonts.row_secondary,
                rect(184.0, y, 222.0, y + 32.0),
                TEXT_MUTED,
                true,
            )?;

            let share = (proj.ram as f32 / total_ram_max).clamp(0.06, 1.0);
            ctx.draw_progress_bar(rect(226.0, y + 13.5, 274.0, y + 18.5), share, BG_TRACK, swatch)?;

            ctx.draw_text(
                &format_ram(proj.ram),
                &fonts.mono_metric,
                rect(278.0, y, 326.0, y + 32.0),
                TEXT_PRIMARY,
                true,
            )?;

            ctx.draw_text(
                &format!("{:.1}%", proj.cpu),
                &fonts.mono_metric,
                rect(330.0, y, w - 14.0, y + 32.0),
                if proj.cpu > 75.0 { ACCENT_WARN } else { TEXT_SECONDARY },
                true,
            )?;

            hit_map.push(
                next_id(),
                row_rect,
                HitAction::OpenProjectDetail(proj.id.clone()),
            );
        }
    }

    let fy = h - 24.0;
    ctx.draw_hairline(14.0, fy, w - 14.0)?;
    ctx.draw_text(
        lang.pick("Click for details", "Detaylar için tıkla"),
        &fonts.row_secondary,
        rect(16.0, fy + 2.0, 240.0, h - 2.0),
        TEXT_MUTED,
        false,
    )?;
    ctx.draw_text(
        cfg.hotkey.label(lang),
        &fonts.mono_badge,
        rect(w - 150.0, fy + 3.0, w - 14.0, h - 3.0),
        TEXT_SECONDARY,
        false,
    )?;
    Ok(())
}

