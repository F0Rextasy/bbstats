//! Global hotkey registration (`Alt+Shift+B`, `Ctrl+Alt+B`, `Alt+Space`, `Ctrl+Shift+P`, or `Disabled`).

use crate::config::HotkeyPreset;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, VK_B, VK_P, VK_SPACE,
};

pub const HOTKEY_TOGGLE_ID: i32 = 0x151E;

pub fn register_hotkey(hwnd: HWND, preset: HotkeyPreset) -> Result<(), String> {
    let _ = unregister_hotkey(hwnd);
    let (mods, vk) = match preset {
        HotkeyPreset::AltShiftB => (MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, VK_B.0 as u32),
        HotkeyPreset::CtrlAltB => (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_B.0 as u32),
        HotkeyPreset::AltSpace => (MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32),
        HotkeyPreset::CtrlShiftP => (MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, VK_P.0 as u32),
        HotkeyPreset::Disabled => return Ok(()),
    };
    // SAFETY: `hwnd` is a valid top-level window handle on the UI thread.
    unsafe {
        RegisterHotKey(hwnd, HOTKEY_TOGGLE_ID, HOT_KEY_MODIFIERS(mods.0), vk)
            .map_err(|e| format!("RegisterHotKey failed: {e}"))
    }
}

pub fn unregister_hotkey(hwnd: HWND) -> Result<(), String> {
    // SAFETY: `hwnd` is the window associated with `HOTKEY_TOGGLE_ID`.
    unsafe {
        UnregisterHotKey(hwnd, HOTKEY_TOGGLE_ID)
            .map_err(|e| format!("UnregisterHotKey failed: {e}"))
    }
}
