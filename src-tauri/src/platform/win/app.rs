//! The app in the foreground: executable name and window title.

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};
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
