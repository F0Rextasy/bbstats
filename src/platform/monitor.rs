//! Multi-monitor enumeration and dock-position rect calculation.

use crate::config::DockPosition;
use crate::platform::dpi::{dip_to_px, px_to_dip, BASE_DPI};
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub monitor_rect: RECT,
    pub work_rect: RECT,
    pub dpi: u32,
    pub is_primary: bool,
}

unsafe extern "system" fn enum_monitor_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut _).as_bool() {
        let mut dpi_x = BASE_DPI;
        let mut dpi_y = BASE_DPI;
        let _ = GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);

        list.push(MonitorInfo {
            monitor_rect: info.monitorInfo.rcMonitor,
            work_rect: info.monitorInfo.rcWork,
            dpi: dpi_x,
            is_primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
        });
    }
    BOOL(1)
}

/// Enumerates all active display monitors, primary first.
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    let mut list = Vec::with_capacity(4);
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_proc),
            LPARAM(&mut list as *mut Vec<MonitorInfo> as isize),
        );
    }
    list.sort_by_key(|m: &MonitorInfo| !m.is_primary);
    list
}

/// Picks the configured monitor by index, falling back to primary.
pub fn select_monitor(monitors: &[MonitorInfo], index: usize) -> Option<&MonitorInfo> {
    monitors.get(index).or_else(|| monitors.first())
}

/// Returns usable work-area height in DIPs for the given monitor.
pub fn monitor_work_height_dip(monitor: &MonitorInfo) -> f32 {
    let px = (monitor.work_rect.bottom - monitor.work_rect.top).max(300);
    px_to_dip(px, monitor.dpi)
}

/// Computes the physical screen rect `(x, y, width, height)` in pixels for the island.
///
/// - `TopCenter`: horizontally centered snugly near top of work area (2 DIP gap).
/// - `RightEdge`: slim right-edge dock anchored near `work.right`; spans top-to-bottom when rail-sized.
pub fn compute_dock_rect(
    monitor: &MonitorInfo,
    dock: DockPosition,
    width_dip: f32,
    height_dip: f32,
    shadow_pad_dip: f32,
) -> (i32, i32, i32, i32) {
    let dpi = monitor.dpi;
    let total_w_px = dip_to_px(width_dip + shadow_pad_dip * 2.0, dpi);
    let total_h_px = dip_to_px(height_dip + shadow_pad_dip * 2.0, dpi);
    let pad_px = dip_to_px(shadow_pad_dip, dpi);

    let work = monitor.work_rect;
    let mon = monitor.monitor_rect;
    let work_w = work.right - work.left;
    let work_h = work.bottom - work.top;

    match dock {
        DockPosition::TopCenter => {
            let x = work.left + (work_w - total_w_px) / 2;
            let top_gap_px = dip_to_px(2.0, dpi);
            let top_base = work.top.max(mon.top);
            let y = top_base + top_gap_px - pad_px;
            (x, y, total_w_px, total_h_px)
        }
        DockPosition::RightEdge => {
            let right_gap_px = dip_to_px(4.0, dpi);
            let x = work.right - right_gap_px - (total_w_px - pad_px);
            let top_margin_px = dip_to_px(6.0, dpi);
            let y = if total_h_px >= work_h - top_margin_px * 3 {
                work.top + top_margin_px - pad_px
            } else {
                (work.top + (work_h - total_h_px) / 2).max(work.top + top_margin_px - pad_px)
            };
            (x, y, total_w_px, total_h_px)
        }
    }
}
