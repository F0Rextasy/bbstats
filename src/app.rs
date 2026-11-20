//! Core application state machine coordinating Win32 events, springs, Direct2D rendering, and the engine.

use crate::config::{autostart, AppConfig, DockPosition, HotkeyPreset};
use crate::engine::EngineHandle;
use crate::model::{Alert, AlertKind, EngineCmd, PollingMode, Snapshot, SortMode};
use crate::platform::dpi::{dip_to_px, is_reduced_motion, window_dpi};
use crate::platform::fullscreen::is_fullscreen_active;
use crate::platform::hotkey::register_hotkey;
use crate::platform::monitor::{
    compute_dock_rect, enumerate_monitors, monitor_work_height_dip, select_monitor, MonitorInfo,
};
use crate::platform::tray::{TrayAction, TrayIcon};
use crate::platform::window::{is_foreground_window, set_window_activatable, set_window_topmost};
use crate::render::compositor::CompositionHost;
use crate::render::device::RenderDevice;
use crate::render::primitives::DrawCtx;
use crate::render::theme::{SPRING_ALERT, SPRING_MORPH};
use crate::ui::hit_test::{HitAction, HitMap};
use crate::ui::layout::{
    ensure_selected_job_visible, filter_project_jobs, format_ram, pop_last_word, target_dimensions,
    AddJobDraft, PendingKill, RenameDraft, UiState, ViewMode,
};
use crate::ui::spring::SpringVal;
use crate::ui::views::{render_ui, MorphParams};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};

const D2DERR_RECREATE_TARGET: HRESULT = HRESULT(0x8899_000C_u32 as i32);

#[inline]
fn is_device_lost_error(code: HRESULT) -> bool {
    code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
        || code == D2DERR_RECREATE_TARGET
}

fn collect_unnotified_crash_alerts<'a>(
    alerts: &'a [Alert],
    last_id: &mut u64,
) -> Vec<&'a Alert> {
    let mut out = Vec::new();
    for a in alerts.iter().rev() {
        if !a.dismissed && a.id > *last_id {
            *last_id = a.id;
            if matches!(a.kind, AlertKind::Crash { .. }) {
                out.push(a);
            }
        }
    }
    out
}
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, SetProcessWorkingSetSizeEx, GMEM_MOVEABLE,
    SETPROCESSWORKINGSETSIZEEX_FLAGS,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_A, VK_BACK, VK_C,
    VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_K, VK_LEFT, VK_MENU, VK_NEXT, VK_P, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_TAB, VK_U, VK_UP, VK_V,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostQuitMessage, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
};

pub const TIMER_ANIM: usize = 1;
pub const TIMER_HOVER_PEEK: usize = 2;
pub const TIMER_LEAVE_COLLAPSE: usize = 3;
pub const TIMER_TRIM: usize = 4;
pub const TIMER_FULLSCREEN: usize = 5;

pub struct App {
    pub hwnd: HWND,
    pub cfg: AppConfig,
    pub ui: UiState,
    pub snap: Arc<Snapshot>,
    pub engine: EngineHandle,
    pub device: RenderDevice,
    pub comp: CompositionHost,
    pub tray: TrayIcon,
    pub monitors: Vec<MonitorInfo>,
    pub dpi: u32,
    pub reduced_motion: bool,
    pub fullscreen_hidden: bool,
    pub hit_map: HitMap,

    pub x_px: SpringVal,
    pub y_px: SpringVal,
    pub w_dip: SpringVal,
    pub h_dip: SpringVal,
    pub radius_dip: SpringVal,
    pub morph_t: SpringVal,
    pub tab_t: SpringVal,
    pub bg_alpha: SpringVal,
    pub prev_mode: Option<(ViewMode, f32, f32)>,
    pub win_rect_px: (i32, i32, i32, i32),

