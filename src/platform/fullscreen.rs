//! Fullscreen application / game detection to auto-hide the Dynamic Island.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsWindowVisible,
    GWL_STYLE, WS_CAPTION, WS_CHILD,
};

/// Returns `true` if a fullscreen game, presentation, or borderless fullscreen video is active.
pub fn is_fullscreen_active(own_hwnd: HWND) -> bool {
    unsafe {
        if let Ok(state) = SHQueryUserNotificationState() {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
                return true;
            }
        }

        let fg = GetForegroundWindow();
        if fg.0.is_null() || fg == own_hwnd || !IsWindowVisible(fg).as_bool() {
            return false;
        }

        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if (style & WS_CHILD.0) != 0 {
            return false;
        }

        // Ignore desktop shell windows (Progman, WorkerW, Shell_TrayWnd).
        let mut class_buf = [0u16; 64];
        let len = GetClassNameW(fg, &mut class_buf) as usize;
        let class_name = String::from_utf16_lossy(&class_buf[..len]);
        if matches!(
            class_name.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }

        let mut fg_rect = RECT::default();
        if GetWindowRect(fg, &mut fg_rect).is_err() {
            return false;
        }

        let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
            return false;
        }

        let m = mi.rcMonitor;
        // Fullscreen borderless or F11 browser covers entire monitor without standard title bar.
        let covers_monitor = fg_rect.left <= m.left
            && fg_rect.top <= m.top
            && fg_rect.right >= m.right
            && fg_rect.bottom >= m.bottom;

        covers_monitor && (style & WS_CAPTION.0) != WS_CAPTION.0
    }
}
