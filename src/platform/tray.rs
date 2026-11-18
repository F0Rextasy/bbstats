//! System tray icon (`Shell_NotifyIconW`) and Win32 context menu.

use crate::config::{AppConfig, DockPosition};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    SetForegroundWindow, TrackPopupMenuEx, HICON, ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP,
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
        let icon = create_pill_icon();
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

#[allow(clippy::too_many_arguments)]
fn sdf_letter_b(
    fx: f32,
    fy: f32,
    stem_x: f32,
    top_y: f32,
    bot_y: f32,
    bowl_cx: f32,
    bowl_cy: f32,
    bowl_r: f32,
    stroke: f32,
) -> f32 {
    let half = stroke * 0.5;
    let y0 = top_y + half;
    let y1 = (bot_y - half).max(y0);
    let dy_stem = fy - fy.clamp(y0, y1);
    let d_stem = (fx - stem_x).hypot(dy_stem) - half;
    let d_bowl = ((fx - bowl_cx).hypot(fy - bowl_cy) - bowl_r).abs() - half;
    d_stem.min(d_bowl)
}

/// Renders a crisp 32x32 ARGB obsidian squircle icon featuring the `bb` monogram and live dot.
fn create_pill_icon() -> HICON {
    const W: i32 = 32;
    const H: i32 = 32;

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: W,
            biHeight: -H, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    unsafe {
        let mut bits_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
        let hdc = CreateCompatibleDC(None);
        let hbm_color: HBITMAP =
            match CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0) {
                Ok(h) => h,
                Err(_) => {
                    let _ = DeleteDC(hdc);
                    return HICON::default();
                }
            };

        if !bits_ptr.is_null() {
            let pixels = std::slice::from_raw_parts_mut(bits_ptr as *mut u32, (W * H) as usize);
            let s = W as f32;
            let margin = s * 0.06;
            let half = s * 0.5 - margin;
            let radius = s * 0.24;
            let cx = s * 0.5;
            let cy = s * 0.5;

            let stroke = s * 0.088;
            let top_y = s * 0.22;
            let bot_y = s * 0.74;
            let bowl_cy = s * 0.58;
            let bowl_r = s * 0.115;
            let dot_cx = s * 0.80;
            let dot_cy = s * 0.26;
            let dot_r = s * 0.055;

            for y in 0..H {
                for x in 0..W {
                    let fx = x as f32 + 0.5;
                    let fy = y as f32 + 0.5;

                    let qx = (fx - cx).abs() - (half - radius);
                    let qy = (fy - cy).abs() - (half - radius);
                    let sdf = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;

                    let cov = (0.7 - sdf).clamp(0.0, 1.0);
                    if cov <= 0.0 {
                        pixels[(y * W + x) as usize] = 0;
                        continue;
                    }

                    let mut r = 16.0_f32;
                    let mut g = 19.0_f32;
                    let mut b = 26.0_f32;

                    if sdf > -1.45 {
                        let border_t = ((sdf + 1.45) / 1.45).clamp(0.0, 1.0);
                        let top_mix = ((1.0 - fy / s) * 0.8 + 0.2).clamp(0.2, 1.0);
                        r += (82.0 - r) * border_t * top_mix;
                        g += (142.0 - g) * border_t * top_mix;
                        b += (210.0 - b) * border_t * top_mix;
                    }

                    let b1_cov = (0.6
                        - sdf_letter_b(
                            fx,
                            fy,
                            s * 0.205,
                            top_y,
                            bot_y,
                            s * 0.32,
                            bowl_cy,
                            bowl_r,
                            stroke,
                        ))
                    .clamp(0.0, 1.0);
                    if b1_cov > 0.0 {
                        r += (242.0 - r) * b1_cov;
                        g += (246.0 - g) * b1_cov;
                        b += (252.0 - b) * b1_cov;
                    }

                    let b2_cov = (0.6
                        - sdf_letter_b(
                            fx,
                            fy,
                            s * 0.565,
                            top_y,
                            bot_y,
                            s * 0.68,
                            bowl_cy,
                            bowl_r,
                            stroke,
                        ))
                    .clamp(0.0, 1.0);
                    if b2_cov > 0.0 {
                        let gt = ((fy - top_y) / (bot_y - top_y).max(1.0)).clamp(0.0, 1.0);
                        let tr = 52.0 + (10.0 - 52.0) * gt;
                        let tg = 211.0 + (132.0 - 211.0) * gt;
                        let tb = 153.0 + (255.0 - 153.0) * gt;
                        r += (tr - r) * b2_cov;
                        g += (tg - g) * b2_cov;
                        b += (tb - b) * b2_cov;
                    }

                    let d_dot = (fx - dot_cx).hypot(fy - dot_cy) - dot_r;
                    let glow = (1.0 - (d_dot / (dot_r * 2.2))).clamp(0.0, 1.0) * 0.30;
                    if glow > 0.0 {
                        r = r * (1.0 - glow) + 52.0 * glow;
                        g = g * (1.0 - glow) + 211.0 * glow;
                        b = b * (1.0 - glow) + 153.0 * glow;
                    }
                    let dot_cov = (0.6 - d_dot).clamp(0.0, 1.0);
                    if dot_cov > 0.0 {
                        r += (52.0 - r) * dot_cov;
                        g += (211.0 - g) * dot_cov;
                        b += (153.0 - b) * dot_cov;
                    }

                    let a = (cov * 255.0).round() as u32;
                    let pr = ((r * cov).round() as u32).min(255);
                    let pg = ((g * cov).round() as u32).min(255);
                    let pb = ((b * cov).round() as u32).min(255);
                    pixels[(y * W + x) as usize] = (a << 24) | (pr << 16) | (pg << 8) | pb;
                }
            }
        }

        let hbm_mask = CreateBitmap(W, H, 1, 1, None);
        let ii = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(hbm_color);
        let _ = DeleteObject(hbm_mask);
        let _ = DeleteDC(hdc);
        icon
    }
}
