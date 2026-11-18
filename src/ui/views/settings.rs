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
    state: &UiState,
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
        "v0.1.0",
        &fonts.row_secondary,
        rect(w - 64.0, 8.0, w - 16.0, 34.0),
        TEXT_MUTED,
        true,
    )?;

    let mut y = 48.0;
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
    ctx.fill_rounded(en_btn, 7.0, if is_en { ACCENT_INFO_BG } else { BG_ELEVATED })?;
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
    hit_map.push(next_id(), en_btn, HitAction::SetLanguage(Language::En));

    ctx.fill_rounded(tr_btn, 7.0, if !is_en { ACCENT_INFO_BG } else { BG_ELEVATED })?;
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
    hit_map.push(next_id(), tr_btn, HitAction::SetLanguage(Language::Tr));
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
    ctx.fill_rounded(top_btn, 7.0, if is_top { ACCENT_INFO_BG } else { BG_ELEVATED })?;
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
    hit_map.push(next_id(), top_btn, HitAction::SetDockPosition(DockPosition::TopCenter));

    ctx.fill_rounded(
        right_btn,
        7.0,
        if !is_top { ACCENT_INFO_BG } else { BG_ELEVATED },
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
    hit_map.push(next_id(), right_btn, HitAction::SetDockPosition(DockPosition::RightEdge));
    y += 38.0;

    ctx.draw_text(
        lang.pick("Monitor", "Monitör"),
        &fonts.row_primary,
        rect(16.0, y, 220.0, y + 30.0),
        TEXT_PRIMARY,
        false,
    )?;
    let mon_label = monitors
        .get(cfg.monitor_index)
        .or_else(|| monitors.first())
        .map(|m| {
            let disp = lang.pick("Display", "Ekran");
            let prim = lang.pick("Primary", "Birincil");
            if m.is_primary {
                format!("{disp} {} ({prim})", cfg.monitor_index + 1)
            } else {
                format!("{disp} {}", cfg.monitor_index + 1)
            }
        })
        .unwrap_or_else(|| lang.pick("Primary", "Birincil").to_string());
    let mon_btn = rect(w - 232.0, y + 2.0, w - 16.0, y + 28.0);
    ctx.fill_rounded(mon_btn, 7.0, BG_ELEVATED)?;
    ctx.draw_text(&mon_label, &fonts.mono_badge, mon_btn, TEXT_PRIMARY, true)?;
    hit_map.push(next_id(), mon_btn, HitAction::CycleMonitor);
    y += 38.0;

    draw_toggle_row(
        ctx,
        fonts,
        w,
        y,
        lang.pick("Always on Top", "Her Zaman Üstte"),
        cfg.always_on_top,
        lang,
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
    ctx.fill_rounded(minus_idle, 6.0, BG_ELEVATED)?;
    ctx.draw_text("−", &fonts.mono_badge, minus_idle, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), minus_idle, HitAction::AdjustIdleOpacity(-0.07));
    let idle_txt = if cfg.idle_opacity <= 0.02 {
        lang.pick("0% (Transparent)", "0% (Şeffaf)").to_string()
    } else {
        format!("{:.0}%", cfg.idle_opacity * 100.0)
    };
    ctx.draw_text(
        &idle_txt,
        &fonts.mono_badge,
        val_idle,
        TEXT_PRIMARY,
        true,
    )?;
    ctx.fill_rounded(plus_idle, 6.0, BG_ELEVATED)?;
    ctx.draw_text("+", &fonts.mono_badge, plus_idle, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), plus_idle, HitAction::AdjustIdleOpacity(0.07));
    y += 34.0;

    draw_toggle_row(
        ctx,
        fonts,
        w,
        y,
        lang.pick("Acrylic Blur", "Acrylic Cam Bulanıklığı"),
        cfg.acrylic_enabled,
        lang,
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
    ctx.fill_rounded(minus_op, 6.0, BG_ELEVATED)?;
    ctx.draw_text("−", &fonts.mono_badge, minus_op, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), minus_op, HitAction::AdjustOpacity(-0.05));
    ctx.draw_text(
        &format!("{:.0}%", cfg.glass_opacity * 100.0),
        &fonts.mono_badge,
        val_op,
        TEXT_PRIMARY,
        true,
    )?;
    ctx.fill_rounded(plus_op, 6.0, BG_ELEVATED)?;
    ctx.draw_text("+", &fonts.mono_badge, plus_op, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), plus_op, HitAction::AdjustOpacity(0.05));
    y += 34.0;

    draw_toggle_row(
        ctx,
        fonts,
        w,
        y,
        lang.pick("Hide on Fullscreen", "Tam Ekranda Gizle"),
        cfg.hide_on_fullscreen,
        lang,
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
    let hk_btn = rect(w - 180.0, y + 2.0, w - 16.0, y + 28.0);
    ctx.fill_rounded(hk_btn, 7.0, BG_ELEVATED)?;
    ctx.draw_text(cfg.hotkey.label(), &fonts.mono_badge, hk_btn, ACCENT_INFO, false)?;
    hit_map.push(next_id(), hk_btn, HitAction::ToggleHotkeyPreset);
    y += 36.0;

    draw_toggle_row(
        ctx,
        fonts,
        w,
        y,
        lang.pick("Start with Windows", "Windows ile Başlat"),
        cfg.autostart,
        lang,
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
        lang.pick("Crash Notifications", "Çökme Bildirimi"),
        cfg.notify_on_crash,
        lang,
        hit_map,
        &mut next_id,
        HitAction::ToggleCrashNotify,
    )?;
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
    ctx.fill_rounded(minus_d, 6.0, BG_ELEVATED)?;
    ctx.draw_text("−", &fonts.mono_badge, minus_d, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), minus_d, HitAction::AdjustPeekDelay(-30));
    ctx.draw_text(
        &format!("{} ms", cfg.hover_peek_delay_ms),
        &fonts.mono_badge,
        val_d,
        TEXT_PRIMARY,
        true,
    )?;
    ctx.fill_rounded(plus_d, 6.0, BG_ELEVATED)?;
    ctx.draw_text("+", &fonts.mono_badge, plus_d, TEXT_PRIMARY, false)?;
    hit_map.push(next_id(), plus_d, HitAction::AdjustPeekDelay(30));
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
        ctx.fill_rounded(restore_btn, 7.0, ACCENT_INFO_BG)?;
        ctx.draw_text(
            &format!("{} ({hidden_cnt})", lang.pick("Show All", "Tümünü Göster")),
            &fonts.mono_badge,
            restore_btn,
            ACCENT_INFO,
            false,
        )?;
        hit_map.push(next_id(), restore_btn, HitAction::RestoreHiddenProjects);
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

    let fy = h - 26.0;
    ctx.draw_hairline(16.0, fy, w - 16.0)?;
    ctx.draw_text(
        &format!(
            "{}: {}",
            lang.pick("Config", "Yapılandırma"),
            AppConfig::config_path().display()
        ),
        &fonts.row_secondary,
        rect(16.0, fy + 3.0, w - 16.0, h - 2.0),
        TEXT_MUTED,
        false,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_toggle_row<F: FnMut() -> u32>(
    ctx: &DrawCtx<'_>,
    fonts: &FontSet,
    w: f32,
    y: f32,
    label: &str,
    on: bool,
    lang: Language,
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
    ctx.fill_rounded(btn, 7.0, if on { ACCENT_LIVE_GLOW } else { BG_ELEVATED })?;
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
    hit_map.push(next_id(), btn, action);
    Ok(())
}
