//! The app in the foreground: executable name and window title.

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindow, SetForegroundWindow};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::{w, PWSTR};

fn exe_of(hwnd: HWND) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return String::new();
        }
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().unwrap_or("").to_lowercase()
    }
}

pub fn foreground_app() -> (String, String) {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return (String::new(), String::new());
        }
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut title);
        let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);
        let mut exe = exe_of(hwnd);
        // Store apps run inside ApplicationFrameHost; the real app owns a child CoreWindow.
        if exe == "applicationframehost.exe" {
            if let Ok(child) = FindWindowExW(Some(hwnd), None, w!("Windows.UI.Core.CoreWindow"), None) {
                let inner = exe_of(child);
                if !inner.is_empty() {
                    exe = inner;
                }
            }
        }
        (exe, title)
    }
}

pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// If one of Wisper's own windows (e.g. the bar after a click) has focus, hand focus back to
/// the window the user was dictating into, so the paste lands there.
pub fn refocus(target: isize) {
    if target == 0 {
        return;
    }
    unsafe {
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let ours = pid == GetCurrentProcessId();
        let target = HWND(target as _);
        if (fg.0.is_null() || ours) && fg != target && IsWindow(Some(target)).as_bool() {
            let _ = SetForegroundWindow(target);
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
}

fn process_elevated(h: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        OpenProcessToken(h, TOKEN_QUERY, &mut token).ok()?;
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok.then_some(elev.TokenIsElevated != 0)
    }
}

pub fn we_are_elevated() -> bool {
    unsafe { process_elevated(windows::Win32::System::Threading::GetCurrentProcess()).unwrap_or(false) }
}

/// Whether the focused app runs as administrator. If we can't tell, assume not and try the paste.
pub fn foreground_is_elevated() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let r = process_elevated(h);
        let _ = CloseHandle(h);
        r.unwrap_or(false)
    }
}
