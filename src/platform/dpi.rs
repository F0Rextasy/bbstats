//! Per-Monitor v2 DPI helpers and Windows reduced-motion detection.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

pub const BASE_DPI: u32 = 96;

/// Enables Per-Monitor v2 DPI awareness as a runtime safeguard alongside the embedded manifest.
pub fn enable_per_monitor_v2() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Returns the effective DPI for `hwnd`, defaulting to 96 if unavailable.
pub fn window_dpi(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        BASE_DPI
    } else {
        dpi
    }
}

#[inline]
pub fn dpi_scale(dpi: u32) -> f32 {
    dpi as f32 / BASE_DPI as f32
}

#[inline]
pub fn dip_to_px(dip: f32, dpi: u32) -> i32 {
    (dip * dpi_scale(dpi)).round() as i32
}

#[inline]
pub fn px_to_dip(px: i32, dpi: u32) -> f32 {
    px as f32 / dpi_scale(dpi)
}

/// Checks Windows `SPI_GETCLIENTAREAANIMATION` to respect the user's Reduced Motion setting.
pub fn is_reduced_motion() -> bool {
    let mut animations_enabled: i32 = 1;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut animations_enabled as *mut i32 as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && animations_enabled == 0
}
