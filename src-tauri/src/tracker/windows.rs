//! Frontmost-window lookup on Windows.
//!
//! NOTE: written but not compiled or run — development happened on macOS.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};

use super::TermWindow;

/// Windows has no cheap equivalent of the CoreGraphics window list, and the
/// overlay only ever needs the foreground window — so this returns at most one
/// entry, always at level 0.
pub fn list_windows() -> Vec<(i64, TermWindow)> {
    foreground().into_iter().map(|w| (0, w)).collect()
}

fn foreground() -> Option<TermWindow> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;

        // GetWindowRect is in physical pixels; Tauri positions in logical units.
        let dpi = GetDpiForWindow(hwnd);
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        Some(TermWindow {
            app: process_name(hwnd)?,
            pid,
            x: rect.left as f64 / scale,
            y: rect.top as f64 / scale,
            w: (rect.right - rect.left) as f64 / scale,
            h: (rect.bottom - rect.top) as f64 / scale,
        })
    }
}

unsafe fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return None;
    }
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

    let mut buf = [0u16; MAX_PATH as usize];
    let mut len = buf.len() as u32;
    let result = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_FORMAT(0),
        PWSTR(buf.as_mut_ptr()),
        &mut len,
    );
    let _ = CloseHandle(handle);
    result.ok()?;

    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit(['\\', '/']).next().map(|s| s.to_string())
}
