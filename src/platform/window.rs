//! Borderless always-on-top toolwindow, Win32 message loop, and single-instance guard.

use crate::app::App;
use crate::platform::dpi::dip_to_px;
use crate::platform::hotkey::{register_hotkey, unregister_hotkey, HOTKEY_TOGGLE_ID};
use crate::platform::tray::{show_menu_for_hwnd, WM_APP_TRAY};
use crate::ui::layout::ViewMode;
use std::cell::RefCell;
use std::rc::Rc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWCP_DONOTROUND, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, LoadCursorW, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
    CREATESTRUCTW, GWLP_USERDATA, GWL_EXSTYLE, HTCLIENT, HTTRANSPARENT, HWND_NOTOPMOST,
    HWND_TOPMOST, IDC_ARROW, MA_ACTIVATE, MA_NOACTIVATE, MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNOACTIVATE, WA_INACTIVE, WM_ACTIVATE, WM_APP,
    WM_CHAR, WM_CREATE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_HOTKEY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_RBUTTONUP,
    WM_SETTINGCHANGE, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_CLIPCHILDREN, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

pub const WM_APP_SNAPSHOT: u32 = WM_APP + 1;

thread_local! {
    static TASKBAR_CREATED_MSG: RefCell<u32> = const { RefCell::new(0) };
}

pub struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub fn acquire_single_instance() -> Option<SingleInstanceGuard> {
    let name: Vec<u16> = "Local\\BbstatsDynamicIslandProcessMonitor\0"
        .encode_utf16()
        .collect();
    // SAFETY: Process/window Win32 handles and callbacks remain valid within scope.
    unsafe {
        let h = CreateMutexW(None, false, PCWSTR(name.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = windows::Win32::Foundation::CloseHandle(h);
            return None;
        }
        Some(SingleInstanceGuard(h))
    }
}

pub fn create_island_hwnd(
    x: i32,
    y: i32,
    w_dip: f32,
    h_dip: f32,
    dpi: u32,
    always_on_top: bool,
) -> windows::core::Result<HWND> {
    let class_name: Vec<u16> = "BbstatsIslandWindow\0".encode_utf16().collect();
    let title: Vec<u16> = "bbstats\0".encode_utf16().collect();
    let tb_msg_name: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();

    // SAFETY: Called by Win32 with matching system ABI; pointer arguments are checked before dereference.
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let app_icon = LoadIconW(hinstance, PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            hIcon: app_icon,
            hIconSm: app_icon,
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let tb_msg = RegisterWindowMessageW(PCWSTR(tb_msg_name.as_ptr()));
        TASKBAR_CREATED_MSG.with(|c| *c.borrow_mut() = tb_msg);

        let w_px = dip_to_px(w_dip, dpi).max(1);
        let h_px = dip_to_px(h_dip, dpi).max(1);

        let mut ex_style = WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE;
        if always_on_top {
            ex_style |= WS_EX_TOPMOST;
        }

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CLIPCHILDREN,
            x,
            y,
            w_px,
            h_px,
            None,
            None,
            hinstance,
            None,
        )?;

        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&corner) as u32,
        );

        Ok(hwnd)
    }
}

