//! Extracted Win32 clipboard, shell, and text-editing helpers for `app`.

use crate::ui::layout::{insert_text_at_cursor, pop_last_word};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_A, VK_BACK, VK_C, VK_DELETE, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_U, VK_V,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

pub fn is_runnable_command(raw: &str) -> bool {
    let low = raw.to_ascii_lowercase();
    raw.contains(' ')
        || raw.contains('\\')
        || raw.contains('/')
        || [
            ".py", ".pyw", ".js", ".mjs", ".ts", ".bat", ".cmd", ".ps1", ".exe",
        ]
        .iter()
        .any(|ext| low.ends_with(ext))
}

pub fn derive_match_hint(raw: &str) -> String {
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

pub fn derive_workdir_from_cmd(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("cd ") {
        if let Some((cd_part, _)) = trimmed.split_once("&&").or_else(|| trimmed.split_once(';')) {
            let cd_tokens = crate::engine::project_resolver::split_cmdline_args(cd_part);
            for tok in cd_tokens.iter().skip(1) {
                if tok.eq_ignore_ascii_case("/d") {
                    continue;
                }
                let dir_clean = tok.trim_matches(|c| c == '"' || c == '\'');
                if !dir_clean.is_empty() {
                    return Some(dir_clean.to_string());
                }
            }
        }
    }
    for tok in crate::engine::project_resolver::split_cmdline_args(raw) {
        let clean = tok.trim_matches(|c| c == '"' || c == '\'');
        let p = std::path::Path::new(clean);
        if p.is_absolute() {
            if p.is_dir() {
                return Some(p.to_string_lossy().into_owned());
            }
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
    // SAFETY: Clipboard and HGLOBAL lock/unlock stay balanced on the UI thread; read length is bounded by GlobalSize.
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return None;
        }
        let mut out = None;
        if let Ok(hdata) = GetClipboardData(13u32 /* CF_UNICODETEXT */) {
            let hmem = windows::Win32::Foundation::HGLOBAL(hdata.0);
            let max_chars = (GlobalSize(hmem) / 2).min(4096);
            let ptr = GlobalLock(hmem) as *const u16;
            if !ptr.is_null() && max_chars > 0 {
                let mut len = 0usize;
                while len < max_chars && *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let sanitized: String = String::from_utf16_lossy(slice)
                    .chars()
                    .filter_map(|c| match c {
                        '\r' | '\n' | '\t' => Some(' '),
                        c if c.is_control() => None,
                        c => Some(c),
                    })
                    .collect();
                out = Some(sanitized);
                let _ = GlobalUnlock(hmem);
            } else if !ptr.is_null() {
                let _ = GlobalUnlock(hmem);
            }
        }
        let _ = CloseClipboard();
        out
    }
}

pub fn apply_text_edit_key(
    buf: &mut String,
    all_selected: &mut bool,
    cursor_from_end: &mut usize,
    vk: u16,
    ctrl_down: bool,
) -> Option<bool> {
    let total_chars = buf.chars().count();
    *cursor_from_end = (*cursor_from_end).min(total_chars);

    if vk == VK_LEFT.0 && !ctrl_down {
        *all_selected = false;
        if *cursor_from_end < total_chars {
            *cursor_from_end += 1;
            return Some(true);
        }
        return None;
    }
    if vk == VK_RIGHT.0 && !ctrl_down {
        *all_selected = false;
        if *cursor_from_end > 0 {
            *cursor_from_end -= 1;
            return Some(true);
        }
        return None;
    }
    if vk == VK_HOME.0 {
        *all_selected = false;
        if *cursor_from_end != total_chars {
            *cursor_from_end = total_chars;
            return Some(true);
        }
        return Some(false);
    }
    if vk == VK_END.0 {
        *all_selected = false;
        if *cursor_from_end != 0 {
            *cursor_from_end = 0;
            return Some(true);
        }
        return Some(false);
    }
    if ctrl_down && vk == VK_A.0 {
        *all_selected = !buf.is_empty();
        *cursor_from_end = 0;
        return Some(true);
    }
    if ctrl_down && (vk == VK_U.0 || vk == VK_DELETE.0) {
        buf.clear();
        *all_selected = false;
        *cursor_from_end = 0;
        return Some(true);
    }
    if ctrl_down && vk == VK_BACK.0 {
        if *all_selected {
            buf.clear();
            *all_selected = false;
            *cursor_from_end = 0;
        } else {
            *cursor_from_end = 0;
            pop_last_word(buf);
        }
        return Some(true);
    }
    if vk == VK_BACK.0 {
        if *all_selected {
            buf.clear();
            *all_selected = false;
            *cursor_from_end = 0;
            return Some(true);
        }
        if *cursor_from_end == 0 {
            return Some(buf.pop().is_some());
        }
        let target_char = total_chars.saturating_sub(*cursor_from_end);
        if target_char == 0 {
            return Some(false);
        }
        if let Some((byte_idx, _)) = buf.char_indices().nth(target_char - 1) {
            buf.remove(byte_idx);
            return Some(true);
        }
        return Some(false);
    }
    if vk == VK_DELETE.0 {
        if *all_selected {
            buf.clear();
            *all_selected = false;
            *cursor_from_end = 0;
            return Some(true);
        }
        if *cursor_from_end > 0 {
            let target_char = total_chars - *cursor_from_end;
            if let Some((byte_idx, _)) = buf.char_indices().nth(target_char) {
                buf.remove(byte_idx);
                *cursor_from_end -= 1;
                return Some(true);
            }
        }
    }
    None
}

pub fn handle_text_edit_shortcut(
    buf: &mut String,
    all_selected: &mut bool,
    cursor_from_end: &mut usize,
    vk: u16,
    ctrl_down: bool,
    hwnd: HWND,
) -> bool {
    if let Some(changed) = apply_text_edit_key(buf, all_selected, cursor_from_end, vk, ctrl_down) {
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
                *cursor_from_end = 0;
            }
            insert_text_at_cursor(buf, cursor_from_end, clip.trim());
            return true;
        }
    }
    false
}

pub fn shell_open(target: &str) {
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    let tgt: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: Null-terminated UTF-16 verb and target strings remain live across the call.
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
    // SAFETY: Clipboard handle and HGLOBAL memory remain valid and balanced on the UI thread.
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
