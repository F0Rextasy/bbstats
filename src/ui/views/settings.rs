use super::rect;
use crate::config::{AppConfig, DockPosition};
use crate::model::Language;
use crate::platform::monitor::MonitorInfo;
use crate::render::device::FontSet;
use crate::render::primitives::DrawCtx;
use crate::render::theme::*;
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::UiState;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_settings(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    h: f32,
    state: &mut UiState,
    cfg: &AppConfig,
    monitors: &[MonitorInfo],
    hit_map: &mut HitMap,
) -> windows::core::Result<()> {
    let lang = cfg.language;
    let mut id_gen = 800u32;
    let mut next_id = || {
        let v = id_gen;
        id_gen += 1;
        v
    };

    let back_rect = rect(14.0, 9.0, 74.0, 33.0);
    let bid = next_id();
    if state.hovered_hit_id == Some(bid) {
        ctx.fill_rounded(back_rect, 7.0, BG_ELEVATED_HOVER)?;
    }
    ctx.draw_text(
        lang.pick("← Back", "← Geri"),
        &fonts.mono_badge,
        back_rect,
        ACCENT_INFO,
        false,
    )?;
    hit_map.push(bid, back_rect, HitAction::BackToExpanded);

    ctx.draw_text(
        lang.pick("Settings", "Ayarlar"),
        &fonts.title_center,
        rect(120.0, 8.0, w - 120.0, 34.0),
        TEXT_PRIMARY,
        false,
    )?;
    ctx.draw_text(
        concat!("v", env!("CARGO_PKG_VERSION")),
        &fonts.row_secondary,
        rect(w - 64.0, 8.0, w - 16.0, 34.0),
        TEXT_MUTED,
        true,
    )?;

    let vp_top = 44.0;
    let vp_bot = (h - 28.0).max(vp_top + 40.0);
    let vp = rect(0.0, vp_top, w, vp_bot);
    hit_map.scroll_viewport = Some(vp);

    let mut total_content = 0.0_f32;
    ctx.with_axis_clip(vp, || {
        let mut y = 48.0 - state.scroll_offset;
        let btn_col = |hovered: bool| if hovered { BG_ELEVATED_HOVER } else { BG_ELEVATED };

        ctx.draw_section_divider(
            16.0,
            w - 16.0,
            y,
            lang.pick("LAYOUT & APPEARANCE", "KONUM VE GÖRÜNÜM"),
            &fonts.section_hdr_center,
        )?;
        y += 14.0;

        ctx.draw_text(
            lang.pick("Language", "Dil"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let en_btn = rect(w - 232.0, y + 2.0, w - 126.0, y + 28.0);
        let tr_btn = rect(w - 122.0, y + 2.0, w - 16.0, y + 28.0);
        let is_en = lang == Language::En;
        let en_id = next_id();
        ctx.fill_rounded(
            en_btn,
            7.0,
            if is_en { ACCENT_INFO_BG } else { btn_col(state.hovered_hit_id == Some(en_id)) },
        )?;
        if is_en {
            ctx.stroke_rounded(en_btn, 7.0, ACCENT_INFO, 1.0)?;
        }
        ctx.draw_text(
            "English",
            &fonts.mono_badge,
            en_btn,
            if is_en { ACCENT_INFO } else { TEXT_SECONDARY },
            false,
        )?;
        hit_map.push_clipped(en_id, en_btn, vp_top, vp_bot, HitAction::SetLanguage(Language::En));

        let tr_id = next_id();
        ctx.fill_rounded(
            tr_btn,
            7.0,
            if !is_en { ACCENT_INFO_BG } else { btn_col(state.hovered_hit_id == Some(tr_id)) },
        )?;
        if !is_en {
            ctx.stroke_rounded(tr_btn, 7.0, ACCENT_INFO, 1.0)?;
        }
        ctx.draw_text(
            "Türkçe",
            &fonts.mono_badge,
            tr_btn,
            if !is_en { ACCENT_INFO } else { TEXT_SECONDARY },
            false,
        )?;
        hit_map.push_clipped(tr_id, tr_btn, vp_top, vp_bot, HitAction::SetLanguage(Language::Tr));
        y += 36.0;

        ctx.draw_text(
            lang.pick("Dock Position", "Yerleşim Konumu"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let top_btn = rect(w - 232.0, y + 2.0, w - 126.0, y + 28.0);
        let right_btn = rect(w - 122.0, y + 2.0, w - 16.0, y + 28.0);
        let is_top = cfg.dock_position == DockPosition::TopCenter;
        let top_id = next_id();
        ctx.fill_rounded(
            top_btn,
            7.0,
            if is_top { ACCENT_INFO_BG } else { btn_col(state.hovered_hit_id == Some(top_id)) },
        )?;
        if is_top {
            ctx.stroke_rounded(top_btn, 7.0, ACCENT_INFO, 1.0)?;
        }
        ctx.draw_text(
            lang.pick("Top Center", "Üst Orta"),
            &fonts.mono_badge,
            top_btn,
            if is_top { ACCENT_INFO } else { TEXT_SECONDARY },
            false,
        )?;
        hit_map.push_clipped(
            top_id,
            top_btn,
            vp_top,
            vp_bot,
            HitAction::SetDockPosition(DockPosition::TopCenter),
        );

        let right_id = next_id();
        ctx.fill_rounded(
            right_btn,
            7.0,
            if !is_top { ACCENT_INFO_BG } else { btn_col(state.hovered_hit_id == Some(right_id)) },
        )?;
        if !is_top {
            ctx.stroke_rounded(right_btn, 7.0, ACCENT_INFO, 1.0)?;
        }
        ctx.draw_text(
            lang.pick("Right Edge", "Sağ Kenar"),
            &fonts.mono_badge,
            right_btn,
            if !is_top { ACCENT_INFO } else { TEXT_SECONDARY },
            false,
        )?;
        hit_map.push_clipped(
            right_id,
            right_btn,
            vp_top,
            vp_bot,
            HitAction::SetDockPosition(DockPosition::RightEdge),
        );
        y += 38.0;

        ctx.draw_text(
            lang.pick("Monitor", "Monitör"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let mon_label = format_monitor_label(monitors, cfg.monitor_index, lang);
        let mon_btn = rect(w - 232.0, y + 2.0, w - 16.0, y + 28.0);
        let mon_id = next_id();
        ctx.fill_rounded(mon_btn, 7.0, btn_col(state.hovered_hit_id == Some(mon_id)))?;
        ctx.draw_text(&mon_label, &fonts.mono_badge, mon_btn, TEXT_PRIMARY, true)?;
        hit_map.push_clipped(mon_id, mon_btn, vp_top, vp_bot, HitAction::CycleMonitor);
        y += 38.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("Always on Top", "Her Zaman Üstte"),
            cfg.always_on_top,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleAlwaysOnTop,
        )?;
        y += 34.0;

        ctx.draw_text(
            lang.pick("Idle Opacity (HUD)", "Boşta Opaklık (Şeffaf HUD)"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let minus_idle = rect(w - 192.0, y + 2.0, w - 158.0, y + 28.0);
        let val_idle = rect(w - 154.0, y + 2.0, w - 54.0, y + 28.0);
        let plus_idle = rect(w - 50.0, y + 2.0, w - 16.0, y + 28.0);
        let mi_id = next_id();
        ctx.fill_rounded(minus_idle, 6.0, btn_col(state.hovered_hit_id == Some(mi_id)))?;
        ctx.draw_text("−", &fonts.mono_badge, minus_idle, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(mi_id, minus_idle, vp_top, vp_bot, HitAction::AdjustIdleOpacity(-0.07));
        let idle_txt = if cfg.idle_opacity <= 0.02 {
            lang.pick("0% (Transparent)", "0% (Şeffaf)").to_string()
        } else {
            format!("{:.0}%", cfg.idle_opacity * 100.0)
        };
        ctx.draw_text(&idle_txt, &fonts.mono_badge, val_idle, TEXT_PRIMARY, true)?;
        let pi_id = next_id();
        ctx.fill_rounded(plus_idle, 6.0, btn_col(state.hovered_hit_id == Some(pi_id)))?;
        ctx.draw_text("+", &fonts.mono_badge, plus_idle, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(pi_id, plus_idle, vp_top, vp_bot, HitAction::AdjustIdleOpacity(0.07));
        y += 34.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("Acrylic Blur", "Acrylic Cam Bulanıklığı"),
            cfg.acrylic_enabled,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleAcrylic,
        )?;
        y += 34.0;

        ctx.draw_text(
            lang.pick("Expanded Opacity", "Açık Panel Opaklığı"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let minus_op = rect(w - 192.0, y + 2.0, w - 158.0, y + 28.0);
        let val_op = rect(w - 154.0, y + 2.0, w - 54.0, y + 28.0);
        let plus_op = rect(w - 50.0, y + 2.0, w - 16.0, y + 28.0);
        let mo_id = next_id();
        ctx.fill_rounded(minus_op, 6.0, btn_col(state.hovered_hit_id == Some(mo_id)))?;
        ctx.draw_text("−", &fonts.mono_badge, minus_op, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(mo_id, minus_op, vp_top, vp_bot, HitAction::AdjustOpacity(-0.05));
        ctx.draw_text(
            &format!("{:.0}%", cfg.glass_opacity * 100.0),
            &fonts.mono_badge,
            val_op,
            TEXT_PRIMARY,
            true,
        )?;
        let po_id = next_id();
        ctx.fill_rounded(plus_op, 6.0, btn_col(state.hovered_hit_id == Some(po_id)))?;
        ctx.draw_text("+", &fonts.mono_badge, plus_op, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(po_id, plus_op, vp_top, vp_bot, HitAction::AdjustOpacity(0.05));
        y += 34.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("Hide on Fullscreen", "Tam Ekranda Gizle"),
            cfg.hide_on_fullscreen,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleFullscreenHide,
        )?;
        y += 42.0;

        ctx.draw_section_divider(
            16.0,
            w - 16.0,
            y,
            lang.pick("BEHAVIOR & STARTUP", "DAVRANIŞ VE BAŞLANGIÇ"),
            &fonts.section_hdr_center,
        )?;
        y += 14.0;

        ctx.draw_text(
            lang.pick("Global Shortcut", "Genel Kısayol"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        if state.hotkey_failed {
            ctx.draw_text(
                lang.pick("Conflict!", "Çakışma!"),
                &fonts.mono_badge,
                rect(w - 254.0, y + 2.0, w - 184.0, y + 28.0),
                ACCENT_DANGER,
                false,
            )?;
        }
        let hk_btn = rect(w - 180.0, y + 2.0, w - 16.0, y + 28.0);
        let hk_id = next_id();
        ctx.fill_rounded(hk_btn, 7.0, btn_col(state.hovered_hit_id == Some(hk_id)))?;
        ctx.draw_text(cfg.hotkey.label(lang), &fonts.mono_badge, hk_btn, ACCENT_INFO, false)?;
        hit_map.push_clipped(hk_id, hk_btn, vp_top, vp_bot, HitAction::ToggleHotkeyPreset);
        y += 36.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("Start with Windows", "Windows ile Başlat"),
            cfg.autostart,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleAutostart,
        )?;
        y += 36.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("Crash Notifications", "Çökme Bildirimi"),
            cfg.notify_on_crash,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleCrashNotify,
        )?;
        y += 36.0;

        draw_toggle_row(
            ctx,
            fonts,
            w,
            y,
            vp_top,
            vp_bot,
            lang.pick("WSL2 Process Bridge", "WSL2 Süreç Köprüsü"),
            cfg.wsl_bridge_enabled,
            lang,
            state,
            hit_map,
            &mut next_id,
            HitAction::ToggleWslBridge,
        )?;
        y += 36.0;

        ctx.draw_text(
            lang.pick("CPU Alert Threshold", "CPU Uyarı Eşiği"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let minus_cpu = rect(w - 192.0, y + 2.0, w - 158.0, y + 28.0);
        let val_cpu = rect(w - 154.0, y + 2.0, w - 54.0, y + 28.0);
        let plus_cpu = rect(w - 50.0, y + 2.0, w - 16.0, y + 28.0);
        let mc_id = next_id();
        ctx.fill_rounded(minus_cpu, 6.0, btn_col(state.hovered_hit_id == Some(mc_id)))?;
        ctx.draw_text("−", &fonts.mono_badge, minus_cpu, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(mc_id, minus_cpu, vp_top, vp_bot, HitAction::AdjustCpuWarn(-5.0));
        ctx.draw_text(
            &format!("{:.0}%", cfg.cpu_warn_percent),
            &fonts.mono_badge,
            val_cpu,
            TEXT_PRIMARY,
            true,
        )?;
        let pc_id = next_id();
        ctx.fill_rounded(plus_cpu, 6.0, btn_col(state.hovered_hit_id == Some(pc_id)))?;
        ctx.draw_text("+", &fonts.mono_badge, plus_cpu, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(pc_id, plus_cpu, vp_top, vp_bot, HitAction::AdjustCpuWarn(5.0));
        y += 34.0;

        ctx.draw_text(
            lang.pick("RAM Spike Threshold", "RAM Sıçrama Eşiği"),
            &fonts.row_primary,
            rect(16.0, y, 220.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let minus_ram = rect(w - 192.0, y + 2.0, w - 158.0, y + 28.0);
        let val_ram = rect(w - 154.0, y + 2.0, w - 54.0, y + 28.0);
        let plus_ram = rect(w - 50.0, y + 2.0, w - 16.0, y + 28.0);
        let mr_id = next_id();
        ctx.fill_rounded(minus_ram, 6.0, btn_col(state.hovered_hit_id == Some(mr_id)))?;
        ctx.draw_text("−", &fonts.mono_badge, minus_ram, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(mr_id, minus_ram, vp_top, vp_bot, HitAction::AdjustRamWarn(-256));
        ctx.draw_text(
            &format!("{} MB", cfg.ram_warn_mb),
            &fonts.mono_badge,
            val_ram,
            TEXT_PRIMARY,
            true,
        )?;
        let pr_id = next_id();
        ctx.fill_rounded(plus_ram, 6.0, btn_col(state.hovered_hit_id == Some(pr_id)))?;
        ctx.draw_text("+", &fonts.mono_badge, plus_ram, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(pr_id, plus_ram, vp_top, vp_bot, HitAction::AdjustRamWarn(256));
        y += 36.0;

        ctx.draw_text(
            lang.pick("Hover Peek Delay", "Hover Önizleme Gecikmesi"),
            &fonts.row_primary,
            rect(16.0, y, 240.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let minus_d = rect(w - 192.0, y + 2.0, w - 158.0, y + 28.0);
        let val_d = rect(w - 154.0, y + 2.0, w - 54.0, y + 28.0);
        let plus_d = rect(w - 50.0, y + 2.0, w - 16.0, y + 28.0);
        let md_id = next_id();
        ctx.fill_rounded(minus_d, 6.0, btn_col(state.hovered_hit_id == Some(md_id)))?;
        ctx.draw_text("−", &fonts.mono_badge, minus_d, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(md_id, minus_d, vp_top, vp_bot, HitAction::AdjustPeekDelay(-30));
        ctx.draw_text(
            &format!("{} ms", cfg.hover_peek_delay_ms),
            &fonts.mono_badge,
            val_d,
            TEXT_PRIMARY,
            true,
        )?;
        let pd_id = next_id();
        ctx.fill_rounded(plus_d, 6.0, btn_col(state.hovered_hit_id == Some(pd_id)))?;
        ctx.draw_text("+", &fonts.mono_badge, plus_d, TEXT_PRIMARY, false)?;
        hit_map.push_clipped(pd_id, plus_d, vp_top, vp_bot, HitAction::AdjustPeekDelay(30));
        y += 36.0;

        ctx.draw_text(
            lang.pick("Hidden Projects", "Gizlenen Projeler"),
            &fonts.row_primary,
            rect(16.0, y, 250.0, y + 30.0),
            TEXT_PRIMARY,
            false,
        )?;
        let hidden_cnt = cfg.hidden_projects.len();
        let restore_btn = rect(w - 156.0, y + 2.0, w - 16.0, y + 28.0);
        if hidden_cnt > 0 {
            let rst_id = next_id();
            ctx.fill_rounded(
                restore_btn,
                7.0,
                if state.hovered_hit_id == Some(rst_id) {
                    rgba(10, 132, 255, 0.28)
                } else {
                    ACCENT_INFO_BG
                },
            )?;
            ctx.draw_text(
                &format!("{} ({hidden_cnt})", lang.pick("Show All", "Tümünü Göster")),
                &fonts.mono_badge,
                restore_btn,
                ACCENT_INFO,
                false,
            )?;
            hit_map.push_clipped(rst_id, restore_btn, vp_top, vp_bot, HitAction::RestoreHiddenProjects);
        } else {
            ctx.fill_rounded(restore_btn, 7.0, BG_ELEVATED)?;
            ctx.draw_text(
                lang.pick("None (0)", "Yok (0)"),
                &fonts.mono_badge,
                restore_btn,
                TEXT_MUTED,
                false,
            )?;
        }
        y += 34.0;
        total_content = (y + state.scroll_offset) - 48.0 + 12.0;
        Ok(())
    })?;

    state.max_scroll = (total_content - (vp_bot - vp_top)).max(0.0);
    state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_scroll);

    let fy = h - 26.0;
    ctx.draw_hairline(16.0, fy, w - 16.0)?;
    let cfg_rect = rect(16.0, fy + 2.0, w - 16.0, h - 2.0);
    let cfg_id = next_id();
    ctx.draw_text(
        &format!(
            "{}: {} ↗",
            lang.pick("Config", "Yapılandırma"),
            AppConfig::config_path().display()
        ),
        &fonts.row_secondary,
        cfg_rect,
        if state.hovered_hit_id == Some(cfg_id) {
            ACCENT_INFO
        } else {
            TEXT_MUTED
        },
        false,
    )?;
    hit_map.push(cfg_id, cfg_rect, HitAction::OpenConfigDir);

    Ok(())
}

pub(crate) fn format_monitor_label(
    monitors: &[MonitorInfo],
    monitor_index: usize,
    lang: Language,
) -> String {
    monitors
        .iter()
        .enumerate()
        .nth(monitor_index)
        .or_else(|| monitors.iter().enumerate().next())
        .map(|(shown_idx, m)| {
            let disp = lang.pick("Display", "Ekran");
            let prim = lang.pick("Primary", "Birincil");
            if m.is_primary {
                format!("{disp} {} ({prim})", shown_idx + 1)
            } else {
                format!("{disp} {}", shown_idx + 1)
            }
        })
        .unwrap_or_else(|| lang.pick("Primary", "Birincil").to_string())
}

#[allow(clippy::too_many_arguments)]
fn draw_toggle_row<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    y: f32,
    vp_top: f32,
    vp_bot: f32,
    label: &str,
    on: bool,
    lang: Language,
    state: &UiState,
    hit_map: &mut HitMap,
    next_id: &mut F,
    action: HitAction,
) -> windows::core::Result<()> {
    ctx.draw_text(
        label,
        &fonts.row_primary,
        rect(16.0, y, 280.0, y + 30.0),
        TEXT_PRIMARY,
        false,
    )?;
    let btn = rect(w - 116.0, y + 2.0, w - 16.0, y + 28.0);
    let tid = next_id();
    let bg = if on {
        ACCENT_LIVE_GLOW
    } else if state.hovered_hit_id == Some(tid) {
        BG_ELEVATED_HOVER
    } else {
        BG_ELEVATED
    };
    ctx.fill_rounded(btn, 7.0, bg)?;
    if on {
        ctx.stroke_rounded(btn, 7.0, ACCENT_LIVE, 1.0)?;
    }
    ctx.draw_text(
        if on {
            lang.pick("● On", "● Açık")
        } else {
            lang.pick("○ Off", "○ Kapalı")
        },
        &fonts.mono_badge,
        btn,
        if on { ACCENT_LIVE } else { TEXT_SECONDARY },
        false,
    )?;
    hit_map.push_clipped(tid, btn, vp_top, vp_bot, action);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_monitor_label;
    use crate::model::Language;
    use crate::platform::monitor::MonitorInfo;
    use windows::Win32::Foundation::RECT;

    fn fake_monitor(primary: bool) -> MonitorInfo {
        let rc = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        MonitorInfo {
            monitor_rect: rc,
            work_rect: RECT {
                bottom: 1040,
                ..rc
            },
            dpi: 96,
            is_primary: primary,
        }
    }

    #[test]
    fn format_monitor_label_uses_resolved_fallback_index() {
        let mons = vec![fake_monitor(true)];
        assert_eq!(
            format_monitor_label(&mons, 4, Language::En),
            "Display 1 (Primary)"
        );
        assert_eq!(
            format_monitor_label(&[], 2, Language::Tr),
            "Birincil"
        );
    }
}