pub fn set_window_topmost(hwnd: HWND, topmost: bool) {
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    unsafe {
        let insert_after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn set_window_activatable(hwnd: HWND, activatable: bool) {
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let no_act = WS_EX_NOACTIVATE.0 as isize;
        let next = if activatable {
            cur & !no_act
        } else {
            cur | no_act
        };
        if next != cur {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn is_foreground_window(hwnd: HWND) -> bool {
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    unsafe { GetForegroundWindow() == hwnd }
}

pub fn attach_and_run(hwnd: HWND, app: Rc<RefCell<App>>) {
    let ptr = Rc::into_raw(app);
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        let hotkey = {
            let borrowed = &*ptr;
            borrowed.borrow().cfg.hotkey
        };
        if let Err(err) = register_hotkey(hwnd, hotkey) {
            crate::config::log_error(&format!("hotkey registration failed: {err}"));
            let borrowed = &*ptr;
            borrowed.borrow_mut().ui.hotkey_failed = true;
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = unregister_hotkey(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = Rc::from_raw(ptr);
    }
}

pub fn post_snapshot_wake(hwnd_raw: isize) {
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        let _ = PostMessageW(hwnd, WM_APP_SNAPSHOT, WPARAM(0), LPARAM(0));
    }
}

fn with_app<R>(hwnd: HWND, f: impl FnOnce(&mut App) -> R) -> Option<R> {
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<App>;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: HWND is valid for this window's lifetime on the UI thread.
    let cell = unsafe { &*ptr };
    // Use try_borrow_mut so synchronous Win32 re-entrant messages (e.g. during SetWindowPos) never panic.
    let mut borrowed = cell.try_borrow_mut().ok()?;
    Some(f(&mut borrowed))
}

fn show_popup_menu_safely(hwnd: HWND, cfg: crate::config::AppConfig) {
    let action = show_menu_for_hwnd(hwnd, &cfg);
    with_app(hwnd, |app| {
        app.menu_open = false;
        if let Some(act) = action {
            app.apply_tray_action(act);
        } else {
            app.collapse_if_unfocused();
        }
    });
}

#[inline]
fn signed_loword(v: usize) -> i32 {
    (v & 0xFFFF) as u16 as i16 as i32
}

#[inline]
fn signed_hiword(v: usize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i16 as i32
}

// SAFETY: Called by Win32 with matching system ABI; pointer arguments are checked before dereference.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let tb_msg = TASKBAR_CREATED_MSG.with(|c| *c.borrow());
    if tb_msg != 0 && msg == tb_msg {
        with_app(hwnd, |app| app.tray.add(app.cfg.language));
        return LRESULT(0);
    }

    match msg {
        WM_CREATE => {
            let cs = lparam.0 as *const CREATESTRUCTW;
            if !cs.is_null() && !(*cs).lpCreateParams.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            }
            LRESULT(0)
        }
        WM_APP_SNAPSHOT => {
            with_app(hwnd, |app| app.on_snapshot());
            LRESULT(0)
        }
        WM_TIMER => {
            with_app(hwnd, |app| app.on_timer(wparam.0));
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let mut pt = POINT {
                x: signed_loword(lparam.0 as usize),
                y: signed_hiword(lparam.0 as usize),
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            let inside = with_app(hwnd, |app| app.is_point_inside_pill(pt.x, pt.y)).unwrap_or(true);
            if inside {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        WM_MOUSEACTIVATE => {
            let activate = with_app(hwnd, |app| {
                !matches!(app.ui.mode, ViewMode::Collapsed | ViewMode::HoverPeek)
            })
            .unwrap_or(false);
            if activate {
                LRESULT(MA_ACTIVATE as isize)
            } else {
                LRESULT(MA_NOACTIVATE as isize)
            }
        }
        WM_MOUSEMOVE => {
            let x = signed_loword(lparam.0 as usize);
            let y = signed_hiword(lparam.0 as usize);
            with_app(hwnd, |app| app.on_mouse_move(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(hwnd, |app| app.on_mouse_leave());
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = signed_loword(lparam.0 as usize);
            let y = signed_hiword(lparam.0 as usize);
            with_app(hwnd, |app| app.on_left_click(x, y));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let x = signed_loword(lparam.0 as usize);
            let y = signed_hiword(lparam.0 as usize);
            if let Some(Some(cfg)) = with_app(hwnd, |app| app.on_right_click(x, y)) {
                show_popup_menu_safely(hwnd, cfg);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            let mut pt = POINT {
                x: signed_loword(lparam.0 as usize),
                y: signed_hiword(lparam.0 as usize),
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            with_app(hwnd, |app| app.on_mouse_wheel(delta, pt.x, pt.y));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            with_app(hwnd, |app| app.on_key_down(wparam.0 as u16));
            LRESULT(0)
        }
        WM_SYSKEYDOWN => {
            let handled =
                with_app(hwnd, |app| app.on_sys_key_down(wparam.0 as u16)).unwrap_or(false);
            if handled {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CHAR => {
            if let Some(ch) = char::from_u32(wparam.0 as u32) {
                with_app(hwnd, |app| app.on_char(ch));
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if wparam.0 as i32 == HOTKEY_TOGGLE_ID {
                with_app(hwnd, |app| app.toggle_expanded());
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let state = (wparam.0 & 0xFFFF) as u32;
            if state == WA_INACTIVE {
                with_app(hwnd, |app| app.on_deactivated());
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            with_app(hwnd, |app| app.on_dpi_or_display_changed(Some(new_dpi)));
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            with_app(hwnd, |app| app.on_dpi_or_display_changed(None));
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            with_app(hwnd, |app| app.on_setting_change());
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU {
                if let Some(cfg) = with_app(hwnd, |app| {
                    app.menu_open = true;
                    app.cfg.clone()
                }) {
                    show_popup_menu_safely(hwnd, cfg);
                }
            } else if event == windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP
                || event == 0x0405
            {
                with_app(hwnd, |app| app.toggle_expanded());
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_app_icon_and_ico_file_are_valid() {
        let ico_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("bbstats.ico");
        let ico = std::fs::read(&ico_path).expect("assets/bbstats.ico should exist");
        assert!(ico.len() > 54);
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 3, 0]);

        // SAFETY: FFI/Win32 call uses valid handles and initialized buffers scoped to this call.
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap();
            let icon = LoadIconW(hinst, PCWSTR(std::ptr::without_provenance(1)))
                .expect("embedded icon ID 1");
            assert!(!icon.is_invalid());
        }
    }
}
