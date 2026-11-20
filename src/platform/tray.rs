//! System tray icon (`Shell_NotifyIconW`) and Win32 context menu.

use crate::config::{AppConfig, DockPosition};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, LoadImageW,
    SetForegroundWindow, TrackPopupMenuEx, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
};

pub const WM_APP_TRAY: u32 = WM_APP + 10;

pub const IDM_TOGGLE_PANEL: u32 = 1001;
pub const IDM_POS_TOP: u32 = 1002;
pub const IDM_POS_RIGHT: u32 = 1003;
pub const IDM_AUTOSTART: u32 = 1004;
pub const IDM_FULLSCREEN_HIDE: u32 = 1005;
pub const IDM_ALWAYS_ON_TOP: u32 = 1008;
pub const IDM_SETTINGS: u32 = 1006;
pub const IDM_QUIT: u32 = 1007;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    TogglePanel,
    SetDock(DockPosition),
    ToggleAlwaysOnTop,
    ToggleAutostart,
    ToggleFullscreenHide,
    OpenSettings,
    Quit,
}

pub struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
    added: bool,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        let icon = load_tray_icon();
        let mut tray = Self {
            hwnd,
            icon,
            added: false,
        };
        tray.add();
        tray
    }

    pub fn add(&mut self) {
        let mut nid = self.base_nid();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.icon;
        let tip = "bbstats — Process Monitor";
        for (dst, src) in nid.szTip.iter_mut().zip(tip.encode_utf16()) {
            *dst = src;
        }
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                self.added = true;
            }
        }
    }

    fn base_nid(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }

    pub fn show_balloon(&self, title: &str, body: &str) {
        if !self.added {
            return;
        }
        let mut nid = self.base_nid();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        let title_cap = nid.szInfoTitle.len() - 1;
        for (dst, src) in nid
            .szInfoTitle
            .iter_mut()
            .zip(title.encode_utf16().take(title_cap))
        {
            *dst = src;
        }
        let body_cap = nid.szInfo.len() - 1;
        for (dst, src) in nid
            .szInfo
            .iter_mut()
            .zip(body.encode_utf16().take(body_cap))
        {
            *dst = src;
        }
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

/// Shows the Win32 popup context menu without requiring a borrow on `App`.
pub fn show_menu_for_hwnd(hwnd: HWND, cfg: &AppConfig) -> Option<TrayAction> {
    let lang = cfg.language;
    unsafe {
        let hmenu = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_TOGGLE_PANEL as usize,
            PCWSTR(wide(lang.pick("Toggle Panel", "Paneli Aç / Kapat")).as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

        let top_flags = MF_STRING
            | if cfg.dock_position == DockPosition::TopCenter {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let right_flags = MF_STRING
            | if cfg.dock_position == DockPosition::RightEdge {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let _ = AppendMenuW(
            hmenu,
            top_flags,
            IDM_POS_TOP as usize,
            PCWSTR(wide(lang.pick("Dock: Top Center", "Konum: Üst Orta")).as_ptr()),
        );
        let _ = AppendMenuW(
            hmenu,
            right_flags,
            IDM_POS_RIGHT as usize,
            PCWSTR(wide(lang.pick("Dock: Right Edge", "Konum: Sağ Kenar")).as_ptr()),
        );

        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let top_most_flags = MF_STRING
            | if cfg.always_on_top {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let _ = AppendMenuW(
            hmenu,
            top_most_flags,
            IDM_ALWAYS_ON_TOP as usize,
            PCWSTR(wide(lang.pick("Always on Top", "Her Zaman Üstte")).as_ptr()),
        );

        let auto_flags = MF_STRING | if cfg.autostart { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(
            hmenu,
            auto_flags,
            IDM_AUTOSTART as usize,
            PCWSTR(wide(lang.pick("Start with Windows", "Windows ile Başlat")).as_ptr()),
        );

        let fs_flags = MF_STRING
            | if cfg.hide_on_fullscreen {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let _ = AppendMenuW(
            hmenu,
            fs_flags,
            IDM_FULLSCREEN_HIDE as usize,
            PCWSTR(wide(lang.pick("Hide on Fullscreen", "Tam Ekranda Gizle")).as_ptr()),
        );

        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_SETTINGS as usize,
            PCWSTR(wide(lang.pick("Settings...", "Ayarlar...")).as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_QUIT as usize,
            PCWSTR(wide(lang.pick("Quit", "Çıkış")).as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);

        let align_flag = if pt.y < 300 {
            windows::Win32::UI::WindowsAndMessaging::TPM_TOPALIGN
        } else {
            TPM_BOTTOMALIGN
        };
        let cmd = TrackPopupMenuEx(
            hmenu,
            (TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | align_flag | TPM_RIGHTBUTTON).0,
            pt.x,
            pt.y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(hmenu);

        match cmd.0 as u32 {
            IDM_TOGGLE_PANEL => Some(TrayAction::TogglePanel),
            IDM_POS_TOP => Some(TrayAction::SetDock(DockPosition::TopCenter)),
            IDM_POS_RIGHT => Some(TrayAction::SetDock(DockPosition::RightEdge)),
            IDM_ALWAYS_ON_TOP => Some(TrayAction::ToggleAlwaysOnTop),
            IDM_AUTOSTART => Some(TrayAction::ToggleAutostart),
            IDM_FULLSCREEN_HIDE => Some(TrayAction::ToggleFullscreenHide),
            IDM_SETTINGS => Some(TrayAction::OpenSettings),
            IDM_QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            if self.added {
                let nid = self.base_nid();
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if !self.icon.0.is_null() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Loads the embedded Win32 resource icon (`MAKEINTRESOURCEW(1)`) compiled by `build.rs`.
fn load_tray_icon() -> HICON {
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return HICON::default();
        };
        LoadImageW(
            hinst,
            PCWSTR(std::ptr::without_provenance(1)),
            IMAGE_ICON,
            32,
            32,
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}
