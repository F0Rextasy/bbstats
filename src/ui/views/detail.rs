use super::rect;
use crate::config::AppConfig;
use crate::model::{JobId, Language, ProjectId, ProjectStatus, Snapshot, TerminalSourceKind};
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::{format_ram, format_uptime, PendingKill, UiState};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_detail(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    state: &mut UiState,
    snap: &Snapshot,
    cfg: &AppConfig,
    proj_id: &ProjectId,
    selected_job_id: Option<&JobId>,
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    let lang = cfg.language;
    let mut id_gen = 500u32;
    let mut next_id = || {
        let v = id_gen;
        id_gen += 1;
        v
    };

    let Some(proj) = snap.projects.iter().find(|p| &p.id == proj_id) else {
        let back_rect = rect(12.0, 9.0, 78.0, 35.0);
        ctx.fill_rounded(back_rect, 8.0, BG_ELEVATED)?;
        ctx.draw_text(
            lang.pick("← Back", "← Geri"),
            &fonts.mono_badge,
            back_rect,
            TEXT_PRIMARY,
            false,
        )?;
        hit_map.push(next_id(), back_rect, HitAction::BackToExpanded);
        ctx.draw_text(
            lang.pick(
                "Project stopped or removed from list.",
                "Proje durdu veya listeden kaldırıldı.",
            ),
            &fonts.row_secondary,
            rect(20.0, 64.0, w - 20.0, 96.0),
            TEXT_MUTED,
            false,
        )?;
        return Ok(());
    };

    let active_job = selected_job_id
        .and_then(|jid| proj.jobs.iter().find(|j| &j.id == jid))
        .or_else(|| proj.jobs.first());

    // Header (44 DIP)
    let back_rect = rect(12.0, 9.0, 74.0, 35.0);
    let bid = next_id();
    ctx.fill_rounded(
        back_rect,
        8.0,
        if state.hovered_hit_id == Some(bid) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        lang.pick("← Back", "← Geri"),
        &fonts.mono_badge,
        back_rect,
        TEXT_PRIMARY,
        false,
    )?;
    hit_map.push(bid, back_rect, HitAction::BackToExpanded);

    let header_title = if let Some(j) = active_job {
        if proj.jobs.len() > 1 {
            format!("{} › {}", proj.name, j.title)
        } else {
            j.title.clone()
        }
    } else {
        proj.name.clone()
    };
    ctx.draw_text(
        &header_title,
        &fonts.title,
        rect(82.0, 6.0, w - 144.0, 25.0),
        TEXT_PRIMARY,
        false,
    )?;
    if let Some(ref root) = proj.root {
        let path_right = if proj.git_branch.is_some() {
            w - 236.0
        } else {
            w - 144.0
        };
        let path_rect = rect(82.0, 24.0, path_right, 40.0);
        ctx.draw_text(
            &format!("{} ↗", root.display()),
            &fonts.row_secondary,
            path_rect,
            ACCENT_INFO,
            false,
        )?;
        hit_map.push(next_id(), path_rect, HitAction::OpenProjectFolder(proj.id.clone()));
    }
    if let Some(ref branch) = proj.git_branch {
        let badge_w = ((branch.chars().count().min(12) as f32) * 5.8 + 14.0).clamp(44.0, 86.0);
        let badge_left = if proj.root.is_some() {
            w - 144.0 - badge_w
        } else {
            82.0
        };
        let b_rect = rect(badge_left, 24.0, badge_left + badge_w, 39.0);
        ctx.fill_rounded(b_rect, 4.0, BG_ELEVATED)?;
        ctx.draw_text(branch, &fonts.mono_badge, b_rect, TEXT_SECONDARY, true)?;
    }

    let hide_rect = rect(w - 136.0, 9.0, w - 108.0, 35.0);
    let hid = next_id();
    ctx.fill_rounded(
        hide_rect,
        8.0,
        if state.hovered_hit_id == Some(hid) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text("⊘", &fonts.mono_badge, hide_rect, TEXT_SECONDARY, false)?;
    hit_map.push(hid, hide_rect, HitAction::HideProject(proj.id.clone()));

    let pin_rect = rect(w - 104.0, 9.0, w - 76.0, 35.0);
    ctx.fill_rounded(
        pin_rect,
        8.0,
        if proj.pinned { ACCENT_INFO_BG } else { BG_ELEVATED },
    )?;
    ctx.draw_text(
        "★",
        &fonts.mono_badge,
        pin_rect,
        if proj.pinned { ACCENT_INFO } else { TEXT_SECONDARY },
        false,
    )?;
    hit_map.push(next_id(), pin_rect, HitAction::TogglePinProject(proj.id.clone()));

    let rst_rect = rect(w - 72.0, 9.0, w - 44.0, 35.0);
    let is_stopped_custom = active_job.is_some_and(|j| {
        j.status == ProjectStatus::Stopped && j.custom_command.is_some()
    });
    let rst_enabled = if is_stopped_custom {
        true
    } else if proj.jobs.len() > 1 {
        active_job.is_some_and(|j| j.can_restart)
    } else {
        active_job.is_some_and(|j| j.can_restart)
            || proj.jobs.iter().any(|j| j.can_restart)
    };
    let rst_id = next_id();
    ctx.fill_rounded(
        rst_rect,
        8.0,
        if rst_enabled && state.hovered_hit_id == Some(rst_id) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        if is_stopped_custom { "▶" } else { "↻" },
        &fonts.mono_badge,
        rst_rect,
        if !rst_enabled {
            TEXT_MUTED
        } else if is_stopped_custom {
            ACCENT_LIVE
        } else {
            TEXT_PRIMARY
        },
        false,
    )?;
    if rst_enabled {
        if let Some(j) = active_job {
            if is_stopped_custom {
                hit_map.push(rst_id, rst_rect, HitAction::StartCustomJob(j.id.clone()));
            } else if j.can_restart {
                hit_map.push(rst_id, rst_rect, HitAction::RestartJob(j.id.clone()));
            } else if proj.jobs.len() <= 1 {
                hit_map.push(rst_id, rst_rect, HitAction::RestartProject(proj.id.clone()));
            }
        } else if proj.jobs.len() <= 1 {
            hit_map.push(rst_id, rst_rect, HitAction::RestartProject(proj.id.clone()));
        }
    }

    let stop_rect = rect(w - 40.0, 9.0, w - 12.0, 35.0);
    let scope_job = if proj.jobs.len() > 1 {
        active_job
    } else {
        None
    };
    let has_killable_procs = proj
        .procs
        .iter()
        .any(|p| p.can_kill && !p.is_critical_system);
    let stop_enabled = match scope_job {
        Some(j) => j.can_kill && j.procs.iter().any(|p| p.can_kill && !p.is_critical_system),
        None => has_killable_procs,
    };
    let stop_id = next_id();
    ctx.fill_rounded(
        stop_rect,
        8.0,
        if stop_enabled {
            ACCENT_DANGER_BG
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        "■",
        &fonts.mono_badge,
        stop_rect,
        if stop_enabled { ACCENT_DANGER } else { TEXT_MUTED },
        false,
    )?;
    if stop_enabled {
        if let Some(j) = scope_job {
            hit_map.push(stop_id, stop_rect, HitAction::RequestKillJob(j.id.clone()));
        } else {
            hit_map.push(stop_id, stop_rect, HitAction::RequestKillProject(proj.id.clone()));
        }
    }

    ctx.draw_hairline(12.0, 44.0, w - 12.0)?;

    let mut y_cursor = 50.0;
    let confirm_target =
        if matches!(&state.pending_kill, Some(PendingKill::Project(id)) if id == &proj.id) {
            Some((
                match lang {
                    Language::En => format!(
                        "Stop {} {} in {}?",
                        proj.procs.len(),
                        lang.plural(proj.procs.len(), "process", "processes", "süreç"),
                        proj.name
                    ),
                    Language::Tr => {
                        format!("{} içindeki {} süreç durdurulsun mu?", proj.name, proj.procs.len())
                    }
                },
                HitAction::ConfirmKillProject(proj.id.clone()),
            ))
        } else {
            active_job
                .filter(|j| {
                    matches!(&state.pending_kill, Some(PendingKill::Job(jid)) if jid == &j.id)
                })
                .map(|j| {
                    (
                        match lang {
                            Language::En => {
                                format!(
                                    "Stop {} {} in {}?",
                                    j.procs.len(),
                                    lang.plural(j.procs.len(), "process", "processes", "süreç"),
                                    j.title
                                )
                            }
                            Language::Tr => {
                                format!("{} içindeki {} süreç durdurulsun mu?", j.title, j.procs.len())
                            }
                        },
                        HitAction::ConfirmKillJob(j.id.clone()),
                    )
                })
        };
    if let Some((kill_msg, confirm_action)) = confirm_target {
        let bar = rect(12.0, y_cursor, w - 12.0, y_cursor + 32.0);
        ctx.fill_rounded(bar, 8.0, ACCENT_DANGER_BG)?;
        ctx.stroke_rounded(bar, 8.0, ACCENT_DANGER, 1.0)?;
        ctx.draw_text(
            &kill_msg,
            &fonts.row_secondary,
            rect(22.0, y_cursor, w - 154.0, y_cursor + 32.0),
            TEXT_PRIMARY,
            true,
        )?;
        let ok_r = rect(w - 148.0, y_cursor + 4.0, w - 78.0, y_cursor + 28.0);
        ctx.fill_rounded(ok_r, 6.0, ACCENT_DANGER)?;
        ctx.draw_text(
            lang.pick("Confirm", "Onayla"),
            &fonts.mono_badge,
            ok_r,
            BG_OVERLAY,
            false,
        )?;
        hit_map.push(next_id(), ok_r, confirm_action);

        let cn_r = rect(w - 72.0, y_cursor + 4.0, w - 18.0, y_cursor + 28.0);
        ctx.fill_rounded(cn_r, 6.0, BG_ELEVATED_HOVER)?;
        ctx.draw_text(
            lang.pick("Cancel", "İptal"),
            &fonts.mono_badge,
            cn_r,
            TEXT_PRIMARY,
            false,
        )?;
        hit_map.push(next_id(), cn_r, HitAction::CancelKillConfirm);

        y_cursor += 38.0;
    }

    // Job switcher tabs (wrap cleanly across rows when 5+ jobs are active)
    if proj.jobs.len() > 1 {
        let mut tx = 12.0;
        for j in &proj.jobs {
            let is_sel = active_job.is_some_and(|a| a.id == j.id);
            let tab_w = ((j.title.chars().count() as f32) * 6.5 + 22.0).clamp(64.0, 128.0);
            if tx > 12.0 && tx + tab_w > w - 12.0 {
                tx = 12.0;
                y_cursor += 26.0;
            }
            let tab_hit_id = next_id();
            let tab_r = rect(tx, y_cursor, tx + tab_w, y_cursor + 22.0);
            let tab_bg = if is_sel {
                ACCENT_INFO_BG
            } else if state.hovered_hit_id == Some(tab_hit_id) {
                BG_ELEVATED_HOVER
            } else {
                BG_ELEVATED
            };
            ctx.fill_rounded(tab_r, 6.0, tab_bg)?;
            if is_sel {
                ctx.stroke_rounded(tab_r, 6.0, ACCENT_INFO, 1.0)?;
                ctx.fill_rounded(
                    rect(tx + 10.0, y_cursor + 19.5, tx + tab_w - 10.0, y_cursor + 21.5),
                    1.0,
                    ACCENT_INFO,
                )?;
            }
            ctx.draw_text(
                &j.title,
                &fonts.row_secondary,
                rect(tx + 6.0, y_cursor, tx + tab_w - 6.0, y_cursor + 22.0),
                if is_sel { ACCENT_INFO } else { TEXT_SECONDARY },
                false,
            )?;
            hit_map.push(
                tab_hit_id,
                tab_r,
                HitAction::OpenJobDetail(proj.id.clone(), j.id.clone()),
            );
            tx += tab_w + 6.0;
        }
        y_cursor += 28.0;
    }

    let (slide_x, body_alpha) = if state.detail_tab_t < 0.995 {
        let u = state.detail_tab_t.clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - u).powi(3);
        let sx = (1.0 - eased) * 14.0 * state.detail_tab_dir;
        let a = (0.28 + 0.72 * eased).clamp(0.0, 1.0);
        (sx, a)
    } else {
        (0.0, 1.0)
    };

    let pre_body_hits = hit_map.regions.len();
    ctx.with_transformed_layer(slide_x, 0.0, 1.0, 1.0, body_alpha, || {
        render_detail_body(
            ctx,
            fonts,
            w,
            h,
            y_cursor,
            lang,
            proj,
            active_job,
            state,
            snap,
            hit_map,
            &mut next_id,
        )
    })?;
    if slide_x.abs() > 0.01 {
        for r in &mut hit_map.regions[pre_body_hits..] {
            r.rect.left += slide_x;
            r.rect.right += slide_x;
        }
        if let Some(vp) = &mut hit_map.scroll_viewport {
            vp.left += slide_x;
            vp.right += slide_x;
        }
        if let Some(tvp) = &mut hit_map.term_viewport {
            tvp.left += slide_x;
            tvp.right += slide_x;
        }
    }

    let list_bot = h - 28.0;
    ctx.draw_hairline(12.0, list_bot, w - 12.0)?;
    ctx.draw_text(
        lang.pick(
            "Esc: Back · :port left-click: open · right-click: copy",
            "Esc: Geri · :port sol tık: aç · sağ tık: kopyala",
        ),
        &fonts.row_secondary,
        rect(14.0, list_bot + 4.0, w - 14.0, h - 2.0),
        TEXT_MUTED,
        false,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_detail_body<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    mut y_cursor: f32,
    lang: Language,
    proj: &crate::model::ProjectGroup,
    active_job: Option<&crate::model::Job>,
    state: &mut UiState,
    snap: &Snapshot,
    hit_map: &mut HitMap,
    next_id: &mut F,
) -> windows::core::Result<()> {
    let mut next_id = || next_id();
    // Clickable / right-click-copyable port bar for active job (or project fallback)
    let detail_ports: &[crate::model::PortBinding] = active_job
        .filter(|j| !j.ports.is_empty())
        .map(|j| j.ports.as_slice())
        .unwrap_or(&proj.ports);
    if !detail_ports.is_empty() {
        ctx.draw_text(
            lang.pick("PORTS", "PORTLAR"),
            &fonts.micro_lbl,
            rect(14.0, y_cursor + 2.0, 68.0, y_cursor + 20.0),
            TEXT_MUTED,
            false,
        )?;
        let mut px = 72.0_f32;
        for port in detail_ports {
            let lbl = format!(":{}↗", port.port);
            let pw = (lbl.chars().count() as f32 * 6.5 + 14.0).clamp(50.0, 74.0);
            if px + pw > w - 12.0 {
                break;
            }
            let pr_rect = rect(px, y_cursor + 1.0, px + pw, y_cursor + 21.0);
            let pid_hit = next_id();
            ctx.fill_rounded(
                pr_rect,
                5.0,
                if state.hovered_hit_id == Some(pid_hit) {
                    rgba(10, 132, 255, 0.28)
                } else {
                    ACCENT_INFO_BG
                },
            )?;
            ctx.draw_text(&lbl, &fonts.mono_badge, pr_rect, ACCENT_INFO, true)?;
            let copy_url = port
                .suggested_url
                .clone()
                .unwrap_or_else(|| format!("http://localhost:{}", port.port));
            hit_map.push_with_rclick(
                pid_hit,
                pr_rect,
                HitAction::OpenPort {
                    port: port.port,
                    url: port.suggested_url.clone(),
                },
                HitAction::CopyText(copy_url),
            );
            px += pw + 6.0;
        }
        y_cursor += 26.0;
    }

    // Compact Dual Sparklines (CPU & RAM, 70 DIP)
    let half_w = (w - 36.0) * 0.5;
    let cpu_card = rect(12.0, y_cursor, 12.0 + half_w, y_cursor + 68.0);
    let ram_card = rect(24.0 + half_w, y_cursor, w - 12.0, y_cursor + 68.0);

    ctx.fill_rounded(cpu_card, 8.0, BG_CARD)?;
    ctx.stroke_rounded(cpu_card, 8.0, BORDER_SUBTLE, 1.0)?;
    let cpu_samples: [f32; 30] = std::array::from_fn(|i| proj.cpu_history[i] as f32);
    let cpu_peak = cpu_samples.iter().copied().fold(0.0_f32, f32::max);
    ctx.draw_text(
        &format!(
            "CPU  {:.1}% ({} {:.0}%)",
            proj.cpu,
            lang.pick("Peak", "Zirve"),
            cpu_peak
        ),
        &fonts.row_secondary,
        rect(cpu_card.left + 10.0, cpu_card.top + 3.0, cpu_card.right - 10.0, cpu_card.top + 20.0),
        TEXT_SECONDARY,
        true,
    )?;
    ctx.draw_sparkline(
        rect(cpu_card.left + 8.0, cpu_card.top + 22.0, cpu_card.right - 8.0, cpu_card.bottom - 5.0),
        &cpu_samples,
        cpu_peak.max(25.0),
        ACCENT_INFO,
    )?;

    ctx.fill_rounded(ram_card, 8.0, BG_CARD)?;
    ctx.stroke_rounded(ram_card, 8.0, BORDER_SUBTLE, 1.0)?;
    let ram_samples: [f32; 30] = std::array::from_fn(|i| proj.ram_history_mb[i] as f32);
    let ram_peak = ram_samples.iter().copied().fold(0.0_f32, f32::max);
    ctx.draw_text(
        &format!(
            "RAM  {} ({} {:.0}M)",
            format_ram(proj.ram),
            lang.pick("Peak", "Zirve"),
            ram_peak
        ),
        &fonts.row_secondary,
        rect(ram_card.left + 10.0, ram_card.top + 3.0, ram_card.right - 10.0, ram_card.top + 20.0),
        TEXT_SECONDARY,
        true,
    )?;
    ctx.draw_sparkline(
        rect(ram_card.left + 8.0, ram_card.top + 22.0, ram_card.right - 8.0, ram_card.bottom - 5.0),
        &ram_samples,
        ram_peak.max(128.0),
        ACCENT_RAM,
    )?;

    y_cursor += 76.0;

    // Terminal / Log Output Box (lazy read only while Detail is open)
    let term_src = snap.active_terminal.as_ref();
    let term_hdr = lang.pick("TERMINAL OUTPUT", "TERMİNAL ÇIKTISI");
    let src_label = term_src
        .map(|t| match (&t.source_kind, &t.source_hint) {
            (k, Some(h)) => format!("{term_hdr} · {} ({h})", k.label_for(lang)),
            (k, None) => format!("{term_hdr} · {}", k.label_for(lang)),
        })
        .unwrap_or_else(|| term_hdr.to_string());
    let show_live_btn = state.term_scroll_offset > 0.5;
    let hdr_right = if show_live_btn { w - 196.0 } else { w - 134.0 };
    ctx.draw_text(
        &src_label,
        &fonts.section_hdr,
        rect(14.0, y_cursor, hdr_right, y_cursor + 20.0),
        ACCENT_TEAL,
        true,
    )?;

    if show_live_btn {
        let live_btn = rect(w - 192.0, y_cursor, w - 134.0, y_cursor + 19.0);
        let lid = next_id();
        ctx.fill_rounded(
            live_btn,
            5.0,
            if state.hovered_hit_id == Some(lid) {
                rgba(10, 132, 255, 0.28)
            } else {
                ACCENT_INFO_BG
            },
        )?;
        ctx.draw_text(
            lang.pick("↓ Live", "↓ Canlı"),
            &fonts.micro_lbl,
            live_btn,
            ACCENT_INFO,
            false,
        )?;
        hit_map.push(lid, live_btn, HitAction::JumpTerminalLive);
    }

    let ref_btn = rect(w - 130.0, y_cursor, w - 76.0, y_cursor + 19.0);
    let ref_id = next_id();
    ctx.fill_rounded(
        ref_btn,
        5.0,
        if state.hovered_hit_id == Some(ref_id) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        lang.pick("↻ Refresh", "↻ Yenile"),
        &fonts.micro_lbl,
        ref_btn,
        TEXT_SECONDARY,
        false,
    )?;
    hit_map.push(ref_id, ref_btn, HitAction::RefreshTerminalOutput);

    let cpy_btn = rect(w - 72.0, y_cursor, w - 14.0, y_cursor + 19.0);
    let cpy_id = next_id();
    ctx.fill_rounded(
        cpy_btn,
        5.0,
        if state.hovered_hit_id == Some(cpy_id) {
            BG_ELEVATED_HOVER
        } else {
            BG_ELEVATED
        },
    )?;
    ctx.draw_text(
        lang.pick("Copy", "Kopyala"),
        &fonts.micro_lbl,
        cpy_btn,
        TEXT_PRIMARY,
        false,
    )?;
    hit_map.push(cpy_id, cpy_btn, HitAction::CopyTerminalOutput);

    y_cursor += 22.0;
    let displayed_procs: &[crate::model::ProcInfo] = if proj.jobs.len() > 1 {
        active_job.map(|j| j.procs.as_slice()).unwrap_or(&proj.procs)
    } else {
        &proj.procs
    };
    let reserve_procs = 22.0 + (displayed_procs.len().clamp(1, 3) as f32) * 46.0 + 38.0;
    let term_h = (h - y_cursor - reserve_procs).clamp(108.0, 190.0);
    let term_box = rect(12.0, y_cursor, w - 12.0, y_cursor + term_h);
    ctx.fill_rounded(term_box, 8.0, rgba(12, 13, 16, 0.94))?;
    ctx.stroke_rounded(term_box, 8.0, BORDER_SUBTLE, 1.0)?;
    hit_map.term_viewport = Some(term_box);

    let empty_lines: Vec<String> = Vec::new();
    let raw_term_lines = term_src.map(|t| &t.lines).unwrap_or(&empty_lines);
    let max_cols = ((term_box.right - term_box.left - 24.0) / 6.0)
        .floor()
        .max(40.0) as usize;
    let mut wrapped_lines: Vec<(String, D2D1_COLOR_F)> = Vec::new();
    for line in raw_term_lines {
        let low = line.to_ascii_lowercase();
        let has_ok_word = low
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|tok| tok == "ok");
        let col = if low.contains("error")
            || low.contains("fail")
            || low.contains("exception")
            || low.contains("traceback")
        {
            ACCENT_DANGER
        } else if low.contains("warn") {
            ACCENT_WARN
        } else if low.contains("ready") || low.contains("listening") || has_ok_word {
            ACCENT_LIVE
        } else {
            TEXT_PRIMARY
        };
        if line.chars().count() <= max_cols {
            wrapped_lines.push((line.clone(), col));
        } else {
            let mut chunk = String::with_capacity(max_cols);
            let mut col_cnt = 0usize;
            for ch in line.chars() {
                chunk.push(ch);
                col_cnt += 1;
                if col_cnt >= max_cols {
                    wrapped_lines.push((std::mem::take(&mut chunk), col));
                    col_cnt = 0;
                }
            }
            if !chunk.is_empty() {
                wrapped_lines.push((chunk, col));
            }
        }
    }

    let line_h = 15.0_f32;
    let inner_h = (term_h - 12.0).max(20.0);
    let total_term_h = wrapped_lines.len() as f32 * line_h;
    state.term_max_scroll = (total_term_h - inner_h).max(0.0);
    state.term_scroll_offset = state.term_scroll_offset.clamp(0.0, state.term_max_scroll);

    if wrapped_lines.is_empty() {
        let hint = if term_src.is_some_and(|t| t.source_kind == TerminalSourceKind::None) {
            lang.pick(
                "No direct console/log stream for this process.\nTip: Launch via (+ Add Job) in bbstats or bind a log file.",
                "Bu süreçten doğrudan konsol/log akışı okunamadı.\nİpucu: bbstats içinden (+ İş Ekle) komutla başlatabilir veya log dosyası bağlayabilirsin.",
            )
        } else {
            lang.pick("Waiting for output...", "Çıktı bekleniyor...")
        };
        ctx.draw_text(
            hint,
            &fonts.row_secondary,
            rect(term_box.left + 10.0, term_box.top + 10.0, term_box.right - 10.0, term_box.bottom - 10.0),
            TEXT_MUTED,
            false,
        )?;
    } else {
        // Bottom-anchored when `term_scroll_offset == 0.0` (scrolling up increases offset toward older lines).
        let bottom_y = term_box.bottom - 6.0 + state.term_scroll_offset;
        let text_right = if state.term_max_scroll > 0.0 {
            term_box.right - 14.0
        } else {
            term_box.right - 10.0
        };
        ctx.with_axis_clip(
            rect(term_box.left + 4.0, term_box.top + 4.0, term_box.right - 4.0, term_box.bottom - 4.0),
            || {
                for (rev_idx, (line, col)) in wrapped_lines.iter().rev().enumerate() {
                    let ly = bottom_y - (rev_idx as f32 + 1.0) * line_h;
                    if ly + line_h <= term_box.top + 2.0 || ly >= term_box.bottom - 2.0 {
                        continue;
                    }
                    ctx.draw_text(
                        line,
                        &fonts.term_line,
                        rect(term_box.left + 10.0, ly, text_right, ly + line_h),
                        *col,
                        true,
                    )?;
                }
                Ok(())
            },
        )?;

        if state.term_max_scroll > 0.0 {
            let track_top = term_box.top + 6.0;
            let track_bot = term_box.bottom - 6.0;
            let track_h = (track_bot - track_top).max(16.0);
            let thumb_h = ((inner_h / total_term_h) * track_h).clamp(18.0, track_h);
            // Offset 0.0 = bottom (1.0), term_max_scroll = top (0.0)
            let ratio_from_top = 1.0 - (state.term_scroll_offset / state.term_max_scroll).clamp(0.0, 1.0);
            let thumb_y = track_top + ratio_from_top * (track_h - thumb_h);
            ctx.fill_rounded(
                rect(term_box.right - 7.0, track_top, term_box.right - 4.0, track_bot),
                1.5,
                rgba(255, 255, 255, 0.07),
            )?;
            ctx.fill_rounded(
                rect(term_box.right - 7.0, thumb_y, term_box.right - 4.0, thumb_y + thumb_h),
                1.5,
                rgba(255, 255, 255, 0.32),
            )?;
        }
    }

    y_cursor += term_h + 10.0;

    // Underlying Raw Processes Tree (hidden in main list, inspectable here)
    ctx.draw_text(
        &format!(
            "{} ({})",
            lang.pick("SUBPROCESSES", "ALT SÜREÇLER"),
            displayed_procs.len()
        ),
        &fonts.section_hdr,
        rect(14.0, y_cursor, 300.0, y_cursor + 20.0),
        TEXT_MUTED,
        true,
    )?;
    let list_top = y_cursor + 22.0;
    let list_bot = (h - 28.0).max(list_top);
    hit_map.scroll_viewport = Some(rect(0.0, list_top, w, list_bot));

    let row_h = 46.0;
    let total_h = displayed_procs.len() as f32 * row_h;
    state.max_scroll = (total_h - (list_bot - list_top)).max(0.0);
    state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_scroll);

    ctx.with_axis_clip(rect(0.0, list_top, w, list_bot), || {
        for (idx, pr) in displayed_procs.iter().enumerate() {
        let y = list_top - state.scroll_offset + idx as f32 * row_h;
        if y + row_h < list_top || y > list_bot {
            continue;
        }
        let rr = rect(12.0, y, w - 12.0, y + row_h - 4.0);
        ctx.fill_rounded(rr, 7.0, BG_CARD)?;

        ctx.draw_status_dot(
            24.0,
            y + 14.0,
            3.0,
            if pr.is_critical_system {
                TEXT_MUTED
            } else {
                ACCENT_LIVE
            },
            false,
        )?;

        let title = format!("{} · {}", pr.name, pr.role_label);
        ctx.draw_text(
            &title,
            &fonts.row_primary,
            rect(34.0, y + 2.0, 270.0, y + 22.0),
            TEXT_PRIMARY,
            false,
        )?;

        let meta = if pr.access_limited {
            format!(
                "PID {} · {} · {}",
                pr.key.pid,
                format_uptime(pr.uptime_secs),
                lang.pick("elevated", "yükseltilmiş")
            )
        } else {
            format!("PID {} · {}", pr.key.pid, format_uptime(pr.uptime_secs))
        };
        ctx.draw_text(
            &meta,
            &fonts.row_secondary,
            rect(34.0, y + 21.0, 270.0, y + 39.0),
            TEXT_MUTED,
            true,
        )?;

        if pr.ports.len() == 1 {
            let port = &pr.ports[0];
            let p_rect = rect(274.0, y + 11.0, 330.0, y + 31.0);
            let pid_hit = next_id();
            ctx.fill_rounded(
                p_rect,
                5.0,
                if state.hovered_hit_id == Some(pid_hit) {
                    rgba(10, 132, 255, 0.28)
                } else {
                    ACCENT_INFO_BG
                },
            )?;
            ctx.draw_text(
                &format!(":{}↗", port.port),
                &fonts.mono_badge,
                p_rect,
                ACCENT_INFO,
                true,
            )?;
            let copy_url = port
                .suggested_url
                .clone()
                .unwrap_or_else(|| format!("http://localhost:{}", port.port));
            hit_map.push_with_rclick_clipped(
                pid_hit,
                p_rect,
                list_top,
                list_bot,
                HitAction::OpenPort {
                    port: port.port,
                    url: port.suggested_url.clone(),
                },
                HitAction::CopyText(copy_url),
            );
        } else if pr.ports.len() >= 2 {
            for (slot, port) in pr.ports.iter().take(2).enumerate() {
                let top = y + 3.0 + (slot as f32) * 20.0;
                let p_rect = rect(272.0, top, 330.0, top + 18.0);
                let pid_hit = next_id();
                let lbl = if slot == 1 && pr.ports.len() > 2 {
                    format!(":{} +{}", port.port, pr.ports.len() - 2)
                } else {
                    format!(":{}↗", port.port)
                };
                ctx.fill_rounded(
                    p_rect,
                    4.0,
                    if state.hovered_hit_id == Some(pid_hit) {
                        rgba(10, 132, 255, 0.28)
                    } else {
                        ACCENT_INFO_BG
                    },
                )?;
                ctx.draw_text(&lbl, &fonts.mono_badge, p_rect, ACCENT_INFO, true)?;
                let copy_url = port
                    .suggested_url
                    .clone()
                    .unwrap_or_else(|| format!("http://localhost:{}", port.port));
                hit_map.push_with_rclick_clipped(
                    pid_hit,
                    p_rect,
                    list_top,
                    list_bot,
                    HitAction::OpenPort {
                        port: port.port,
                        url: port.suggested_url.clone(),
                    },
                    HitAction::CopyText(copy_url),
                );
            }
        }

        ctx.draw_text(
            &format!("{:.1}%", pr.cpu),
            &fonts.mono_metric,
            rect(334.0, y + 4.0, 392.0, y + 22.0),
            TEXT_PRIMARY,
            true,
        )?;
        ctx.draw_text(
            &format_ram(pr.ram),
            &fonts.mono_metric,
            rect(334.0, y + 21.0, 392.0, y + 39.0),
            TEXT_SECONDARY,
            true,
        )?;

        let confirming = matches!(
            &state.pending_kill,
            Some(PendingKill::Process(pk)) if pk == &pr.key
        );
        if confirming {
            let ok_r = rect(w - 118.0, y + 9.0, w - 66.0, y + 33.0);
            ctx.fill_rounded(ok_r, 6.0, ACCENT_DANGER)?;
            ctx.draw_text(
                lang.pick("Kill", "Sonlandır"),
                &fonts.mono_badge,
                ok_r,
                BG_OVERLAY,
                false,
            )?;
            hit_map.push_clipped(next_id(), ok_r, list_top, list_bot, HitAction::ConfirmKillProcess(pr.key));

            let cn_r = rect(w - 62.0, y + 9.0, w - 18.0, y + 33.0);
            ctx.fill_rounded(cn_r, 6.0, BG_ELEVATED_HOVER)?;
            ctx.draw_text(
                lang.pick("No", "Hayır"),
                &fonts.mono_badge,
                cn_r,
                TEXT_PRIMARY,
                false,
            )?;
            hit_map.push_clipped(next_id(), cn_r, list_top, list_bot, HitAction::CancelKillConfirm);
        } else if pr.is_critical_system || !pr.can_kill {
            let lk_r = rect(w - 60.0, y + 9.0, w - 18.0, y + 33.0);
            ctx.fill_rounded(lk_r, 6.0, BG_OVERLAY)?;
            ctx.draw_text(
                lang.pick("LOCK", "KİLİT"),
                &fonts.mono_badge,
                lk_r,
                TEXT_MUTED,
                false,
            )?;
        } else {
            let k_r = rect(w - 44.0, y + 9.0, w - 18.0, y + 33.0);
            ctx.fill_rounded(k_r, 6.0, ACCENT_DANGER_BG)?;
            ctx.draw_text("■", &fonts.mono_badge, k_r, ACCENT_DANGER, false)?;
            hit_map.push_clipped(next_id(), k_r, list_top, list_bot, HitAction::RequestKillProcess(pr.key));
        }
    }
    Ok(())
    })?;

    Ok(())
}

