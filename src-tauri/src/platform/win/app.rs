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

/// Whether a process runs elevated. `Err(())` when its token can't be read: a normal process
/// isn't allowed to read the token of one running as administrator.
fn process_elevated(h: windows::Win32::Foundation::HANDLE) -> Result<bool, ()> {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        OpenProcessToken(h, TOKEN_QUERY, &mut token).map_err(|_| ())?;
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
        if ok {
            Ok(elev.TokenIsElevated != 0)
        } else {
            Err(())
        }
    }
}

pub fn we_are_elevated() -> bool {
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe { process_elevated(windows::Win32::System::Threading::GetCurrentProcess()).unwrap_or(false) })
}

/// Whether the focused app runs as administrator (or at least above Wisper: we're not even
/// allowed to look at its token). If there's nothing in front or it's Wisper, no.
pub fn foreground_is_elevated() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return false;
        }
        let h = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(e) => return e.code() == windows::Win32::Foundation::E_ACCESSDENIED,
        };
        let r = process_elevated(h);
        let _ = CloseHandle(h);
        // Token unreadable: it runs above us (as administrator); reading our own user's apps works.
        r.unwrap_or(true)
    }
}

/// The window in front runs as administrator and Wisper doesn't, so Windows hides keyboard
/// input from Wisper's hooks and blocks pasting into it (User Interface Privilege Isolation).
pub fn admin_window_in_front() -> bool {
    !we_are_elevated() && foreground_is_elevated()
}