    animating: bool,
    last_anim_tick: Instant,
    last_hover_render: Instant,
    mouse_tracked: bool,
    last_mouse_dip: (f32, f32),
    last_collapsed_sig: Option<u64>,
    last_notified_alert_id: u64,
    last_seen_action_error_id: u64,
    pub menu_open: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hwnd: HWND,
        cfg: AppConfig,
        engine: EngineHandle,
        device: RenderDevice,
        comp: CompositionHost,
        tray: TrayIcon,
        monitors: Vec<MonitorInfo>,
        init_rect: (i32, i32, i32, i32),
        init_w_dip: f32,
        init_h_dip: f32,
        init_r_dip: f32,
    ) -> Self {
        let snap = engine.latest();
        let dpi = window_dpi(hwnd);
        let init_idle = cfg.idle_opacity;
        let mut app = Self {
            hwnd,
            cfg,
            ui: UiState::default(),
            snap,
            engine,
            device,
            comp,
            tray,
            monitors,
            dpi,
            reduced_motion: is_reduced_motion(),
            fullscreen_hidden: false,
            hit_map: HitMap::default(),
            x_px: SpringVal::new(init_rect.0 as f32),
            y_px: SpringVal::new(init_rect.1 as f32),
            w_dip: SpringVal::new(init_w_dip),
            h_dip: SpringVal::new(init_h_dip),
            radius_dip: SpringVal::new(init_r_dip),
            morph_t: SpringVal::new(1.0),
            tab_t: SpringVal::new(1.0),
            bg_alpha: SpringVal::new(init_idle),
            prev_mode: None,
            win_rect_px: init_rect,
            animating: false,
            last_anim_tick: Instant::now(),
            last_hover_render: Instant::now(),
            mouse_tracked: false,
            last_mouse_dip: (0.0, 0.0),
            last_collapsed_sig: None,
            last_notified_alert_id: 0,
            last_seen_action_error_id: 0,
            menu_open: false,
        };
        if let Some(ref err) = app.cfg.load_error {
            crate::config::log_error(err);
            let msg = app
                .cfg
                .language
                .pick(
                    "Corrupt config.toml backed up to .bak",
                    "Bozuk config.toml .bak olarak yedeklendi",
                )
                .to_string();
            app.set_toast(msg);
        }
        app.sync_engine_intervals();
        app.sync_config_to_engine();
        set_window_topmost(app.hwnd, app.cfg.always_on_top);
        let _ = app.render_frame();
        app.sync_fullscreen_timer();
        unsafe {
            SetTimer(hwnd, TIMER_TRIM, 1500, None);
        }
        app
    }

    fn set_toast(&mut self, msg: String) {
        self.ui.toast = Some((msg, Instant::now()));
    }

    fn save_cfg_or_toast(&mut self) {
        if let Err(err) = self.cfg.save() {
            crate::config::log_error(&format!("config save failed: {err}"));
            let msg = self
                .cfg
                .language
                .pick("Could not save settings.", "Ayarlar kaydedilemedi.")
                .to_string();
            self.set_toast(msg);
        }
    }

    pub fn on_snapshot(&mut self) {
        self.snap = self.engine.latest();
        let toast_expired = self
            .ui
            .toast
            .as_ref()
            .is_some_and(|(_, ts)| ts.elapsed().as_secs() >= 5);
        if toast_expired {
            self.ui.toast = None;
        }
        let unnotified =
            collect_unnotified_crash_alerts(&self.snap.alerts, &mut self.last_notified_alert_id);
        if self.cfg.notify_on_crash {
            for a in unnotified {
                self.tray.show_balloon(
                    self.cfg
                        .language
                        .pick("bbstats — Process Crashed", "bbstats — Süreç Çöktü"),
                    &a.message,
                );
            }
        }
        if let Some((err_id, ref err_msg)) = self.snap.last_action_error {
            if err_id > self.last_seen_action_error_id {
                self.last_seen_action_error_id = err_id;
                self.set_toast(err_msg.clone());
            }
        }
        if matches!(self.ui.mode, ViewMode::Collapsed) && !self.animating {
            let sig = collapsed_visual_sig(&self.snap);
            if self.last_collapsed_sig == Some(sig) && !toast_expired {
                return;
            }
            self.last_collapsed_sig = Some(sig);
        } else {
            self.last_collapsed_sig = None;
        }
        self.recompute_targets(false);
        let _ = self.render_frame();
        if matches!(self.ui.mode, ViewMode::Collapsed) && !self.animating {
            unsafe {
                SetTimer(self.hwnd, TIMER_TRIM, 900, None);
            }
        }
    }

    pub fn set_mode(&mut self, mode: ViewMode, activate: bool) {
        if self.ui.mode == mode {
            return;
        }
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_HOVER_PEEK);
            let _ = KillTimer(self.hwnd, TIMER_LEAVE_COLLAPSE);
            let _ = KillTimer(self.hwnd, TIMER_TRIM);
        }
        self.last_collapsed_sig = None;
        let old_mode = self.ui.mode.clone();
        let old_w = self.w_dip.value;
        let old_h = self.h_dip.value;

        self.ui.mode = mode.clone();
        self.ui.pending_kill = None;
        self.ui.text_all_selected = false;
        self.ui.scroll_offset = 0.0;
        self.ui.term_scroll_offset = 0.0;
        self.ui.unpinned_auto_collapse = false;
        let interactive = !matches!(mode, ViewMode::Collapsed | ViewMode::HoverPeek);
        set_window_activatable(self.hwnd, interactive);
        if !interactive {
            self.ui.search_focused = false;
            self.ui.sticky = false;
            self.ui.rename_draft = None;
            self.ui.add_job_draft = None;
        }

        let same_detail_proj = match (&old_mode, &mode) {
            (
                ViewMode::Detail {
                    project_id: p1,
                    job_id: j1,
                },
                ViewMode::Detail {
                    project_id: p2,
                    job_id: j2,
                },
            ) => p1 == p2 && j1 != j2,
            _ => false,
        };
        if same_detail_proj && !self.reduced_motion {
            let dir = match (&old_mode, &mode) {
                (
                    ViewMode::Detail {
                        project_id: p1,
                        job_id: old_j,
                    },
                    ViewMode::Detail { job_id: new_j, .. },
                ) => self
                    .snap
                    .projects
                    .iter()
                    .find(|p| &p.id == p1)
                    .map(|p| {
                        let old_idx = old_j
                            .as_ref()
                            .and_then(|id| p.jobs.iter().position(|j| &j.id == id))
                            .unwrap_or(0);
                        let new_idx = new_j
                            .as_ref()
                            .and_then(|id| p.jobs.iter().position(|j| &j.id == id))
                            .unwrap_or(0);
                        if new_idx >= old_idx { 1.0 } else { -1.0 }
                    })
                    .unwrap_or(1.0),
                _ => 1.0,
            };
            self.ui.detail_tab_dir = dir;
            self.prev_mode = None;
            self.morph_t.snap(1.0);
            self.tab_t.snap(0.0);
            self.tab_t.set_target(1.0);
            self.ui.detail_tab_t = 0.0;
        } else if self.reduced_motion {
            self.prev_mode = None;
            self.morph_t.snap(1.0);
            self.tab_t.snap(1.0);
            self.ui.detail_tab_t = 1.0;
        } else {
            self.prev_mode = Some((old_mode, old_w, old_h));
            self.morph_t.snap(0.0);
            self.morph_t.set_target(1.0);
            self.tab_t.snap(1.0);
            self.ui.detail_tab_t = 1.0;
        }
        let target_bg = if matches!(mode, ViewMode::Collapsed) {
            self.cfg.idle_opacity
        } else {
            self.cfg.glass_opacity
        };
        if self.reduced_motion {
            self.bg_alpha.snap(target_bg);
        } else {
            if !matches!(mode, ViewMode::Collapsed) && self.bg_alpha.value < target_bg * 0.65 {
                self.bg_alpha.snap(target_bg * 0.65);
            }
            self.bg_alpha.set_target(target_bg);
        }

        let inspect_target = match &mode {
            ViewMode::Detail { project_id, job_id } => {
                Some((project_id.clone(), job_id.clone()))
            }
            _ => None,
        };
        self.engine.send(EngineCmd::InspectTerminal(inspect_target));

        let polling = match mode {
            ViewMode::Collapsed => {
                if self.fullscreen_hidden {
                    PollingMode::LowPowerLocked
                } else {
                    PollingMode::CollapsedIdle
                }
            }
            ViewMode::HoverPeek => PollingMode::HoverPeek,
            ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings => {
                PollingMode::ExpandedActive
            }
        };
        self.engine.send(EngineCmd::SetPollingMode(polling));

        if !matches!(mode, ViewMode::Collapsed) {
            set_window_topmost(self.hwnd, self.cfg.always_on_top);
        }

        if activate && interactive {
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(self.hwnd);
            }
            self.ui.search_focused = matches!(self.ui.mode, ViewMode::Expanded);
        }

        self.recompute_targets(false);
        let _ = self.render_frame();
    }

    pub fn toggle_expanded(&mut self) {
        if self.fullscreen_hidden {
            self.fullscreen_hidden = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
        }
        match self.ui.mode {
            ViewMode::Collapsed | ViewMode::HoverPeek => {
                self.set_mode(ViewMode::Expanded, true);
            }
            ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings => {
                self.set_mode(ViewMode::Collapsed, false);
            }
        }
    }

    pub fn recompute_targets(&mut self, snap_immediately: bool) {
        let mon = select_monitor(&self.monitors, self.cfg.monitor_index);
        let work_h_dip = mon.map(monitor_work_height_dip).unwrap_or(1000.0);
        let (tw, th, tr) = target_dimensions(&self.ui, &self.snap, self.cfg.dock_position, work_h_dip);
        let (tx, ty) = if let Some(m) = mon {
            let (x, y, _, _) = compute_dock_rect(m, self.cfg.dock_position, tw, th, 0.0);
            (x as f32, y as f32)
        } else {
            (self.x_px.target, self.y_px.target)
        };

        if snap_immediately || self.reduced_motion {
            self.x_px.snap(tx);
            self.y_px.snap(ty);
            self.w_dip.snap(tw);
            self.h_dip.snap(th);
            self.radius_dip.snap(tr);
            self.morph_t.snap(1.0);
            self.tab_t.snap(1.0);
            self.ui.detail_tab_t = 1.0;
            self.prev_mode = None;
            self.snap_window_to_current();
        } else {
            self.x_px.set_target(tx);
            self.y_px.set_target(ty);
            self.w_dip.set_target(tw);
            self.h_dip.set_target(th);
            self.radius_dip.set_target(tr);
            let needs_anim = [
                &self.x_px,
                &self.y_px,
                &self.w_dip,
                &self.h_dip,
                &self.radius_dip,
                &self.morph_t,
                &self.tab_t,
                &self.bg_alpha,
            ]
            .iter()
            .any(|s| (s.value - s.target).abs() >= 0.004 || s.velocity.abs() >= 0.02);
            if needs_anim {
                self.ensure_envelope_for_target(tx, ty, tw, th);
                self.start_anim_timer();
            }
        }
    }

    fn start_anim_timer(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_anim_tick = Instant::now();
            unsafe {
                SetTimer(self.hwnd, TIMER_ANIM, 8, None);
            }
        }
    }

    pub fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_ANIM => self.step_animation(),
            TIMER_HOVER_PEEK => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_HOVER_PEEK);
                }
                if matches!(self.ui.mode, ViewMode::Collapsed)
                    && !self.fullscreen_hidden
                    && self.cfg.dock_position == DockPosition::TopCenter
                {
                    self.set_mode(ViewMode::HoverPeek, false);
                }
            }
            TIMER_LEAVE_COLLAPSE => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_LEAVE_COLLAPSE);
                }
                if self.should_auto_collapse_on_leave() {
                    self.set_mode(ViewMode::Collapsed, false);
                }
            }
            TIMER_TRIM => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_TRIM);
                }
                if matches!(self.ui.mode, ViewMode::Collapsed) && !self.animating {
                    self.device.trim();
                    unsafe {
                        let _ = SetProcessWorkingSetSizeEx(
                            GetCurrentProcess(),
                            usize::MAX,
                            usize::MAX,
                            SETPROCESSWORKINGSETSIZEEX_FLAGS(0),
                        );
                    }
                }
            }
            TIMER_FULLSCREEN => self.check_fullscreen(),
            _ => {}
        }
    }

    fn step_animation(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_anim_tick).as_secs_f32().clamp(0.001, 0.033);
        self.last_anim_tick = now;

        let spec = if self.snap.alerts.iter().any(|a| !a.dismissed) {
            SPRING_ALERT
        } else {
            SPRING_MORPH
        };
        let mut moving = false;
        moving |= self.x_px.step(dt, spec);
        moving |= self.y_px.step(dt, spec);
        moving |= self.w_dip.step(dt, spec);
        moving |= self.h_dip.step(dt, spec);
        moving |= self.radius_dip.step(dt, spec);
        moving |= self.morph_t.step_unit(dt, spec);
        moving |= self.tab_t.step_unit(dt, SPRING_MORPH);
        moving |= self.bg_alpha.step_unit(dt, spec);
        self.ui.detail_tab_t = self.tab_t.value.clamp(0.0, 1.0);

        let _ = self.render_frame();

        if !moving {
            self.animating = false;
            self.prev_mode = None;
            self.morph_t.snap(1.0);
            self.tab_t.snap(1.0);
            self.ui.detail_tab_t = 1.0;
            self.snap_window_to_current();
            let _ = self.render_frame();
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ANIM);
                if matches!(self.ui.mode, ViewMode::Collapsed) {
                    SetTimer(self.hwnd, TIMER_TRIM, 500, None);
                }
            }
        }
    }

    fn set_window_rect(&mut self, rect: (i32, i32, i32, i32)) {
        if rect == self.win_rect_px {
            return;
        }
        self.win_rect_px = rect;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                rect.0,
                rect.1,
                rect.2.max(1),
                rect.3.max(1),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }

    fn ensure_envelope_for_target(&mut self, tx: f32, ty: f32, tw_dip: f32, th_dip: f32) {
        let cur_w_px = dip_to_px(self.w_dip.value, self.dpi).max(1);
        let cur_h_px = dip_to_px(self.h_dip.value, self.dpi).max(1);
        let tgt_w_px = dip_to_px(tw_dip, self.dpi).max(1);
        let tgt_h_px = dip_to_px(th_dip, self.dpi).max(1);
        let pad = dip_to_px(14.0, self.dpi).max(12);
        let req = compute_envelope_rect(
            (self.x_px.value, self.y_px.value, cur_w_px, cur_h_px),
            (tx, ty, tgt_w_px, tgt_h_px),
            pad,
        );
        let (wx, wy, ww, wh) = self.win_rect_px;
        let contains = wx <= req.0
            && wy <= req.1
            && wx + ww >= req.0 + req.2
            && wy + wh >= req.1 + req.3;
        if !contains {
            let nl = wx.min(req.0);
            let nt = wy.min(req.1);
            let nr = (wx + ww).max(req.0 + req.2);
            let nb = (wy + wh).max(req.1 + req.3);
            self.set_window_rect((nl, nt, (nr - nl).max(1), (nb - nt).max(1)));
        }
    }

    fn snap_window_to_current(&mut self) {
        let x = self.x_px.value.round() as i32;
        let y = self.y_px.value.round() as i32;
        let w_px = dip_to_px(self.w_dip.value, self.dpi).max(1);
        let h_px = dip_to_px(self.h_dip.value, self.dpi).max(1);
        self.set_window_rect((x, y, w_px, h_px));
    }

    fn sync_fullscreen_timer(&self) {
        unsafe {
            if self.cfg.hide_on_fullscreen {
                SetTimer(self.hwnd, TIMER_FULLSCREEN, 1500, None);
            } else {
                let _ = KillTimer(self.hwnd, TIMER_FULLSCREEN);
            }
        }
    }

    fn check_fullscreen(&mut self) {
        if !self.cfg.hide_on_fullscreen {
            if self.fullscreen_hidden {
                self.fullscreen_hidden = false;
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                }
                self.engine
                    .send(EngineCmd::SetPollingMode(PollingMode::CollapsedIdle));
            }
            return;
        }

        // Do not auto-hide while the user explicitly opened the panel/settings.
        if !matches!(self.ui.mode, ViewMode::Collapsed | ViewMode::HoverPeek) {
            return;
        }

        let fs = is_fullscreen_active(self.hwnd);
        if fs && !self.fullscreen_hidden {
            self.fullscreen_hidden = true;
            if matches!(self.ui.mode, ViewMode::HoverPeek) {
                self.set_mode(ViewMode::Collapsed, false);
            }
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.engine
                .send(EngineCmd::SetPollingMode(PollingMode::LowPowerLocked));
        } else if !fs && self.fullscreen_hidden {
            self.fullscreen_hidden = false;
            set_window_topmost(self.hwnd, self.cfg.always_on_top);
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.engine
                .send(EngineCmd::SetPollingMode(PollingMode::CollapsedIdle));
        }
    }

    fn recreate_graphics_resources(&mut self) -> windows::core::Result<()> {
        let surf_w_px = self.win_rect_px.2.max(1);
        let surf_h_px = self.win_rect_px.3.max(1);
        self.device.recreate()?;
        self.comp
            .recreate_surface(&self.device, surf_w_px, surf_h_px)
    }

    pub fn render_frame(&mut self) -> windows::core::Result<()> {
        match self.render_frame_once() {
            Ok(()) => Ok(()),
            Err(e) if is_device_lost_error(e.code()) => {
                self.recreate_graphics_resources()?;
                self.render_frame_once()
            }
            Err(e) => Err(e),
        }
    }

    fn render_frame_once(&mut self) -> windows::core::Result<()> {
        let w_dip = self.w_dip.value.max(40.0);
        let h_dip = self.h_dip.value.max(24.0);
        let r_dip = self.radius_dip.value.max(4.0);
        let scale = self.dpi as f32 / 96.0;
        let r_px = r_dip * scale;

        let off_x_px = self.x_px.value - self.win_rect_px.0 as f32;
        let off_y_px = self.y_px.value - self.win_rect_px.1 as f32;
        let off_x_dip = off_x_px / scale;
        let off_y_dip = off_y_px / scale;
        let surf_w_px = self.win_rect_px.2.max(1);
        let surf_h_px = self.win_rect_px.3.max(1);

        let want_acrylic = self.cfg.acrylic_enabled
            && (!matches!(self.ui.mode, ViewMode::Collapsed)
                || self.animating
                || self.bg_alpha.value >= 0.40);
        self.comp.update_backdrop(want_acrylic);
        self.comp.set_geometry(
            surf_w_px,
            surf_h_px,
            off_x_px,
            off_y_px,
            w_dip * scale,
            h_dip * scale,
            r_px,
        )?;

        let dc = self.comp.begin_draw(self.dpi, off_x_dip, off_y_dip)?;

        unsafe {
            dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }

        let ctx = DrawCtx::new(
            &dc,
            &self.device.d2d_factory,
            &self.device.dwrite_factory,
            &self.device.fonts.tabular_typography,
        )?;
        let acrylic_eff = self.comp.is_acrylic_effective(want_acrylic);
        let morph = MorphParams {
            prev: self
                .prev_mode
                .as_ref()
                .map(|(m, pw, ph)| (m, *pw, *ph)),
            target_w: self.w_dip.target.max(40.0),
            target_h: self.h_dip.target.max(24.0),
            morph_t: self.morph_t.value.clamp(0.0, 1.0),
            bg_opacity: self.bg_alpha.value.clamp(0.0, 0.99),
        };
        let draw_res = render_ui(
            &ctx,
            &self.device.fonts,
            w_dip,
            h_dip,
            r_dip,
            morph,
            &mut self.ui,
            &self.snap,
            &self.cfg,
            &self.monitors,
            acrylic_eff,
            &mut self.hit_map,
        );

        let end_res = self.comp.end_draw();
        draw_res.and(end_res)
    }

    #[inline]
    pub fn client_to_island_dip(&self, client_x_px: i32, client_y_px: i32) -> (f32, f32) {
        let scale = self.dpi as f32 / 96.0;
        let off_x = self.x_px.value - self.win_rect_px.0 as f32;
        let off_y = self.y_px.value - self.win_rect_px.1 as f32;
        (
            (client_x_px as f32 - off_x) / scale,
            (client_y_px as f32 - off_y) / scale,
        )
    }

    pub fn is_point_inside_pill(&self, client_x_px: i32, client_y_px: i32) -> bool {
        let (x, y) = self.client_to_island_dip(client_x_px, client_y_px);
        let w = self.w_dip.value;
        let h = self.h_dip.value;
        let r = self.radius_dip.value.min(w * 0.5).min(h * 0.5);

        if x < 0.0 || y < 0.0 || x > w || y > h {
            return false;
        }
        let cx = x.clamp(r, w - r);
        let cy = y.clamp(r, h - r);
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= r * r + 1.0
    }

    pub fn on_mouse_move(&mut self, x_px: i32, y_px: i32) {
        if !self.mouse_tracked {
            self.mouse_tracked = true;
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
        }

        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_LEAVE_COLLAPSE);
        }

        if matches!(self.ui.mode, ViewMode::Collapsed)
            && self.cfg.dock_position == DockPosition::TopCenter
        {
            unsafe {
                SetTimer(
                    self.hwnd,
                    TIMER_HOVER_PEEK,
                    self.cfg.hover_peek_delay_ms,
                    None,
                );
            }
        }

        let (x, y) = self.client_to_island_dip(x_px, y_px);
        self.last_mouse_dip = (x, y);
        let inside = x >= 0.0 && y >= 0.0 && x <= self.w_dip.value && y <= self.h_dip.value;
        let next_pos = if inside { Some((x, y)) } else { None };
        let moved_enough = match (self.ui.hover_pos_dip, next_pos) {
            (Some((px, py)), Some((nx, ny))) => (px - nx).hypot(py - ny) >= 2.5,
            (None, None) => false,
            _ => true,
        };
        let new_hover = self.hit_map.hit_test(x, y).map(|r| r.id);
        let hover_changed = new_hover != self.ui.hovered_hit_id;
        if hover_changed {
            self.ui.hovered_hit_id = new_hover;
        }
        if moved_enough {
            self.ui.hover_pos_dip = next_pos;
        }
        let throttle_ok =
            hover_changed || self.last_hover_render.elapsed() >= Duration::from_millis(16);
        if (hover_changed || moved_enough)
            && throttle_ok
            && (!matches!(self.ui.mode, ViewMode::Collapsed) || self.cfg.idle_opacity >= 0.25)
        {
            self.last_hover_render = Instant::now();
            let _ = self.render_frame();
        }
    }

    fn should_auto_collapse_on_leave(&self) -> bool {
        matches!(self.ui.mode, ViewMode::HoverPeek)
            || (!self.ui.sticky
                && !self.menu_open
                && matches!(
                    self.ui.mode,
                    ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings
                )
                && (self.ui.unpinned_auto_collapse || !is_foreground_window(self.hwnd)))
    }

    pub fn on_mouse_leave(&mut self) {
        self.mouse_tracked = false;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_HOVER_PEEK);
        }
        let had_pos = self.ui.hover_pos_dip.take().is_some();
        if self.ui.hovered_hit_id.take().is_some() || had_pos {
            let _ = self.render_frame();
        }
        if self.should_auto_collapse_on_leave() {
            unsafe {
                SetTimer(self.hwnd, TIMER_LEAVE_COLLAPSE, 250, None);
            }
        }
    }

    pub fn on_left_click(&mut self, x_px: i32, y_px: i32) {
        let (x, y) = self.client_to_island_dip(x_px, y_px);
        let action = self.hit_map.hit_test(x, y).map(|r| r.action.clone());
        if let Some(act) = action {
            self.execute_hit_action(act);
        }
    }

    /// Handles right-click inside the island. Returns `Some(AppConfig)` if a fallback
    /// context menu should be shown *after* dropping the `App` borrow.
    pub fn on_right_click(&mut self, x_px: i32, y_px: i32) -> Option<AppConfig> {
        let (x, y) = self.client_to_island_dip(x_px, y_px);
        if let Some(hit) = self.hit_map.hit_test(x, y) {
            if let HitAction::OpenPort { port, url } = &hit.action {
                let target = url
                    .clone()
                    .unwrap_or_else(|| format!("http://localhost:{port}"));
                copy_to_clipboard(self.hwnd, &target);
                return None;
            }
            if let Some(r_act) = hit.right_click_action.clone() {
                self.execute_hit_action(r_act);
                return None;
            }
        }
        self.menu_open = true;
        Some(self.cfg.clone())
    }

    pub fn on_mouse_wheel(&mut self, delta: i16, x_px: i32, y_px: i32) {
        if !matches!(self.ui.mode, ViewMode::Expanded | ViewMode::Detail { .. }) {
            return;
        }
        self.last_mouse_dip = self.client_to_island_dip(x_px, y_px);
        let step = -(delta as f32 / 120.0) * 36.0;
        let in_term = self.hit_map.term_viewport.is_some_and(|r| {
            let (mx, my) = self.last_mouse_dip;
            mx >= r.left && mx <= r.right && my >= r.top && my <= r.bottom
        });
        if in_term {
            // Scroll up (positive delta) moves toward older lines (increases offset from bottom).
            let term_step = (delta as f32 / 120.0) * 30.0;
            self.ui.term_scroll_offset =
                (self.ui.term_scroll_offset + term_step).clamp(0.0, self.ui.term_max_scroll);
        } else {
            self.ui.scroll_offset = (self.ui.scroll_offset + step).clamp(0.0, self.ui.max_scroll);
        }
        let _ = self.render_frame();
    }

    pub fn on_key_down(&mut self, vk: u16) {
        if !matches!(
            self.ui.mode,
            ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings
        ) {
            return;
        }

        let ctrl_down = unsafe { (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 };
        let alt_down = unsafe { (GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 };

        // Inline Add Custom Job mode
        if self.ui.add_job_draft.is_some() {
            match vk {
                v if v == VK_ESCAPE.0 => {
                    self.execute_hit_action(HitAction::CancelAddJob);
                }
                v if v == VK_RETURN.0 => {
                    self.execute_hit_action(HitAction::SaveAddJob);
                }
                v if v == VK_TAB.0 => {
                    if let Some(ref mut a) = self.ui.add_job_draft {
                        a.active_field = (a.active_field + 1) % 3;
                    }
                    self.ui.text_all_selected = false;
                    let _ = self.render_frame();
                }
                _ => {
                    let changed = if let Some(ref mut a) = self.ui.add_job_draft {
                        let target = match a.active_field {
                            0 => &mut a.title,
                            1 => &mut a.command_or_match,
                            _ => &mut a.log_file,
                        };
                        handle_text_edit_shortcut(
                            target,
                            &mut self.ui.text_all_selected,
                            vk,
                            ctrl_down,
                            self.hwnd,
                        )
                    } else {
                        false
                    };
                    if changed {
                        let _ = self.render_frame();
                    }
                }
            }
            return;
        }

        // Inline Rename mode intercepts Esc, Enter, and shared text-editing shortcuts.
        if self.ui.rename_draft.is_some() {
            match vk {
                v if v == VK_ESCAPE.0 => {
                    self.execute_hit_action(HitAction::CancelRename);
                }
                v if v == VK_RETURN.0 => {
                    self.execute_hit_action(HitAction::SaveRename);
                }
                _ => {
                    let changed = if let Some(ref mut d) = self.ui.rename_draft {
                        handle_text_edit_shortcut(
                            &mut d.input,
                            &mut self.ui.text_all_selected,
                            vk,
                            ctrl_down,
                            self.hwnd,
                        )
                    } else {
                        false
                    };
                    if changed {
                        let _ = self.render_frame();
                    }
                }
            }
            return;
        }

        if matches!(self.ui.mode, ViewMode::Expanded)
            && (self.ui.search_focused || (ctrl_down && vk == VK_V.0))
        {
            if ctrl_down && vk == VK_V.0 {
                self.ui.search_focused = true;
            }
            if handle_text_edit_shortcut(
                &mut self.ui.search_query,
                &mut self.ui.text_all_selected,
                vk,
                ctrl_down,
                self.hwnd,
            ) {
                self.ui.selected_index = 0;
                self.recompute_targets(false);
                let _ = self.render_frame();
                return;
            }
        }

        if alt_down && self.on_sys_key_down(vk) {
            return;
        }

        match vk {
            v if v == VK_ESCAPE.0 => {
                if self.ui.pending_kill.is_some() {
                    self.ui.pending_kill = None;
                } else if !self.ui.search_query.is_empty() {
                    self.ui.search_query.clear();
                    self.ui.text_all_selected = false;
                    self.recompute_targets(false);
                } else if matches!(self.ui.mode, ViewMode::Detail { .. } | ViewMode::Settings) {
                    self.set_mode(ViewMode::Expanded, true);
                    return;
                } else {
                    self.set_mode(ViewMode::Collapsed, false);
                    return;
                }
                let _ = self.render_frame();
            }
            v if v == VK_DOWN.0 => {
                if matches!(self.ui.mode, ViewMode::Expanded) {
                    let count: usize = filter_project_jobs(&self.snap, &self.ui.search_query)
                        .iter()
                        .map(|(_, jobs)| jobs.len())
                        .sum();
                    if count > 0 {
                        self.ui.selected_index = (self.ui.selected_index + 1).min(count - 1);
                        let vh = self
                            .hit_map
                            .scroll_viewport
                            .map(|r| (r.bottom - r.top).max(40.0))
                            .unwrap_or_else(|| (self.h_dip.target - 284.0).max(80.0));
                        ensure_selected_job_visible(&mut self.ui, &self.snap, vh);
                        let _ = self.render_frame();
                    }
                } else if matches!(self.ui.mode, ViewMode::Detail { .. })
                    && self.ui.term_max_scroll > 0.0
                {
                    self.ui.term_scroll_offset =
                        (self.ui.term_scroll_offset - 30.0).clamp(0.0, self.ui.term_max_scroll);
                    let _ = self.render_frame();
                }
            }
            v if v == VK_UP.0 => {
                if matches!(self.ui.mode, ViewMode::Expanded) {
                    self.ui.selected_index = self.ui.selected_index.saturating_sub(1);
                    let vh = self
                        .hit_map
                        .scroll_viewport
                        .map(|r| (r.bottom - r.top).max(40.0))
                        .unwrap_or_else(|| (self.h_dip.target - 284.0).max(80.0));
                    ensure_selected_job_visible(&mut self.ui, &self.snap, vh);
                    let _ = self.render_frame();
                } else if matches!(self.ui.mode, ViewMode::Detail { .. })
                    && self.ui.term_max_scroll > 0.0
                {
                    self.ui.term_scroll_offset =
                        (self.ui.term_scroll_offset + 30.0).clamp(0.0, self.ui.term_max_scroll);
                    let _ = self.render_frame();
                }
            }
            v if v == VK_PRIOR.0 && matches!(self.ui.mode, ViewMode::Detail { .. }) => {
                if self.ui.term_max_scroll > 0.0 {
                    self.ui.term_scroll_offset =
                        (self.ui.term_scroll_offset + 120.0).clamp(0.0, self.ui.term_max_scroll);
                    let _ = self.render_frame();
                }
            }
            v if v == VK_NEXT.0 && matches!(self.ui.mode, ViewMode::Detail { .. }) => {
                if self.ui.term_max_scroll > 0.0 {
                    self.ui.term_scroll_offset =
                        (self.ui.term_scroll_offset - 120.0).clamp(0.0, self.ui.term_max_scroll);
                    let _ = self.render_frame();
                }
            }
            v if v == VK_RETURN.0 || v == VK_RIGHT.0 => {
                if matches!(self.ui.mode, ViewMode::Expanded) {
                    let target = filter_project_jobs(&self.snap, &self.ui.search_query)
                        .into_iter()
                        .flat_map(|(_, jobs)| jobs.into_iter())
                        .nth(self.ui.selected_index)
                        .map(|j| (j.project.clone(), j.id.clone()));
                    if let Some((pid, jid)) = target {
                        self.set_mode(
                            ViewMode::Detail {
                                project_id: pid,
                                job_id: Some(jid),
                            },
                            true,
                        );
                    }
                }
            }
            v if v == VK_LEFT.0 => {
                if matches!(self.ui.mode, ViewMode::Detail { .. } | ViewMode::Settings)
                    && !self.ui.search_focused
                {
                    self.set_mode(ViewMode::Expanded, true);
                }
            }
            v if v == VK_BACK.0 => {
                if self.ui.search_focused && !self.ui.search_query.is_empty() {
                    self.ui.search_query.pop();
                    self.recompute_targets(false);
                    let _ = self.render_frame();
                } else if matches!(self.ui.mode, ViewMode::Detail { .. } | ViewMode::Settings)
                    && self.ui.search_query.is_empty()
                {
                    self.set_mode(ViewMode::Expanded, true);
                }
            }
            v if v == VK_TAB.0 && matches!(self.ui.mode, ViewMode::Expanded) => {
                self.ui.search_focused = !self.ui.search_focused;
                self.ui.text_all_selected = false;
                let _ = self.render_frame();
            }
            v if v == VK_DELETE.0
                && matches!(self.ui.mode, ViewMode::Expanded)
                && (!self.ui.search_focused || self.ui.search_query.is_empty()) =>
            {
                let jid = filter_project_jobs(&self.snap, &self.ui.search_query)
                    .into_iter()
                    .flat_map(|(_, jobs)| jobs.into_iter())
                    .nth(self.ui.selected_index)
                    .filter(|j| j.can_kill)
                    .map(|j| j.id.clone());
                if let Some(id) = jid {
                    self.ui.pending_kill = Some(PendingKill::Job(id));
                    let _ = self.render_frame();
                }
            }
            _ => {}
        }
    }

    pub fn on_sys_key_down(&mut self, vk: u16) -> bool {
        if !matches!(self.ui.mode, ViewMode::Expanded)
            || self.ui.add_job_draft.is_some()
            || self.ui.rename_draft.is_some()
        {
            return false;
        }
        let alt_down = unsafe { (GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 };
        if !alt_down {
            return false;
        }
        if vk == VK_K.0 {
            let jid = filter_project_jobs(&self.snap, &self.ui.search_query)
                .into_iter()
                .flat_map(|(_, jobs)| jobs.into_iter())
                .nth(self.ui.selected_index)
                .filter(|j| j.can_kill)
                .map(|j| j.id.clone());
            if let Some(id) = jid {
                self.ui.pending_kill = Some(PendingKill::Job(id));
                let _ = self.render_frame();
            }
            return true;
        }
        if vk == VK_P.0 {
            let id = filter_project_jobs(&self.snap, &self.ui.search_query)
                .into_iter()
                .flat_map(|(p, jobs)| jobs.into_iter().map(move |_| p.id.clone()))
                .nth(self.ui.selected_index);
            if let Some(id) = id {
                self.execute_hit_action(HitAction::TogglePinProject(id));
            }
            return true;
        }
        false
    }

    pub fn on_char(&mut self, ch: char) {
        if !matches!(self.ui.mode, ViewMode::Expanded) {
            return;
        }
        if let Some(ref mut add) = self.ui.add_job_draft {
            if !ch.is_control() {
                let target = match add.active_field {
                    0 => &mut add.title,
                    1 => &mut add.command_or_match,
                    _ => &mut add.log_file,
                };
                if self.ui.text_all_selected {
                    target.clear();
                    self.ui.text_all_selected = false;
                }
                target.push(ch);
                let _ = self.render_frame();
            }
            return;
        }
        if let Some(ref mut draft) = self.ui.rename_draft {
            if !ch.is_control() {
                if self.ui.text_all_selected {
                    draft.input.clear();
                    self.ui.text_all_selected = false;
                }
                draft.input.push(ch);
                let _ = self.render_frame();
            }
            return;
        }
        if ch == '/' && !self.ui.search_focused {
            self.ui.search_focused = true;
            self.ui.text_all_selected = false;
            let _ = self.render_frame();
            return;
        }
        if !ch.is_control() {
            self.ui.search_focused = true;
            if self.ui.text_all_selected {
                self.ui.search_query.clear();
                self.ui.text_all_selected = false;
            }
            self.ui.search_query.push(ch);
            self.ui.selected_index = 0;
            self.recompute_targets(false);
            let _ = self.render_frame();
        }
    }

    fn execute_hit_action(&mut self, action: HitAction) {
        if !matches!(action, HitAction::ToggleSticky) {
            self.ui.unpinned_auto_collapse = false;
        }
        match action {
            HitAction::ExpandPanel => self.set_mode(ViewMode::Expanded, true),
            HitAction::CollapsePanel => self.set_mode(ViewMode::Collapsed, false),
            HitAction::ToggleSticky => {
                self.ui.sticky = !self.ui.sticky;
                if self.ui.sticky {
                    self.ui.unpinned_auto_collapse = false;
                } else {
                    self.ui.unpinned_auto_collapse = true;
                    set_window_activatable(self.hwnd, true);
                    unsafe {
                        let _ = SetForegroundWindow(self.hwnd);
                        let _ = SetFocus(self.hwnd);
                    }
                    if !is_foreground_window(self.hwnd) {
                        self.set_mode(ViewMode::Collapsed, false);
                        return;
                    }
                }
                let _ = self.render_frame();
            }
            HitAction::ToggleShowAllProcesses => {
                self.ui.show_all_processes = !self.ui.show_all_processes;
                self.ui.scroll_offset = 0.0;
                self.engine
                    .send(EngineCmd::SetShowAllProcesses(self.ui.show_all_processes));
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::OpenSettings => self.set_mode(ViewMode::Settings, true),
            HitAction::BackToExpanded => self.set_mode(ViewMode::Expanded, true),
            HitAction::FocusSearch => {
                self.ui.search_focused = true;
                self.ui.text_all_selected = false;
                let _ = self.render_frame();
            }
            HitAction::ClearSearch => {
                self.ui.search_query.clear();
                self.ui.text_all_selected = false;
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::OpenProjectDetail(id) => self.set_mode(
                ViewMode::Detail {
                    project_id: id,
                    job_id: None,
                },
                true,
            ),
            HitAction::OpenJobDetail(pid, jid) => self.set_mode(
                ViewMode::Detail {
                    project_id: pid,
                    job_id: Some(jid),
                },
                true,
            ),
            HitAction::OpenProjectFolder(id) => {
                if let Some(root) = self
                    .snap
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .and_then(|p| p.root.clone())
                {
                    shell_open(&root.to_string_lossy());
                }
            }
            HitAction::TogglePinProject(id) => {
                if let Some(p) = self.snap.projects.iter().find(|p| p.id == id) {
                    let next = !p.pinned;
                    let key_l = id.0.to_ascii_lowercase();
                    self.cfg
                        .pinned_projects
                        .retain(|x| !x.eq_ignore_ascii_case(&key_l));
                    if next {
                        self.cfg.pinned_projects.push(id.0.clone());
                    }
                    self.save_cfg_or_toast();
                    self.engine.send(EngineCmd::SetPinned {
                        project_id: id,
                        pinned: next,
                    });
                }
            }
            HitAction::CycleSortMode => {
                self.cfg.sort_mode = self.cfg.sort_mode.next();
                self.save_cfg_or_toast();
                self.sync_config_to_engine();
                let _ = self.render_frame();
            }
            HitAction::MoveProjectOrder(id, delta) => {
                let current_ids: Vec<String> =
                    self.snap.projects.iter().map(|p| p.id.0.clone()).collect();
                if reorder_vec(&mut self.cfg.project_order, &current_ids, &id.0, delta) {
                    self.cfg.sort_mode = SortMode::Custom;
                    self.save_cfg_or_toast();
                    self.sync_config_to_engine();
                }
            }
            HitAction::MoveJobOrder(key, delta) => {
                let current_ids: Vec<String> = self
                    .snap
                    .projects
                    .iter()
                    .flat_map(|p| p.jobs.iter())
                    .map(|j| {
                        if !j.match_hint.is_empty() {
                            j.match_hint.clone()
                        } else {
                            j.id.0.clone()
                        }
                    })
                    .collect();
                if reorder_vec(&mut self.cfg.job_order, &current_ids, &key, delta) {
                    self.cfg.sort_mode = SortMode::Custom;
                    self.save_cfg_or_toast();
                    self.sync_config_to_engine();
                }
            }
            HitAction::OpenPort { port, url } => {
                let target = url.unwrap_or_else(|| format!("http://localhost:{port}"));
                shell_open(&target);
            }
            HitAction::CopyText(text) => {
                copy_to_clipboard(self.hwnd, &text);
            }
            HitAction::HideProject(id) => {
                if !self
                    .cfg
                    .hidden_projects
                    .iter()
                    .any(|x| x.eq_ignore_ascii_case(&id.0))
                {
                    self.cfg.hidden_projects.push(id.0.clone());
                }
                self.save_cfg_or_toast();
                self.sync_config_to_engine();
                let leave_detail = matches!(
                    &self.ui.mode,
                    ViewMode::Detail { project_id, .. } if project_id == &id
                );
                self.engine.send(EngineCmd::HideProject(id));
                if leave_detail {
                    self.set_mode(ViewMode::Expanded, true);
                }
            }
            HitAction::RestoreHiddenProjects => {
                if !self.cfg.hidden_projects.is_empty() {
                    self.cfg.hidden_projects.clear();
                    self.save_cfg_or_toast();
                    self.sync_config_to_engine();
                    self.engine.send(EngineCmd::ForceRescan);
                    let _ = self.render_frame();
                }
            }
            HitAction::RequestKillJob(id) => {
                self.ui.pending_kill = Some(PendingKill::Job(id));
                let _ = self.render_frame();
            }
            HitAction::ConfirmKillJob(id) => {
                self.ui.pending_kill = None;
                self.engine.send(EngineCmd::KillJob(id));
            }
            HitAction::RestartJob(id) => {
                self.engine.send(EngineCmd::RestartJob(id));
            }
            HitAction::StartCustomJob(id) => {
                self.engine.send(EngineCmd::StartCustomJob(id));
            }
            HitAction::RemoveWatchedJob(match_hint) => {
                self.engine.send(EngineCmd::RemoveLabelRule(match_hint));
            }
            HitAction::BeginRenameJob {
                match_hint,
                current_title,
            } => {
                self.ui.search_focused = false;
                self.ui.text_all_selected = false;
                self.ui.add_job_draft = None;
                self.ui.rename_draft = Some(RenameDraft {
                    project_id: None,
                    match_substr: match_hint,
                    input: current_title,
                });
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::BeginRenameProject {
                project_id,
                current_name,
            } => {
                self.ui.search_focused = false;
                self.ui.text_all_selected = false;
                self.ui.add_job_draft = None;
                self.ui.rename_draft = Some(RenameDraft {
                    project_id: Some(project_id),
                    match_substr: String::new(),
                    input: current_name,
                });
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::SaveRename => {
                if let Some(draft) = self.ui.rename_draft.take() {
                    let new_name = draft.input.trim().to_string();
                    if !new_name.is_empty() {
                        if let Some(pid) = draft.project_id {
                            self.cfg.project_aliases.insert(pid.0.clone(), new_name.clone());
                            self.save_cfg_or_toast();
                            self.engine.send(EngineCmd::SetAlias {
                                project_id: pid,
                                custom_name: Some(new_name),
                            });
                        } else if !draft.match_substr.is_empty() {
                            self.engine.send(EngineCmd::SaveLabelRule {
                                match_substr: draft.match_substr,
                                project: None,
                                job: new_name,
                                icon: None,
                                color: None,
                                command: None,
                                working_dir: None,
                                log_file: None,
                                watch: None,
                            });
                        }
                    }
                }
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::CancelRename => {
                self.ui.rename_draft = None;
                self.ui.search_focused = matches!(self.ui.mode, ViewMode::Expanded);
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::BeginAddJob => {
                self.ui.search_focused = false;
                self.ui.text_all_selected = false;
                self.ui.rename_draft = None;
                if self.ui.add_job_draft.is_some() {
                    self.ui.add_job_draft = None;
                } else {
                    self.ui.add_job_draft = Some(AddJobDraft::default());
                }
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::FocusAddJobField(idx) => {
                if let Some(ref mut a) = self.ui.add_job_draft {
                    a.active_field = idx;
                }
                self.ui.text_all_selected = false;
                let _ = self.render_frame();
            }
            HitAction::SaveAddJob => {
                if let Some(add) = self.ui.add_job_draft.take() {
                    let raw_cmd = add.command_or_match.trim().to_string();
                    let title = if add.title.trim().is_empty() {
                        derive_match_hint(&raw_cmd)
                    } else {
                        add.title.trim().to_string()
                    };
                    if !raw_cmd.is_empty() {
                        let match_substr = derive_match_hint(&raw_cmd);
                        let is_cmd = raw_cmd.contains(' ')
                            || raw_cmd.ends_with(".py")
                            || raw_cmd.ends_with(".js")
                            || raw_cmd.ends_with(".bat")
                            || raw_cmd.ends_with(".cmd")
                            || raw_cmd.ends_with(".ps1");
                        let working_dir = derive_workdir_from_cmd(&raw_cmd);
                        let log_file = {
                            let lf = add.log_file.trim();
                            if lf.is_empty() {
                                None
                            } else {
                                Some(lf.to_string())
                            }
                        };
                        self.engine.send(EngineCmd::SaveLabelRule {
                            match_substr,
                            project: Some(
                                self.cfg
                                    .language
                                    .pick("Custom Jobs", "Özel İşler")
                                    .to_string(),
                            ),
                            job: title,
                            icon: None,
                            color: None,
                            command: if is_cmd { Some(raw_cmd) } else { None },
                            working_dir,
                            log_file,
                            watch: Some(true),
                        });
                    }
                }
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::CancelAddJob => {
                self.ui.add_job_draft = None;
                self.ui.search_focused = matches!(self.ui.mode, ViewMode::Expanded);
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::WatchProcessFromAll {
                match_substr,
                title,
                working_dir,
            } => {
                self.engine.send(EngineCmd::SaveLabelRule {
                    match_substr,
                    project: Some(
                        self.cfg
                            .language
                            .pick("Custom Jobs", "Özel İşler")
                            .to_string(),
                    ),
                    job: title,
                    icon: None,
                    color: None,
                    command: None,
                    working_dir,
                    log_file: None,
                    watch: Some(true),
                });
                self.ui.show_all_processes = false;
                self.engine.send(EngineCmd::SetShowAllProcesses(false));
                self.recompute_targets(false);
                let _ = self.render_frame();
            }
            HitAction::CopyTerminalOutput => {
                if let Some(ref term) = self.snap.active_terminal {
                    if !term.lines.is_empty() {
                        copy_to_clipboard(self.hwnd, &term.lines.join("\r\n"));
                    }
                }
            }
            HitAction::RefreshTerminalOutput => {
                self.engine.send(EngineCmd::ForceRescan);
            }
            HitAction::RequestKillProject(id) => {
                self.ui.pending_kill = Some(PendingKill::Project(id));
                let _ = self.render_frame();
            }
            HitAction::ConfirmKillProject(id) => {
                self.ui.pending_kill = None;
                self.engine.send(EngineCmd::KillProject(id));
            }
            HitAction::CancelKillConfirm => {
                self.ui.pending_kill = None;
                let _ = self.render_frame();
            }
            HitAction::RequestKillProcess(key) => {
                self.ui.pending_kill = Some(PendingKill::Process(key));
                let _ = self.render_frame();
            }
            HitAction::ConfirmKillProcess(key) => {
                self.ui.pending_kill = None;
                self.engine.send(EngineCmd::Kill { key, tree: true });
            }
            HitAction::RestartProject(id) => {
                self.engine.send(EngineCmd::RestartProject(id));
            }
            HitAction::DismissAlert(aid) => {
                self.engine.send(EngineCmd::DismissAlert(aid));
            }
            HitAction::RestartAlertProject(pid, aid) => {
                self.engine.send(EngineCmd::DismissAlert(aid));
                self.engine.send(EngineCmd::RestartProject(pid));
            }
            HitAction::SetLanguage(lang) => {
                if self.cfg.language != lang {
                    self.cfg.language = lang;
                    self.save_cfg_or_toast();
                    self.sync_config_to_engine();
                    let _ = self.render_frame();
                }
            }
            HitAction::SetDockPosition(pos) => self.set_dock_position(pos),
            HitAction::CycleMonitor => {
                self.monitors = enumerate_monitors();
                if !self.monitors.is_empty() {
                    self.cfg.monitor_index = (self.cfg.monitor_index + 1) % self.monitors.len();
                    self.dpi = self.monitors[self.cfg.monitor_index].dpi;
                    self.save_cfg_or_toast();
                    self.recompute_targets(false);
                    let _ = self.render_frame();
                }
            }
            HitAction::ToggleAlwaysOnTop => self.toggle_always_on_top(),
            HitAction::ToggleAcrylic => {
                self.cfg.acrylic_enabled = !self.cfg.acrylic_enabled;
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
            HitAction::AdjustIdleOpacity(delta) => {
                self.cfg.idle_opacity = (self.cfg.idle_opacity + delta).clamp(0.0, 0.95);
                if self.cfg.idle_opacity < 0.03 {
                    self.cfg.idle_opacity = 0.0;
                }
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
            HitAction::AdjustOpacity(delta) => {
                self.cfg.glass_opacity = (self.cfg.glass_opacity + delta).clamp(0.25, 0.98);
                if !matches!(self.ui.mode, ViewMode::Collapsed) {
                    self.bg_alpha.snap(self.cfg.glass_opacity);
                }
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
            HitAction::ToggleFullscreenHide => {
                self.cfg.hide_on_fullscreen = !self.cfg.hide_on_fullscreen;
                self.save_cfg_or_toast();
                self.sync_fullscreen_timer();
                self.check_fullscreen();
                let _ = self.render_frame();
            }
            HitAction::ToggleCrashNotify => {
                self.cfg.notify_on_crash = !self.cfg.notify_on_crash;
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
            HitAction::ToggleHotkeyPreset => {
                self.cfg.hotkey = match self.cfg.hotkey {
                    HotkeyPreset::AltSpace => HotkeyPreset::CtrlShiftP,
                    HotkeyPreset::CtrlShiftP => HotkeyPreset::AltSpace,
                };
                match register_hotkey(self.hwnd, self.cfg.hotkey) {
                    Ok(()) => self.ui.hotkey_failed = false,
                    Err(err) => {
                        crate::config::log_error(&format!("hotkey registration failed: {err}"));
                        self.ui.hotkey_failed = true;
                    }
                }
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
            HitAction::ToggleAutostart => self.toggle_autostart(),
            HitAction::AdjustPeekDelay(delta) => {
                let next = (self.cfg.hover_peek_delay_ms as i32 + delta).clamp(80, 600);
                self.cfg.hover_peek_delay_ms = next as u32;
                self.save_cfg_or_toast();
                let _ = self.render_frame();
            }
        }
    }

    pub fn set_dock_position(&mut self, pos: DockPosition) {
        if self.cfg.dock_position == pos {
            return;
        }
        self.cfg.dock_position = pos;
        self.last_collapsed_sig = None;
        self.save_cfg_or_toast();
        // Animate the move across the screen using spring.morph!
        self.recompute_targets(false);
        let _ = self.render_frame();
    }

    pub fn toggle_autostart(&mut self) {
        let next = !self.cfg.autostart;
        if autostart::set_autostart_enabled(next).is_ok() {
            self.cfg.autostart = next;
            self.save_cfg_or_toast();
            let _ = self.render_frame();
        }
    }

    pub fn toggle_always_on_top(&mut self) {
        self.cfg.always_on_top = !self.cfg.always_on_top;
        set_window_topmost(self.hwnd, self.cfg.always_on_top);
        self.save_cfg_or_toast();
        let _ = self.render_frame();
    }

    pub fn apply_tray_action(&mut self, act: TrayAction) {
        match act {
            TrayAction::TogglePanel => self.toggle_expanded(),
            TrayAction::SetDock(pos) => self.set_dock_position(pos),
            TrayAction::ToggleAlwaysOnTop => self.toggle_always_on_top(),
            TrayAction::ToggleAutostart => self.toggle_autostart(),
            TrayAction::ToggleFullscreenHide => {
                self.cfg.hide_on_fullscreen = !self.cfg.hide_on_fullscreen;
                self.save_cfg_or_toast();
                self.sync_fullscreen_timer();
                self.check_fullscreen();
            }
            TrayAction::OpenSettings => {
                if self.fullscreen_hidden {
                    self.fullscreen_hidden = false;
                    unsafe {
                        let _ = ShowWindow(self.hwnd, SW_SHOW);
                    }
                }
                self.set_mode(ViewMode::Settings, true);
            }
            TrayAction::Quit => {
                self.engine.send(EngineCmd::Shutdown);
                unsafe {
                    PostQuitMessage(0);
                }
            }
        }
    }

    pub fn on_dpi_or_display_changed(&mut self, new_dpi: Option<u32>) {
        if let Some(d) = new_dpi {
            self.dpi = d;
        } else {
            self.dpi = window_dpi(self.hwnd);
        }
        self.monitors = enumerate_monitors();
        self.last_collapsed_sig = None;
        set_window_topmost(self.hwnd, self.cfg.always_on_top);
        self.recompute_targets(true);
        let _ = self.render_frame();
    }

    pub fn on_setting_change(&mut self) {
        self.reduced_motion = is_reduced_motion();
        self.last_collapsed_sig = None;
        let _ = self.render_frame();
    }

    pub fn on_deactivated(&mut self) {
        if self.menu_open {
            return;
        }
        if !self.ui.sticky
            && matches!(
                self.ui.mode,
                ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings
            )
        {
            self.set_mode(ViewMode::Collapsed, false);
        }
    }

    pub fn collapse_if_unfocused(&mut self) {
        if !self.menu_open
            && !self.ui.sticky
            && matches!(
                self.ui.mode,
                ViewMode::Expanded | ViewMode::Detail { .. } | ViewMode::Settings
            )
            && !is_foreground_window(self.hwnd)
        {
            self.set_mode(ViewMode::Collapsed, false);
        }
    }

    fn sync_engine_intervals(&self) {
        self.engine.send(EngineCmd::SetInterval {
            collapsed_ms: Some(self.cfg.collapsed_poll_ms),
            expanded_ms: Some(self.cfg.expanded_poll_ms),
        });
    }

    fn sync_config_to_engine(&self) {
        self.engine.send(EngineCmd::SyncConfigState {
            pinned_projects: self.cfg.pinned_projects.clone(),
            hidden_projects: self.cfg.hidden_projects.clone(),
            project_aliases: self.cfg.project_aliases.clone(),
            project_order: self.cfg.project_order.clone(),
            job_order: self.cfg.job_order.clone(),
            sort_mode: self.cfg.sort_mode,
            language: self.cfg.language,
        });
    }
}

fn reorder_vec(order: &mut Vec<String>, visible_ids: &[String], target: &str, delta: i32) -> bool {
    for id in visible_ids {
        if !order.iter().any(|x| x.eq_ignore_ascii_case(id)) {
            order.push(id.clone());
        }
    }
    let Some(idx) = order.iter().position(|x| x.eq_ignore_ascii_case(target)) else {
        return false;
    };
    if order.len() < 2 {
        return false;
    }
    let next = (idx as isize + delta as isize).clamp(0, order.len() as isize - 1) as usize;
    if next == idx {
        return false;
    }
    order.swap(idx, next);
    true
}

fn derive_match_hint(raw: &str) -> String {
    let tokens = crate::engine::project_resolver::split_cmdline_args(raw);
    if tokens.is_empty() {
        return String::new();
    }
    for tok in &tokens {
        let clean = tok.trim_matches(|c| c == '"' || c == '\'');
        let low = clean.to_ascii_lowercase();
        if low.ends_with(".py")
            || low.ends_with(".pyw")
            || low.ends_with(".js")
            || low.ends_with(".mjs")
            || low.ends_with(".ts")
            || low.ends_with(".bat")
            || low.ends_with(".cmd")
            || low.ends_with(".ps1")
        {
            if let Some(name) = std::path::Path::new(clean).file_name().and_then(|s| s.to_str()) {
                return name.to_string();
            }
        }
    }
    let first = tokens[0].trim_matches(|c| c == '"' || c == '\'');
    std::path::Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first)
        .to_string()
}

fn derive_workdir_from_cmd(raw: &str) -> Option<String> {
    for tok in crate::engine::project_resolver::split_cmdline_args(raw) {
        let clean = tok.trim_matches(|c| c == '"' || c == '\'');
        let p = std::path::Path::new(clean);
        if p.is_absolute() {
            if let Some(parent) = p.parent() {
                if parent.is_dir() {
                    return Some(parent.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

pub fn paste_from_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return None;
        }
        let mut out = None;
        if let Ok(hdata) = GetClipboardData(13u32 /* CF_UNICODETEXT */) {
            let hmem = windows::Win32::Foundation::HGLOBAL(hdata.0);
            let ptr = GlobalLock(hmem) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while len < 4096 && *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                out = Some(String::from_utf16_lossy(slice));
                let _ = GlobalUnlock(hmem);
            }
        }
        let _ = CloseClipboard();
        out
    }
}

fn apply_text_edit_key(
    buf: &mut String,
    all_selected: &mut bool,
    vk: u16,
    ctrl_down: bool,
) -> Option<bool> {
    if ctrl_down && vk == VK_A.0 {
        *all_selected = !buf.is_empty();
        return Some(true);
    }
    if ctrl_down && (vk == VK_U.0 || vk == VK_DELETE.0) {
        buf.clear();
        *all_selected = false;
        return Some(true);
    }
    if ctrl_down && vk == VK_BACK.0 {
        if *all_selected {
            buf.clear();
            *all_selected = false;
        } else {
            pop_last_word(buf);
        }
        return Some(true);
    }
    if vk == VK_BACK.0 {
        if *all_selected {
            buf.clear();
            *all_selected = false;
            return Some(true);
        }
        return Some(buf.pop().is_some());
    }
    if vk == VK_DELETE.0 && *all_selected {
        buf.clear();
        *all_selected = false;
        return Some(true);
    }
    None
}

fn handle_text_edit_shortcut(
    buf: &mut String,
    all_selected: &mut bool,
    vk: u16,
    ctrl_down: bool,
    hwnd: HWND,
) -> bool {
    if let Some(changed) = apply_text_edit_key(buf, all_selected, vk, ctrl_down) {
        return changed;
    }
    if ctrl_down && vk == VK_C.0 {
        if !buf.is_empty() {
            copy_to_clipboard(hwnd, buf);
        }
        return true;
    }
    if ctrl_down && vk == VK_V.0 {
        if let Some(clip) = paste_from_clipboard(hwnd) {
            if *all_selected {
                buf.clear();
                *all_selected = false;
            }
            buf.push_str(clip.trim());
            return true;
        }
    }
    false
}

fn collapsed_visual_sig(snap: &Snapshot) -> u64 {
    let mut h = DefaultHasher::new();
    let active_alert = snap.alerts.iter().find(|a| !a.dismissed);
    active_alert.map(|a| a.id).hash(&mut h);

    let crashed_count = snap
        .projects
        .iter()
        .flat_map(|p| p.jobs.iter())
        .filter(|j| j.status == crate::model::ProjectStatus::Crashed)
        .count();
    let any_high = snap
        .projects
        .iter()
        .any(|p| p.status == crate::model::ProjectStatus::HighLoad);
    crashed_count.hash(&mut h);
    any_high.hash(&mut h);
    snap.total_jobs.hash(&mut h);
    snap.system_cpu_history[14..24].hash(&mut h);

    let cpu_disp = if snap.total_dev_cpu > 0.05 {
        snap.total_dev_cpu
    } else {
        snap.system_cpu
    };
    (cpu_disp.round() as i32).hash(&mut h);
    (snap.system_cpu.round() as i32).hash(&mut h);

    let ram_bytes = if snap.total_dev_ram_bytes > 0 {
        snap.total_dev_ram_bytes
    } else {
        snap.system_ram_used_bytes
    };
    format_ram(ram_bytes).hash(&mut h);
    let ram_denom = (snap.system_ram_total_bytes as f32 * 0.20).max(512.0 * 1024.0 * 1024.0);
    let ram_ratio_pct = if snap.total_dev_ram_bytes > 0 {
        ((snap.total_dev_ram_bytes as f32 / ram_denom).clamp(0.08, 1.0) * 100.0).round() as u32
    } else if snap.system_ram_total_bytes > 0 {
        ((snap.system_ram_used_bytes as f32 / snap.system_ram_total_bytes as f32).clamp(0.08, 1.0)
            * 100.0)
            .round() as u32
    } else {
        10
    };
    ram_ratio_pct.hash(&mut h);

    let sys_ram_pct = if snap.system_ram_total_bytes > 0 {
        ((snap.system_ram_used_bytes as f32 / snap.system_ram_total_bytes as f32).clamp(0.0, 1.0)
            * 100.0)
            .round() as u32
    } else {
        0
    };
    sys_ram_pct.hash(&mut h);

    for proj in snap.projects.iter().filter(|p| !p.jobs.is_empty()).take(8) {
        proj.id.0.hash(&mut h);
        (proj.status as u8).hash(&mut h);
        proj.color.hash(&mut h);
        (proj.ram / (8 * 1024 * 1024)).hash(&mut h);
    }
    h.finish()
}

fn shell_open(target: &str) {
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    let tgt: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(tgt.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        );
    }
}

pub fn copy_to_clipboard(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let ptr = GlobalLock(hmem) as *mut u16;
            if !ptr.is_null() {
                core::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                let _ = GlobalUnlock(hmem);
                if SetClipboardData(
                    13u32, /* CF_UNICODETEXT */
                    windows::Win32::Foundation::HANDLE(hmem.0),
                )
                .is_err()
                {
                    let _ = GlobalFree(hmem);
                }
            } else {
                let _ = GlobalFree(hmem);
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn compute_envelope_rect(
    cur: (f32, f32, i32, i32),
    tgt: (f32, f32, i32, i32),
    pad: i32,
) -> (i32, i32, i32, i32) {
    let l = cur.0.floor().min(tgt.0.floor()) as i32 - pad;
    let t = cur.1.floor().min(tgt.1.floor()) as i32 - pad;
    let r = (cur.0.ceil() as i32 + cur.2).max(tgt.0.ceil() as i32 + tgt.2) + pad;
    let b = (cur.1.ceil() as i32 + cur.3).max(tgt.1.ceil() as i32 + tgt.3) + pad;
    (l, t, (r - l).max(1), (b - t).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_visual_sig_ignores_tiny_jitter_but_tracks_visible_changes() {
        let mut snap = Snapshot {
            total_jobs: 2,
            system_ram_total_bytes: 32 * 1024 * 1024 * 1024,
            total_dev_ram_bytes: 512 * 1024 * 1024,
            total_dev_cpu: 12.1,
            system_cpu: 8.2,
            ..Default::default()
        };
        let s1 = collapsed_visual_sig(&snap);

        snap.total_dev_cpu = 12.2;
        snap.system_cpu = 8.3;
        snap.total_dev_ram_bytes += 512;
        assert_eq!(s1, collapsed_visual_sig(&snap));

        snap.total_dev_cpu = 14.0;
        assert_ne!(s1, collapsed_visual_sig(&snap));
    }

    #[test]
    fn reorder_vec_swaps_and_clamps() {
        let mut order = Vec::new();
        let vis = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(reorder_vec(&mut order, &vis, "b", -1));
        assert_eq!(order, vec!["b", "a", "c"]);
        assert!(!reorder_vec(&mut order, &vis, "b", -1));
        assert!(reorder_vec(&mut order, &vis, "a", 1));
        assert_eq!(order, vec!["b", "c", "a"]);
    }

    #[test]
    fn derive_match_hint_extracts_script_or_exe() {
        assert_eq!(
            derive_match_hint(r#"python "C:\work\my_bot.py" --port 8080"#),
            "my_bot.py"
        );
        assert_eq!(
            derive_match_hint(r#"python "C:\My Work\Projects\worker bot.py" --port 8080"#),
            "worker bot.py"
        );
        assert_eq!(derive_match_hint("redis-server.exe"), "redis-server.exe");
    }

    #[test]
    fn collect_unnotified_crash_alerts_delivers_all_simultaneous_crashes_in_order() {
        use crate::model::{JobId, ProjectId};
        let alerts = vec![
            Alert {
                id: 3,
                project_id: ProjectId("p3".into()),
                project_name: "P3".into(),
                kind: AlertKind::Crash {
                    pid: 30,
                    process_name: "node".into(),
                    exit_code: 1,
                    can_restart: false,
                },
                message: "P3 crashed".into(),
                timestamp_ms: 300,
                dismissed: false,
            },
            Alert {
                id: 2,
                project_id: ProjectId("p2".into()),
                project_name: "P2".into(),
                kind: AlertKind::HighCpu {
                    cpu_percent: 95.0,
                    duration_secs: 20,
                },
                message: "P2 cpu".into(),
                timestamp_ms: 200,
                dismissed: false,
            },
            Alert {
                id: 1,
                project_id: ProjectId("p1".into()),
                project_name: "P1".into(),
                kind: AlertKind::Crash {
                    pid: 10,
                    process_name: "python".into(),
                    exit_code: 2,
                    can_restart: true,
                },
                message: "P1 crashed".into(),
                timestamp_ms: 100,
                dismissed: false,
            },
        ];
        let _ = JobId("unused".into());
        let mut last_id = 0;
        let out = collect_unnotified_crash_alerts(&alerts, &mut last_id);
        assert_eq!(last_id, 3);
        let ids: Vec<u64> = out.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn text_edit_shortcuts_support_ctrl_backspace_ctrl_a_and_ctrl_u() {
        let mut buf = r"python C:\dev\api\server.py".to_string();
        let mut sel = false;

        // Ctrl+Backspace pops last word/segment
        assert_eq!(apply_text_edit_key(&mut buf, &mut sel, VK_BACK.0, true), Some(true));
        assert_eq!(buf, r"python C:\dev\api\server.");
        assert_eq!(apply_text_edit_key(&mut buf, &mut sel, VK_BACK.0, true), Some(true));
        assert_eq!(buf, r"python C:\dev\api\");

        // Ctrl+A followed by Backspace clears entire field
        assert_eq!(apply_text_edit_key(&mut buf, &mut sel, VK_A.0, true), Some(true));
        assert!(sel);
        assert_eq!(apply_text_edit_key(&mut buf, &mut sel, VK_BACK.0, false), Some(true));
        assert!(buf.is_empty());
        assert!(!sel);

        // Ctrl+U clears immediately
        buf = "some query".to_string();
        assert_eq!(apply_text_edit_key(&mut buf, &mut sel, VK_U.0, true), Some(true));
        assert!(buf.is_empty());
    }

    #[test]
    fn envelope_rect_and_island_coord_mapping_are_exact() {
        // Collapsing from 536x700 top-center pill to 288x30 pill must cover BOTH rects + pad.
        let collapsed = (816.0f32, 2.0f32, 288i32, 30i32);
        let expanded = (692.0f32, 2.0f32, 536i32, 700i32);
        let env = compute_envelope_rect(collapsed, expanded, 14);
        assert_eq!(env, (678, -12, 564, 728));

        // Relative offset maps both endpoints safely inside envelope:
        let off_exp_x = expanded.0 - env.0 as f32;
        let off_col_x = collapsed.0 - env.0 as f32;
        assert_eq!(off_exp_x, 14.0);
        assert_eq!(off_col_x, 138.0);
        assert!(off_exp_x + expanded.2 as f32 <= env.2 as f32 - 14.0);
        assert!(off_col_x + collapsed.2 as f32 <= env.2 as f32 - 14.0);
    }
}
