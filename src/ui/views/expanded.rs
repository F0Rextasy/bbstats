use super::{project_swatch_color, rect, status_color};
use crate::config::AppConfig;
use crate::model::{fold_search_lower, AlertKind, Job, Language, ProjectStatus, Snapshot};
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::{filter_project_jobs, format_ram, format_uptime, PendingKill, UiState};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_expanded(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    state: &mut UiState,
    snap: &Snapshot,
    cfg: &AppConfig,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    let mut id_gen = 200u32;
    let mut next_id = || {
        let v = id_gen;
        id_gen += 1;
        v
    };

    let lang = cfg.language;

    // 1. Minimal macOS Popover Top Header (y = 6..36, borderless controls)
    ctx.draw_status_dot(18.0, 21.0, 3.8, ACCENT_LIVE, snap.total_jobs > 0)?;
    ctx.draw_text(
        "bbstats",
        &fonts.row_secondary,
        rect(28.0, 6.0, 82.0, 36.0),
        TEXT_MUTED,
        false,
    )?;

    // Sort mode pill (clickable cycle: Custom -> CPU -> RAM -> Name -> Time)
    let sort_rect = rect(84.0, 10.0, 154.0, 32.0);
    let sort_id = next_id();
    ctx.fill_rounded(
        sort_rect,
        6.0,
        if state.hovered_hit_id == Some(sort_id) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        &format!("⇅ {}", cfg.sort_mode.label_for(lang)),
        &fonts.row_secondary,
        sort_rect,
        TEXT_SECONDARY,
        false,
    )?;
    hit_map.push(sort_id, sort_rect, HitAction::CycleSortMode);

    ctx.draw_text(
        lang.pick("Background Jobs", "Arka Plan İşleri"),
        &fonts.title_center,
        rect(152.0, 6.0, w - 152.0, 36.0),
        TEXT_PRIMARY,
        false,
    )?;

    // Borderless top-right toolbar icons: + Ekle, ≡ Tüm Süreçler, ★ Sabitle, ⚙ Ayarlar, ✕
    let add_rect = rect(w - 146.0, 9.0, w - 122.0, 33.0);
    let add_id = next_id();
    if state.add_job_draft.is_some() || state.hovered_hit_id == Some(add_id) {
        ctx.fill_rounded(
            add_rect,
            6.0,
            if state.add_job_draft.is_some() {
                ACCENT_INFO_BG
            } else {
                BG_ELEVATED_HOVER
            },
        )?;
    }
    ctx.draw_text(
        "+",
        &fonts.mono_badge,
        add_rect,
        if state.add_job_draft.is_some() {
            ACCENT_INFO
        } else {
            TEXT_SECONDARY
        },
        false,
    )?;
    hit_map.push(add_id, add_rect, HitAction::BeginAddJob);

    let all_rect = rect(w - 118.0, 9.0, w - 94.0, 33.0);
    let all_id = next_id();
    if state.show_all_processes || state.hovered_hit_id == Some(all_id) {
        ctx.fill_rounded(
            all_rect,
            6.0,
            if state.show_all_processes {
                ACCENT_INFO_BG
            } else {
                BG_ELEVATED_HOVER
            },
        )?;
    }
    ctx.draw_text(
        "≡",
        &fonts.mono_badge,
        all_rect,
        if state.show_all_processes {
            ACCENT_INFO
        } else {
            TEXT_SECONDARY
        },
        false,
    )?;
    hit_map.push(all_id, all_rect, HitAction::ToggleShowAllProcesses);

    let pin_rect = rect(w - 90.0, 9.0, w - 66.0, 33.0);
    let pin_id = next_id();
    if state.sticky || state.hovered_hit_id == Some(pin_id) {
        ctx.fill_rounded(
            pin_rect,
            6.0,
            if state.sticky {
                ACCENT_INFO_BG
            } else {
                BG_ELEVATED_HOVER
            },
        )?;
    }
    ctx.draw_text(
        if state.sticky { "★" } else { "☆" },
        &fonts.mono_badge,
        pin_rect,
        if state.sticky { ACCENT_INFO } else { TEXT_SECONDARY },
        false,
    )?;
    hit_map.push(pin_id, pin_rect, HitAction::ToggleSticky);

    let cfg_rect = rect(w - 62.0, 9.0, w - 38.0, 33.0);
    let cfg_id = next_id();
    if state.hovered_hit_id == Some(cfg_id) {
        ctx.fill_rounded(cfg_rect, 6.0, BG_ELEVATED_HOVER)?;
    }
    ctx.draw_text("⚙", &fonts.mono_badge, cfg_rect, TEXT_SECONDARY, false)?;
    hit_map.push(cfg_id, cfg_rect, HitAction::OpenSettings);

    let cls_rect = rect(w - 34.0, 9.0, w - 12.0, 33.0);
    let cls_id = next_id();
    if state.hovered_hit_id == Some(cls_id) {
        ctx.fill_rounded(cls_rect, 6.0, BG_ELEVATED_HOVER)?;
    }
    ctx.draw_text("✕", &fonts.mono_badge, cls_rect, TEXT_SECONDARY, false)?;
    hit_map.push(cls_id, cls_rect, HitAction::CollapsePanel);

    // 2. Centered 3 Hero Ring Gauges (exelban/stats CPU/GPU/RAM header style, y = 40..124)
    let col_w = (w - 32.0) / 3.0;
    let cx1 = 16.0 + col_w * 0.5;
    let cx2 = 16.0 + col_w * 1.5;
    let cx3 = 16.0 + col_w * 2.5;
    let cy = 70.0;
    let ring_r = 25.0;

    // Gauge 1: CPU
    let cpu_ratio = (snap.total_dev_cpu / 100.0).clamp(0.04, 1.0);
    ctx.draw_ring_gauge(cx1, cy, ring_r, 4.6, cpu_ratio, BG_TRACK, ACCENT_INFO)?;
    ctx.draw_text(
        &format!("{:.1}%", snap.total_dev_cpu),
        &fonts.gauge_val,
        rect(cx1 - 24.0, cy - 12.0, cx1 + 24.0, cy + 5.0),
        TEXT_PRIMARY,
        true,
    )?;
    ctx.draw_text(
        "CPU",
        &fonts.gauge_lbl,
        rect(cx1 - 22.0, cy + 4.0, cx1 + 22.0, cy + 16.0),
        TEXT_MUTED,
        false,
    )?;
    ctx.draw_text(
        &format!("{} {:.0}%", lang.pick("System", "Sistem"), snap.system_cpu),
        &fonts.caption_center,
        rect(cx1 - 56.0, 102.0, cx1 + 56.0, 120.0),
        TEXT_SECONDARY,
        true,
    )?;

    // Gauge 2: RAM
    let sys_total = snap.system_ram_total_bytes.max(1);
    let ram_ratio =
        (snap.total_dev_ram_bytes as f32 / (sys_total as f32 * 0.25).max(1.0)).clamp(0.06, 1.0);
    ctx.draw_ring_gauge(cx2, cy, ring_r, 4.6, ram_ratio, BG_TRACK, ACCENT_RAM)?;
    ctx.draw_text(
        &format_ram(snap.total_dev_ram_bytes),
        &fonts.gauge_val,
        rect(cx2 - 24.0, cy - 12.0, cx2 + 24.0, cy + 5.0),
        TEXT_PRIMARY,
        true,
    )?;
    ctx.draw_text(
        "RAM",
        &fonts.gauge_lbl,
        rect(cx2 - 22.0, cy + 4.0, cx2 + 22.0, cy + 16.0),
        TEXT_MUTED,
        false,
    )?;
    ctx.draw_text(
        &format!("{} {}", snap.total_procs, lang.pick("procs", "süreç")),
        &fonts.caption_center,
        rect(cx2 - 56.0, 102.0, cx2 + 56.0, 120.0),
        TEXT_SECONDARY,
        true,
    )?;

    // Gauge 3: Active Jobs
    let jobs_ratio = (snap.total_jobs as f32 / 20.0).clamp(0.12, 1.0);
    ctx.draw_ring_gauge(cx3, cy, ring_r, 4.6, jobs_ratio, BG_TRACK, ACCENT_LIVE)?;
    ctx.draw_text(
        &format!("{}", snap.total_jobs),
        &fonts.gauge_val,
        rect(cx3 - 24.0, cy - 12.0, cx3 + 24.0, cy + 5.0),
        TEXT_PRIMARY,
        true,
    )?;
    ctx.draw_text(
        lang.pick("JOBS", "İŞ"),
        &fonts.gauge_lbl,
        rect(cx3 - 22.0, cy + 4.0, cx3 + 22.0, cy + 16.0),
        TEXT_MUTED,
        false,
    )?;
    ctx.draw_text(
        &format!("{} {}", snap.total_projects, lang.pick("projects", "proje")),
        &fonts.caption_center,
        rect(cx3 - 56.0, 102.0, cx3 + 56.0, 120.0),
        TEXT_SECONDARY,
        true,
    )?;

    // 3. `─── USAGE HISTORY ───` Area Chart + Project RAM Breakdown Bar (y = 126..200)
    ctx.draw_section_divider(
        16.0,
        w - 16.0,
        130.0,
        lang.pick("USAGE HISTORY", "KULLANIM GEÇMİŞİ"),
        &fonts.section_hdr_center,
    )?;

    let chart_rect = rect(16.0, 141.0, w - 16.0, 187.0);
    ctx.fill_rounded(chart_rect, 6.0, BG_WELL)?;

    // Aggregate RAM/CPU history across projects with smooth initial baseline backfill
    let mut ram_series = [0.0f32; 30];
    for p in &snap.projects {
        for (i, &mb) in p.ram_history_mb.iter().enumerate() {
            ram_series[i] += mb as f32;
        }
    }
    if let Some(first_nz) = ram_series.iter().position(|&v| v > 0.0) {
        let base_val = ram_series[first_nz];
        for v in ram_series.iter_mut().take(first_nz) {
            *v = base_val;
        }
    }
    let ram_peak = ram_series.iter().copied().fold(0.0f32, f32::max);
    ctx.draw_sparkline(
        rect(chart_rect.left + 2.0, chart_rect.top + 4.0, chart_rect.right - 2.0, chart_rect.bottom - 2.0),
        &ram_series,
        (ram_peak * 1.25).max(128.0),
        ACCENT_INFO,
    )?;

    // Segmented RAM share bar across projects (like exelban/stats Memory bar)
    let segments: Vec<(f32, D2D1_COLOR_F)> = snap
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| p.ram > 0)
        .map(|(idx, p)| (p.ram as f32, project_swatch_color(p, idx)))
        .collect();
    ctx.draw_segmented_bar(rect(16.0, 192.0, w - 16.0, 198.0), &segments, BG_TRACK)?;

    // 4. Clean subtle search bar (y = 206..234)
    let search_rect = rect(16.0, 206.0, w - 16.0, 234.0);
    ctx.fill_rounded(search_rect, 7.0, BG_ELEVATED)?;
    if state.search_focused && state.rename_draft.is_none() && state.add_job_draft.is_none() {
        ctx.stroke_rounded(search_rect, 7.0, BORDER_FOCUS, 1.3)?;
    }
    let search_txt = if state.search_query.is_empty() {
        lang.pick("Search job, project or :port...", "İş, proje veya :port ara...")
    } else {
        &state.search_query
    };
    ctx.draw_text(
        search_txt,
        &fonts.row_secondary,
        rect(26.0, 206.0, w - 42.0, 234.0),
        if state.search_query.is_empty() {
            TEXT_MUTED
        } else {
            TEXT_PRIMARY
        },
        true,
    )?;
    hit_map.push(next_id(), search_rect, HitAction::FocusSearch);

    if !state.search_query.is_empty() {
        let clr = rect(w - 38.0, 208.0, w - 18.0, 232.0);
        ctx.draw_text("×", &fonts.mono_badge, clr, TEXT_SECONDARY, false)?;
        hit_map.push(next_id(), clr, HitAction::ClearSearch);
    }

    ctx.draw_section_divider(
        16.0,
        w - 16.0,
        245.0,
        lang.pick("PROJECTS & JOBS", "PROJELER VE İŞLER"),
        &fonts.section_hdr_center,
    )?;

    let mut list_top = 254.0;

    // 2. Inline Rename Bar (writes %APPDATA%\bbstats\labels.toml or project alias on Save)
    if let Some(ref draft) = state.rename_draft {
        let bar = rect(12.0, list_top, w - 12.0, list_top + 36.0);
        ctx.fill_rounded(bar, 8.0, BG_OVERLAY)?;
        ctx.stroke_rounded(bar, 8.0, BORDER_FOCUS, 1.2)?;

        ctx.draw_text(
            if draft.project_id.is_some() {
                lang.pick("Project:", "Proje:")
            } else {
                lang.pick("Name:", "Ad:")
            },
            &fonts.mono_badge,
            rect(18.0, list_top, 68.0, list_top + 36.0),
            ACCENT_INFO,
            false,
        )?;

        let input_box = rect(70.0, list_top + 5.0, w - 134.0, list_top + 31.0);
        ctx.fill_rounded(input_box, 5.0, BG_ELEVATED)?;
        ctx.draw_text(
            &format!("{}|", draft.input),
            &fonts.row_primary,
            rect(78.0, list_top + 5.0, w - 140.0, list_top + 31.0),
            TEXT_PRIMARY,
            false,
        )?;

        let save_btn = rect(w - 128.0, list_top + 5.0, w - 72.0, list_top + 31.0);
        ctx.fill_rounded(save_btn, 6.0, ACCENT_INFO_BG)?;
        ctx.draw_text(
            lang.pick("Save ↵", "Kaydet ↵"),
            &fonts.mono_badge,
            save_btn,
            ACCENT_INFO,
            false,
        )?;
        hit_map.push(next_id(), save_btn, HitAction::SaveRename);

        let can_btn = rect(w - 68.0, list_top + 5.0, w - 18.0, list_top + 31.0);
        ctx.fill_rounded(can_btn, 6.0, BG_ELEVATED)?;
        ctx.draw_text(
            lang.pick("Cancel", "İptal"),
            &fonts.mono_badge,
            can_btn,
            TEXT_SECONDARY,
            false,
        )?;
        hit_map.push(next_id(), can_btn, HitAction::CancelRename);

        list_top += 42.0;
    }

    // 2b. Inline Add Custom Job Card (+ Add)
    if let Some(ref add) = state.add_job_draft {
        let box_r = rect(12.0, list_top, w - 12.0, list_top + 96.0);
        ctx.fill_rounded(box_r, 8.0, BG_OVERLAY)?;
        ctx.stroke_rounded(box_r, 8.0, BORDER_FOCUS, 1.2)?;

        let fields: [(&str, &str, &str, u8, f32); 3] = [
            (
                lang.pick("Name", "Ad"),
                &add.title,
                lang.pick("Ex: API Server", "Örn: API Sunucu"),
                0,
                list_top + 6.0,
            ),
            (
                lang.pick("Cmd/Match", "Komut/Eş"),
                &add.command_or_match,
                lang.pick("Ex: python bot.py or redis.exe", "Örn: python bot.py veya redis.exe"),
                1,
                list_top + 36.0,
            ),
            (
                "Log",
                &add.log_file,
                lang.pick("Optional: C:\\logs\\app.log", "Opsiyonel: C:\\logs\\app.log"),
                2,
                list_top + 66.0,
            ),
        ];

        for (lbl, val, ph, idx, fy) in fields {
            ctx.draw_text(
                lbl,
                &fonts.row_secondary,
                rect(20.0, fy, 80.0, fy + 24.0),
                TEXT_SECONDARY,
                false,
            )?;
            let right_edge = if idx == 0 { w - 134.0 } else { w - 20.0 };
            let f_rect = rect(82.0, fy, right_edge, fy + 24.0);
            ctx.fill_rounded(f_rect, 5.0, BG_ELEVATED)?;
            if add.active_field == idx {
                ctx.stroke_rounded(f_rect, 5.0, BORDER_FOCUS, 1.0)?;
            }
            let shown = if add.active_field == idx {
                format!("{val}|")
            } else if val.is_empty() {
                ph.to_string()
            } else {
                val.to_string()
            };
            ctx.draw_text(
                &shown,
                &fonts.row_secondary,
                rect(88.0, fy, right_edge - 6.0, fy + 24.0),
                if val.is_empty() && add.active_field != idx {
                    TEXT_MUTED
                } else {
                    TEXT_PRIMARY
                },
                false,
            )?;
            hit_map.push(next_id(), f_rect, HitAction::FocusAddJobField(idx));
        }

        let save_btn = rect(w - 128.0, list_top + 6.0, w - 72.0, list_top + 30.0);
        ctx.fill_rounded(save_btn, 6.0, ACCENT_INFO_BG)?;
        ctx.draw_text(
            lang.pick("Add ↵", "Ekle ↵"),
            &fonts.mono_badge,
            save_btn,
            ACCENT_INFO,
            false,
        )?;
        hit_map.push(next_id(), save_btn, HitAction::SaveAddJob);

        let can_btn = rect(w - 68.0, list_top + 6.0, w - 18.0, list_top + 30.0);
        ctx.fill_rounded(can_btn, 6.0, BG_ELEVATED)?;
        ctx.draw_text(
            lang.pick("Cancel", "İptal"),
            &fonts.mono_badge,
            can_btn,
            TEXT_SECONDARY,
            false,
        )?;
        hit_map.push(next_id(), can_btn, HitAction::CancelAddJob);

        list_top += 104.0;
    }

    // 3. Alert Banner (if active)
    if let Some(alert) = snap.alerts.iter().find(|a| !a.dismissed) {
        let banner = rect(12.0, list_top, w - 12.0, list_top + 44.0);
        let (bg_a, fg_a) = match alert.kind {
            AlertKind::Crash { .. } => (ACCENT_DANGER_BG, ACCENT_DANGER),
            _ => (ACCENT_WARN_BG, ACCENT_WARN),
        };
        ctx.fill_rounded(banner, 8.0, bg_a)?;
        ctx.stroke_rounded(banner, 8.0, fg_a, 1.0)?;

        let title = format!("! {} — {}", alert.project_name, alert.message);
        ctx.draw_text(
            &title,
            &fonts.row_primary,
            rect(22.0, list_top + 4.0, w - 126.0, list_top + 24.0),
            TEXT_PRIMARY,
            true,
        )?;
        ctx.draw_text(
            lang.pick(
                "Click Restart to relaunch or dismiss",
                "Yeniden başlatmak için Restart'a tıklayın veya kapatın",
            ),
            &fonts.row_secondary,
            rect(22.0, list_top + 22.0, w - 126.0, list_top + 40.0),
            TEXT_SECONDARY,
            false,
        )?;

        if matches!(alert.kind, AlertKind::Crash { can_restart: true, .. }) {
            let r_btn = rect(w - 118.0, list_top + 8.0, w - 50.0, list_top + 36.0);
            ctx.fill_rounded(r_btn, 6.0, BG_OVERLAY)?;
            ctx.draw_text("↻ Restart", &fonts.mono_badge, r_btn, TEXT_PRIMARY, false)?;
            hit_map.push(
                next_id(),
                r_btn,
                HitAction::RestartAlertProject(alert.project_id.clone(), alert.id),
            );
        }

        let d_btn = rect(w - 44.0, list_top + 8.0, w - 20.0, list_top + 36.0);
        ctx.fill_rounded(d_btn, 6.0, BG_OVERLAY)?;
        ctx.draw_text("×", &fonts.mono_badge, d_btn, TEXT_SECONDARY, false)?;
        hit_map.push(next_id(), d_btn, HitAction::DismissAlert(alert.id));

        list_top += 50.0;
    }

    // 4. Main Content Area (Project Cards -> Jobs OR secondary All Processes view)
    let list_bot = h - 30.0;
    let viewport_h = (list_bot - list_top).max(40.0);
    hit_map.scroll_viewport = Some(rect(0.0, list_top, w, list_bot));

    if state.show_all_processes {
        render_all_procs_list(ctx, fonts, w, list_top, list_bot, viewport_h, lang, state, snap, hit_map, &mut next_id)?;
    } else {
        render_project_job_cards(ctx, fonts, w, list_top, list_bot, viewport_h, lang, state, snap, hit_map, &mut next_id)?;
    }

    if let Some((ref toast_msg, ts)) = state.toast {
        if ts.elapsed().as_secs() < 5 {
            let t_rect = rect(16.0, list_bot - 30.0, w - 16.0, list_bot - 5.0);
            ctx.fill_rounded(t_rect, 6.0, BG_OVERLAY)?;
            ctx.stroke_rounded(t_rect, 6.0, ACCENT_WARN, 1.0)?;
            ctx.draw_text(
                toast_msg,
                &fonts.row_secondary,
                rect(24.0, list_bot - 30.0, w - 24.0, list_bot - 5.0),
                ACCENT_WARN,
                false,
            )?;
        }
    }

    // 5. Bottom Status Bar (24 DIP)
    ctx.draw_hairline(16.0, list_bot, w - 16.0)?;
    let status_left = format!(
        "{} {} · {} {} · {:.1}% CPU · {}",
        snap.total_jobs,
        lang.pick("jobs", "iş"),
        snap.total_projects,
        lang.pick("projects", "proje"),
        snap.total_dev_cpu,
        format_ram(snap.total_dev_ram_bytes)
    );
    ctx.draw_text(
        &status_left,
        &fonts.row_secondary,
        rect(16.0, list_bot + 3.0, 250.0, h - 2.0),
        TEXT_SECONDARY,
        true,
    )?;
    ctx.draw_text(
        lang.pick(
            "Click: Terminal · Right-click: Rename · Esc: Collapse",
            "Tıkla: Terminal · Sağ tık: Ad · Esc: Daralt",
        ),
        &fonts.row_secondary,
        rect(252.0, list_bot + 3.0, w - 16.0, h - 2.0),
        TEXT_MUTED,
        false,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_project_job_cards<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    list_top: f32,
    list_bot: f32,
    viewport_h: f32,
    lang: Language,
    state: &mut UiState,
    snap: &Snapshot,
    hit_map: &mut HitMap,
    next_id: &mut F,
) -> windows::core::Result<()> {
    let groups = filter_project_jobs(snap, &state.search_query);
    let total_jobs_shown: usize = groups.iter().map(|(_, j)| j.len()).sum();
    if total_jobs_shown > 0 && state.selected_index >= total_jobs_shown {
        state.selected_index = total_jobs_shown - 1;
    }

    let total_content_h: f32 = groups
        .iter()
        .map(|(p, jobs)| {
            let wd = if p.watchdog_note.is_some() { 20.0 } else { 0.0 };
            30.0 + wd + (jobs.len().max(1) as f32) * 40.0 + 10.0
        })
        .sum::<f32>()
        + 6.0;
    state.max_scroll = (total_content_h - viewport_h).max(0.0);
    state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_scroll);

    if groups.is_empty() {
        ctx.draw_text(
            if state.search_query.is_empty() {
                lang.pick(
                    "No running background jobs detected yet.",
                    "Henüz çalışan bir arka plan otomasyonu tespit edilmedi.",
                )
            } else {
                lang.pick(
                    "No jobs or projects matched your search.",
                    "Aramanızla eşleşen iş veya proje bulunamadı.",
                )
            },
            &fonts.row_secondary,
            rect(20.0, list_top + 16.0, w - 20.0, list_top + 50.0),
            TEXT_MUTED,
            false,
        )?;
        return Ok(());
    }

    let enter_t = state.content_enter_t;
    let group_count = groups.len();
    let mut y = list_top - state.scroll_offset;
    let mut flat_idx = 0usize;
    let mut stagger_idx = 0usize;

    ctx.with_axis_clip(rect(0.0, list_top, w, list_bot), || {
        for (g_idx, (proj, jobs)) in groups.into_iter().enumerate() {
            let wd_h = if proj.watchdog_note.is_some() { 20.0 } else { 0.0 };
            let block_h = 30.0 + wd_h + (jobs.len().max(1) as f32) * 40.0 + 4.0;
            let swatch_col = project_swatch_color(proj, g_idx);

            if y + block_h >= list_top && y <= list_bot {
                let (grp_alpha, grp_dy) =
                    crate::ui::layout::stagger_row_progress(enter_t, stagger_idx);
                stagger_idx += 1;

                if grp_alpha > 0.01 {
                    ctx.with_transformed_layer(0.0, grp_dy, 1.0, 1.0, grp_alpha, || {
                        // Flat exelban/stats Project Row Header (30 DIP, NO outer card border!)
                        let pin_r = rect(14.0, y + 4.0, 32.0, y + 26.0);
                        if proj.pinned {
                            ctx.draw_text("★", &fonts.mono_badge, pin_r, swatch_col, false)?;
                        } else {
                            // Signature exelban/stats 8x8 rounded color square
                            ctx.fill_rounded(rect(17.0, y + 10.5, 25.0, y + 18.5), 2.5, swatch_col)?;
                        }
                        hit_map.push_clipped(
                            next_id(),
                            pin_r,
                            list_top,
                            list_bot,
                            HitAction::TogglePinProject(proj.id.clone()),
                        );

                        let hdr_name_right = if proj.git_branch.is_some() { 180.0 } else { 212.0 };
                        let hdr_name_rect = rect(32.0, y + 2.0, hdr_name_right, y + 27.0);
                        let hdr_id = next_id();
                        ctx.draw_text(
                            &proj.name,
                            &fonts.title,
                            hdr_name_rect,
                            TEXT_PRIMARY,
                            false,
                        )?;
                        hit_map.push_with_rclick_clipped(
                            hdr_id,
                            hdr_name_rect,
                            list_top,
                            list_bot,
                            HitAction::OpenProjectDetail(proj.id.clone()),
                            HitAction::BeginRenameProject {
                                project_id: proj.id.clone(),
                                current_name: proj.name.clone(),
                            },
                        );

                        let jobs_x0 = hdr_name_right + 2.0;
                        let jobs_x1 = jobs_x0 + 44.0;
                        ctx.draw_text(
                            &format!("{} {}", jobs.len(), lang.pick("jobs", "iş")),
                            &fonts.row_secondary,
                            rect(jobs_x0, y + 3.0, jobs_x1, y + 26.0),
                            TEXT_MUTED,
                            true,
                        )?;
                        if let Some(ref branch) = proj.git_branch {
                            let bw = ((branch.chars().count().min(11) as f32) * 5.8 + 12.0)
                                .clamp(36.0, 82.0);
                            let b_left = jobs_x1 + 4.0;
                            let b_rect =
                                rect(b_left, y + 5.5, (b_left + bw).min(312.0), y + 23.5);
                            ctx.fill_rounded(b_rect, 4.0, BG_ELEVATED)?;
                            ctx.draw_text(
                                branch,
                                &fonts.mono_badge,
                                b_rect,
                                TEXT_SECONDARY,
                                true,
                            )?;
                        }

                        // Hover reorder / rename controls on project header
                        let up_r = rect(w - 116.0, y + 4.0, w - 98.0, y + 25.0);
                        let up_id = next_id();
                        let dn_r = rect(w - 96.0, y + 4.0, w - 78.0, y + 25.0);
                        let dn_id = next_id();
                        let rn_r = rect(w - 76.0, y + 4.0, w - 58.0, y + 25.0);
                        let rn_id = next_id();
                        let hdr_hovered = state.hovered_hit_id == Some(hdr_id)
                            || state.hovered_hit_id == Some(up_id)
                            || state.hovered_hit_id == Some(dn_id)
                            || state.hovered_hit_id == Some(rn_id);

                        if hdr_hovered {
                            if state.hovered_hit_id == Some(up_id) {
                                ctx.fill_rounded(up_r, 4.0, BG_ELEVATED_HOVER)?;
                            }
                            ctx.draw_text("↑", &fonts.mono_badge, up_r, TEXT_MUTED, false)?;
                            if state.hovered_hit_id == Some(dn_id) {
                                ctx.fill_rounded(dn_r, 4.0, BG_ELEVATED_HOVER)?;
                            }
                            ctx.draw_text("↓", &fonts.mono_badge, dn_r, TEXT_MUTED, false)?;
                            if state.hovered_hit_id == Some(rn_id) {
                                ctx.fill_rounded(rn_r, 4.0, BG_ELEVATED_HOVER)?;
                            }
                            ctx.draw_text("✎", &fonts.mono_badge, rn_r, TEXT_MUTED, false)?;
                        } else {
                            // Total project RAM & CPU% (crisp tabular sans-serif)
                            ctx.draw_text(
                                &format_ram(proj.ram),
                                &fonts.mono_metric,
                                rect(316.0, y + 2.0, 394.0, y + 27.0),
                                TEXT_PRIMARY,
                                true,
                            )?;
                            ctx.draw_text(
                                &format!("{:.1}%", proj.cpu),
                                &fonts.mono_metric,
                                rect(396.0, y + 2.0, w - 58.0, y + 27.0),
                                TEXT_SECONDARY,
                                true,
                            )?;
                        }
                        hit_map.push_clipped(
                            up_id,
                            up_r,
                            list_top,
                            list_bot,
                            HitAction::MoveProjectOrder(proj.id.clone(), -1),
                        );
                        hit_map.push_clipped(
                            dn_id,
                            dn_r,
                            list_top,
                            list_bot,
                            HitAction::MoveProjectOrder(proj.id.clone(), 1),
                        );
                        hit_map.push_clipped(
                            rn_id,
                            rn_r,
                            list_top,
                            list_bot,
                            HitAction::BeginRenameProject {
                                project_id: proj.id.clone(),
                                current_name: proj.name.clone(),
                            },
                        );

                        // Borderless Folder + Detail icons (hover pill only)
                        if proj.root.is_some() {
                            let f_r = rect(w - 52.0, y + 4.0, w - 34.0, y + 25.0);
                            let fid = next_id();
                            if state.hovered_hit_id == Some(fid) {
                                ctx.fill_rounded(f_r, 5.0, BG_ELEVATED_HOVER)?;
                            }
                            ctx.draw_text("↗", &fonts.mono_badge, f_r, TEXT_MUTED, false)?;
                            hit_map.push_clipped(
                                fid,
                                f_r,
                                list_top,
                                list_bot,
                                HitAction::OpenProjectFolder(proj.id.clone()),
                            );
                        }
                        let d_r = rect(w - 32.0, y + 4.0, w - 14.0, y + 25.0);
                        let did = next_id();
                        if state.hovered_hit_id == Some(did) {
                            ctx.fill_rounded(d_r, 5.0, BG_ELEVATED_HOVER)?;
                        }
                        ctx.draw_text("›", &fonts.mono_badge, d_r, TEXT_SECONDARY, false)?;
                        hit_map.push_clipped(
                            did,
                            d_r,
                            list_top,
                            list_bot,
                            HitAction::OpenProjectDetail(proj.id.clone()),
                        );

                        // Optional Watchdog inline info line (no heavy box!)
                        if let Some(ref wd_note) = proj.watchdog_note {
                            let wy = y + 28.0;
                            ctx.draw_status_dot(30.0, wy + 10.0, 2.5, ACCENT_INFO, false)?;
                            ctx.draw_text(
                                wd_note,
                                &fonts.row_secondary,
                                rect(38.0, wy, w - 18.0, wy + 19.0),
                                TEXT_MUTED,
                                true,
                            )?;
                        }
                        Ok(())
                    })?;
                }

                // Job Rows inside Project (40 DIP per row, zero nested boxes)
                for (j_idx, job) in jobs.iter().enumerate() {
                    let jy = y + 28.0 + wd_h + j_idx as f32 * 40.0;
                    if jy + 40.0 < list_top || jy > list_bot {
                        flat_idx += 1;
                        continue;
                    }
                    let (row_alpha, row_dy) =
                        crate::ui::layout::stagger_row_progress(enter_t, stagger_idx);
                    stagger_idx += 1;
                    if row_alpha > 0.01 {
                        ctx.with_transformed_layer(0.0, row_dy, 1.0, 1.0, row_alpha, || {
                            render_job_row(
                                ctx,
                                fonts,
                                w,
                                jy,
                                list_top,
                                list_bot,
                                job,
                                flat_idx == state.selected_index,
                                lang,
                                state,
                                hit_map,
                                next_id,
                            )
                        })?;
                    }
                    flat_idx += 1;
                }

                if g_idx + 1 < group_count && grp_alpha > 0.01 {
                    ctx.with_transformed_layer(0.0, grp_dy, 1.0, 1.0, grp_alpha, || {
                        ctx.draw_hairline(16.0, y + block_h + 2.0, w - 16.0)
                    })?;
                }
            } else {
                flat_idx += jobs.len();
            }
            y += block_h + 6.0;
        }
        Ok(())
    })?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_job_row<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    jy: f32,
    list_top: f32,
    list_bot: f32,
    job: &Job,
    is_selected: bool,
    lang: Language,
    state: &UiState,
    hit_map: &mut HitMap,
    next_id: &mut F,
) -> windows::core::Result<()> {
    let row_rect = rect(12.0, jy + 1.0, w - 12.0, jy + 39.0);
    let row_id = next_id();
    let up_rect = rect(w - 130.0, jy + 8.0, w - 110.0, jy + 31.0);
    let up_id = next_id();
    let dn_rect = rect(w - 108.0, jy + 8.0, w - 88.0, jy + 31.0);
    let dn_id = next_id();
    let ed_rect = rect(w - 86.0, jy + 8.0, w - 66.0, jy + 31.0);
    let ed_id = next_id();
    let r_rect = rect(w - 64.0, jy + 8.0, w - 42.0, jy + 31.0);
    let rid = next_id();
    let k_rect = rect(w - 38.0, jy + 8.0, w - 16.0, jy + 31.0);
    let kid = next_id();
    let pid_hit = next_id();

    let row_hovered = state.hovered_hit_id == Some(row_id)
        || state.hovered_hit_id == Some(up_id)
        || state.hovered_hit_id == Some(dn_id)
        || state.hovered_hit_id == Some(ed_id)
        || state.hovered_hit_id == Some(rid)
        || state.hovered_hit_id == Some(kid)
        || state.hovered_hit_id == Some(pid_hit);

    let show_selection = is_selected && !state.show_all_processes;
    if row_hovered {
        ctx.fill_rounded(row_rect, 7.0, BG_ELEVATED)?;
    } else if show_selection {
        ctx.fill_rounded(row_rect, 7.0, rgba(10, 132, 255, 0.11))?;
    }
    if show_selection {
        ctx.stroke_rounded(row_rect, 7.0, rgba(10, 132, 255, 0.55), 1.0)?;
    }

    let dot_col = if job.status == ProjectStatus::Stopped {
        TEXT_MUTED
    } else {
        job.color
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or_else(|| status_color(job.status))
    };
    if row_hovered || show_selection {
        ctx.fill_rounded(
            rect(14.5, jy + 10.0, 17.0, jy + 30.0),
            1.25,
            if show_selection { ACCENT_INFO } else { dot_col },
        )?;
    }
    ctx.draw_status_dot(
        30.0,
        jy + 12.0,
        3.2,
        dot_col,
        job.status == ProjectStatus::Healthy,
    )?;

    // Line 1: Clean Job Title + muted ×N suffix + clean port chip + terminal badge
    let has_multi_procs = job.procs.len() > 1;
    let has_port = !job.ports.is_empty();
    let has_term_hint =
        job.log_path.is_some() || job.last_log.is_some() || job.custom_command.is_some();
    let title_right = if has_multi_procs && has_port {
        184.0
    } else if has_multi_procs || has_port {
        214.0
    } else if has_term_hint {
        262.0
    } else {
        290.0
    };
    let title_left = if row_hovered { 41.5 } else { 40.0 };
    ctx.draw_text(
        &job.title,
        &fonts.row_primary,
        rect(title_left, jy + 2.0, title_right, jy + 21.0),
        if job.status == ProjectStatus::Stopped {
            TEXT_MUTED
        } else {
            TEXT_PRIMARY
        },
        false,
    )?;

    if has_multi_procs {
        let bx = if has_port { 188.0 } else { 218.0 };
        ctx.draw_text(
            &format!("×{}", job.procs.len()),
            &fonts.row_secondary,
            rect(bx, jy + 2.0, bx + 26.0, jy + 21.0),
            TEXT_MUTED,
            true,
        )?;
    }

    if has_term_hint && !has_port {
        let tx = if has_multi_procs { 246.0 } else { 264.0 };
        ctx.draw_text(
            ">_",
            &fonts.micro_lbl,
            rect(tx, jy + 4.0, tx + 18.0, jy + 19.0),
            ACCENT_TEAL,
            false,
        )?;
    }

    hit_map.push_with_rclick_clipped(
        row_id,
        row_rect,
        list_top,
        list_bot,
        HitAction::OpenJobDetail(job.project.clone(), job.id.clone()),
        HitAction::BeginRenameJob {
            match_hint: job.match_hint.clone(),
            current_title: job.title.clone(),
        },
    );

    if let Some(port) = job.ports.first() {
        let has_port_multi = job.ports.len() > 1;
        let p_rect = if has_port_multi {
            rect(214.0, jy + 3.0, 278.0, jy + 20.0)
        } else {
            rect(226.0, jy + 3.0, 278.0, jy + 20.0)
        };
        let label = if has_port_multi {
            format!(":{}↗ +{}", port.port, job.ports.len() - 1)
        } else {
            format!(":{}↗", port.port)
        };
        let copy_target = port
            .suggested_url
            .clone()
            .unwrap_or_else(|| format!("http://localhost:{}", port.port));
        ctx.fill_rounded(
            p_rect,
            4.0,
            if state.hovered_hit_id == Some(pid_hit) {
                rgba(10, 132, 255, 0.28)
            } else {
                ACCENT_INFO_BG
            },
        )?;
        ctx.draw_text(&label, &fonts.mono_badge, p_rect, ACCENT_INFO, true)?;
        hit_map.push_with_rclick_clipped(
            pid_hit,
            p_rect,
            list_top,
            list_bot,
            HitAction::OpenPort {
                port: port.port,
                url: port.suggested_url.clone(),
            },
            HitAction::CopyText(copy_target),
        );
    }

    // Telemetry columns (when hovered/confirming, shift left slightly for action buttons)
    let confirming = matches!(&state.pending_kill, Some(PendingKill::Job(jid)) if jid == &job.id);
    let show_actions = row_hovered || confirming;

    if matches!(job.status, ProjectStatus::Stopped | ProjectStatus::Crashed)
        && job.procs.is_empty()
    {
        let right_edge = if show_actions { w - 136.0 } else { w - 16.0 };
        let (lbl, col) = if job.status == ProjectStatus::Crashed {
            (lang.pick("Crashed", "Çöktü"), ACCENT_DANGER)
        } else {
            (lang.pick("Stopped", "Durdu"), TEXT_MUTED)
        };
        ctx.draw_text(
            lbl,
            &fonts.mono_metric,
            rect(right_edge - 62.0, jy + 2.0, right_edge, jy + 21.0),
            col,
            false,
        )?;
    } else if show_actions {
        ctx.draw_text(
            &format_ram(job.ram),
            &fonts.mono_metric,
            rect(280.0, jy + 2.0, 342.0, jy + 21.0),
            TEXT_PRIMARY,
            true,
        )?;
        ctx.draw_text(
            &format!("{:.1}%", job.cpu),
            &fonts.mono_metric,
            rect(344.0, jy + 2.0, w - 134.0, jy + 21.0),
            if job.cpu > 75.0 { ACCENT_WARN } else { TEXT_SECONDARY },
            true,
        )?;
    } else {
        // Resting exelban/stats clean columns right-aligned to edge
        ctx.draw_text(
            &format_ram(job.ram),
            &fonts.mono_metric,
            rect(294.0, jy + 2.0, 366.0, jy + 21.0),
            TEXT_PRIMARY,
            true,
        )?;
        ctx.draw_text(
            &format!("{:.1}%", job.cpu),
            &fonts.mono_metric,
            rect(368.0, jy + 2.0, 424.0, jy + 21.0),
            if job.cpu > 75.0 { ACCENT_WARN } else { TEXT_SECONDARY },
            true,
        )?;
        ctx.draw_text(
            &format_uptime(job.uptime),
            &fonts.mono_metric,
            rect(426.0, jy + 2.0, w - 16.0, jy + 21.0),
            TEXT_MUTED,
            true,
        )?;
    }

    // Line 2: crash summary (red) OR clean relative path + last_log
    let sub_right = if show_actions { w - 136.0 } else { w - 18.0 };
    if let Some(ref crash) = job.crash_summary {
        ctx.draw_text(
            crash,
            &fonts.row_secondary,
            rect(40.0, jy + 20.0, sub_right, jy + 37.0),
            ACCENT_DANGER,
            true,
        )?;
    } else {
        let proj_prefix = format!("{} › ", job.project_name);
        let clean_sub = job
            .subtitle
            .strip_prefix(&proj_prefix)
            .unwrap_or(&job.subtitle);
        let sub_line = if let Some(ref log) = job.last_log {
            format!("{}  ·  {}", clean_sub, log)
        } else {
            clean_sub.to_string()
        };
        ctx.draw_text(
            &sub_line,
            &fonts.row_secondary,
            rect(40.0, jy + 20.0, sub_right, jy + 37.0),
            TEXT_MUTED,
            false,
        )?;
    }

    // Hover-only Action Buttons (or inline Kill Confirmation)
    if confirming {
        let ok_r = rect(w - 130.0, jy + 8.0, w - 74.0, jy + 31.0);
        let cn_r = rect(w - 70.0, jy + 8.0, w - 16.0, jy + 31.0);
        ctx.fill_rounded(ok_r, 6.0, ACCENT_DANGER)?;
        ctx.draw_text(
            lang.pick("Confirm", "Onayla"),
            &fonts.mono_badge,
            ok_r,
            BG_OVERLAY,
            false,
        )?;
        hit_map.push_clipped(next_id(), ok_r, list_top, list_bot, HitAction::ConfirmKillJob(job.id.clone()));

        ctx.fill_rounded(cn_r, 6.0, BG_ELEVATED_HOVER)?;
        ctx.draw_text(
            lang.pick("Cancel", "İptal"),
            &fonts.mono_badge,
            cn_r,
            TEXT_PRIMARY,
            false,
        )?;
        hit_map.push_clipped(next_id(), cn_r, list_top, list_bot, HitAction::CancelKillConfirm);
    } else {
        if show_actions {
            for (b_rect, b_id, lbl) in [(up_rect, up_id, "↑"), (dn_rect, dn_id, "↓"), (ed_rect, ed_id, "✎")] {
                ctx.fill_rounded(
                    b_rect,
                    5.0,
                    if state.hovered_hit_id == Some(b_id) {
                        BG_ELEVATED_HOVER
                    } else {
                        BG_WELL
                    },
                )?;
                ctx.draw_text(lbl, &fonts.mono_badge, b_rect, TEXT_SECONDARY, false)?;
            }

            ctx.fill_rounded(
                r_rect,
                5.0,
                if job.can_restart && state.hovered_hit_id == Some(rid) {
                    BG_ELEVATED_HOVER
                } else {
                    BG_WELL
                },
            )?;
            ctx.draw_text(
                if job.status == ProjectStatus::Stopped {
                    "▶"
                } else {
                    "↻"
                },
                &fonts.mono_badge,
                r_rect,
                if job.can_restart {
                    if job.status == ProjectStatus::Stopped {
                        ACCENT_LIVE
                    } else {
                        TEXT_PRIMARY
                    }
                } else {
                    TEXT_MUTED
                },
                false,
            )?;

            let can_remove_watch = job.status == ProjectStatus::Stopped && job.is_watched && !job.can_kill;
            ctx.fill_rounded(
                k_rect,
                5.0,
                if (job.can_kill || can_remove_watch) && state.hovered_hit_id == Some(kid) {
                    ACCENT_DANGER_BG
                } else {
                    BG_WELL
                },
            )?;
            ctx.draw_text(
                if can_remove_watch { "×" } else { "■" },
                &fonts.mono_badge,
                k_rect,
                if job.can_kill || can_remove_watch {
                    ACCENT_DANGER
                } else {
                    TEXT_MUTED
                },
                false,
            )?;
        }
        let order_key = if !job.match_hint.is_empty() {
            job.match_hint.clone()
        } else {
            job.id.0.clone()
        };
        hit_map.push_clipped(up_id, up_rect, list_top, list_bot, HitAction::MoveJobOrder(order_key.clone(), -1));
        hit_map.push_clipped(dn_id, dn_rect, list_top, list_bot, HitAction::MoveJobOrder(order_key, 1));
        hit_map.push_clipped(
            ed_id,
            ed_rect,
            list_top,
            list_bot,
            HitAction::BeginRenameJob {
                match_hint: job.match_hint.clone(),
                current_title: job.title.clone(),
            },
        );
        if job.can_restart {
            if job.status == ProjectStatus::Stopped && job.custom_command.is_some() {
                hit_map.push_clipped(rid, r_rect, list_top, list_bot, HitAction::StartCustomJob(job.id.clone()));
            } else {
                hit_map.push_clipped(rid, r_rect, list_top, list_bot, HitAction::RestartJob(job.id.clone()));
            }
        }
        if job.can_kill {
            hit_map.push_clipped(kid, k_rect, list_top, list_bot, HitAction::RequestKillJob(job.id.clone()));
        } else if job.status == ProjectStatus::Stopped && job.is_watched {
            hit_map.push_clipped(kid, k_rect, list_top, list_bot, HitAction::RemoveWatchedJob(job.match_hint.clone()));
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_all_procs_list<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    list_top: f32,
    list_bot: f32,
    viewport_h: f32,
    lang: Language,
    state: &mut UiState,
    snap: &Snapshot,
    hit_map: &mut HitMap,
    next_id: &mut F,
) -> windows::core::Result<()> {
    let q = fold_search_lower(state.search_query.trim());
    let procs: Vec<_> = snap
        .all_procs
        .iter()
        .filter(|p| {
            q.is_empty()
                || fold_search_lower(&p.name).contains(&q)
                || p.key.pid.to_string().contains(&q)
                || fold_search_lower(&p.role_label).contains(&q)
        })
        .collect();

    let row_h = 42.0;
    let total_h = procs.len() as f32 * row_h + 24.0;
    state.max_scroll = (total_h - viewport_h).max(0.0);
    state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_scroll);

    ctx.with_axis_clip(rect(0.0, list_top, w, list_bot), || {
        let hdr_y = list_top - state.scroll_offset;
        ctx.draw_text(
            &format!(
                "{} ({})",
                lang.pick(
                    "ALL OS PROCESSES — raw fallback view",
                    "TÜM İŞLETİM SİSTEMİ SÜREÇLERİ — ham görünüm",
                ),
                procs.len()
            ),
            &fonts.section_hdr,
            rect(14.0, hdr_y, w - 14.0, hdr_y + 20.0),
            TEXT_MUTED,
            true,
        )?;

        for (idx, pr) in procs.iter().enumerate() {
            let y = hdr_y + 22.0 + idx as f32 * row_h;
            if y + row_h < list_top || y > list_bot {
                continue;
            }
            let rr = rect(12.0, y, w - 12.0, y + row_h - 4.0);
            ctx.fill_rounded(rr, 7.0, BG_CARD)?;
            ctx.draw_text(
                &format!("{} · {}", pr.name, pr.role_label),
                &fonts.row_primary,
                rect(22.0, y + 2.0, 290.0, y + 20.0),
                TEXT_PRIMARY,
                false,
            )?;
            ctx.draw_text(
                &format!("PID {} · {}", pr.key.pid, format_uptime(pr.uptime_secs)),
                &fonts.row_secondary,
                rect(22.0, y + 19.0, 290.0, y + 36.0),
                TEXT_MUTED,
                true,
            )?;
            let confirming = matches!(
                &state.pending_kill,
                Some(PendingKill::Process(pk)) if pk == &pr.key
            );
            let metric_right = if confirming { w - 126.0 } else { w - 118.0 };
            ctx.draw_text(
                &format!("{:.1}% · {}", pr.cpu, format_ram(pr.ram)),
                &fonts.mono_metric,
                rect(280.0, y + 4.0, metric_right, y + 34.0),
                TEXT_SECONDARY,
                true,
            )?;
            if confirming {
                let ok_r = rect(w - 120.0, y + 7.0, w - 66.0, y + 31.0);
                ctx.fill_rounded(ok_r, 5.0, ACCENT_DANGER)?;
                ctx.draw_text(
                    lang.pick("Confirm", "Onayla"),
                    &fonts.mono_badge,
                    ok_r,
                    BG_OVERLAY,
                    false,
                )?;
                hit_map.push_clipped(
                    next_id(),
                    ok_r,
                    list_top,
                    list_bot,
                    HitAction::ConfirmKillProcess(pr.key),
                );

                let cn_r = rect(w - 62.0, y + 7.0, w - 18.0, y + 31.0);
                ctx.fill_rounded(cn_r, 5.0, BG_ELEVATED_HOVER)?;
                ctx.draw_text(
                    lang.pick("Cancel", "İptal"),
                    &fonts.mono_badge,
                    cn_r,
                    TEXT_PRIMARY,
                    false,
                )?;
                hit_map.push_clipped(next_id(), cn_r, list_top, list_bot, HitAction::CancelKillConfirm);
            } else {
                let short_exe = pr
                    .name
                    .strip_suffix(".exe")
                    .or_else(|| pr.name.strip_suffix(".EXE"))
                    .unwrap_or(&pr.name)
                    .to_string();
                if !pr.is_critical_system {
                    let add_r = rect(w - 112.0, y + 7.0, w - 52.0, y + 31.0);
                    let add_id = next_id();
                    ctx.fill_rounded(
                        add_r,
                        5.0,
                        if state.hovered_hit_id == Some(add_id) {
                            rgba(10, 132, 255, 0.30)
                        } else {
                            ACCENT_INFO_BG
                        },
                    )?;
                    ctx.draw_text(
                        lang.pick("+ Watch", "+ İzle"),
                        &fonts.mono_badge,
                        add_r,
                        ACCENT_INFO,
                        false,
                    )?;
                    hit_map.push_clipped(
                        add_id,
                        add_r,
                        list_top,
                        list_bot,
                        HitAction::WatchProcessFromAll {
                            match_substr: pr.name.clone(),
                            title: short_exe,
                            working_dir: pr
                                .working_dir
                                .as_ref()
                                .map(|p| p.to_string_lossy().into_owned()),
                        },
                    );
                }
                if pr.is_critical_system || !pr.can_kill {
                    let lk = rect(w - 48.0, y + 7.0, w - 16.0, y + 31.0);
                    ctx.fill_rounded(lk, 5.0, BG_OVERLAY)?;
                    ctx.draw_text("LOCK", &fonts.mono_badge, lk, TEXT_MUTED, false)?;
                } else {
                    let k_r = rect(w - 46.0, y + 7.0, w - 18.0, y + 31.0);
                    ctx.fill_rounded(k_r, 5.0, ACCENT_DANGER_BG)?;
                    ctx.draw_text("■", &fonts.mono_badge, k_r, ACCENT_DANGER, false)?;
                    hit_map.push_clipped(
                        next_id(),
                        k_r,
                        list_top,
                        list_bot,
                        HitAction::RequestKillProcess(pr.key),
                    );
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}

