//! Low-level keyboard (and optional mouse) hook on its own thread with a message loop.
//! The callback must return quickly (Windows drops slow hooks), so it only runs the
//! gesture engine and posts actions to a channel.

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KillTimer, PostThreadMessageW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
    WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::super::{HookEvent, HookShared};
use wisper_core::hotkey::{vk, Action};

/// Marks input we inject ourselves so the hook ignores it.
pub const INJECTED_MARK: usize = 0x5749_5350; // "WISP"

static SHARED: OnceLock<Arc<HookShared>> = OnceLock::new();
static THREAD_ID: OnceLock<u32> = OnceLock::new();

const MSG_REINSTALL: u32 = WM_APP + 1;
/// Re-install the hooks periodically: Windows silently removes hooks it thinks are slow.
const REINSTALL_EVERY_MS: u32 = 10 * 60 * 1000;

pub fn start_hook(shared: Arc<HookShared>) {
    if SHARED.set(shared).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("wisper-hook".into())
        .spawn(|| unsafe { hook_thread() })
        .expect("spawn hook thread");
}

/// Asks the hook thread to re-install its hooks (e.g. after a mouse shortcut was added).
pub fn refresh_hooks() {
    if let Some(&tid) = THREAD_ID.get() {
        unsafe {
            let _ = PostThreadMessageW(tid, MSG_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe fn install() -> (Option<HHOOK>, Option<HHOOK>) {
    let kb = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }.ok();
    if kb.is_none() {
        tracing::error!("couldn't install keyboard hook");
    }
    let mouse_wanted = SHARED.get().map(|s| s.mouse_enabled.load(Ordering::SeqCst)).unwrap_or(false);
    let ms = if mouse_wanted { unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) }.ok() } else { None };
    (kb, ms)
}

unsafe fn uninstall(h: (Option<HHOOK>, Option<HHOOK>)) {
    unsafe {
        if let Some(k) = h.0 {
            let _ = UnhookWindowsHookEx(k);
        }
        if let Some(m) = h.1 {
            let _ = UnhookWindowsHookEx(m);
        }
    }
}

unsafe fn hook_thread() {
    let _ = THREAD_ID.set(unsafe { GetCurrentThreadId() });
    let mut hooks = unsafe { install() };
    let timer = unsafe { SetTimer(None, 0, REINSTALL_EVERY_MS, None) };
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        if msg.message == MSG_REINSTALL || msg.message == WM_TIMER {
            unsafe { uninstall(hooks) };
            hooks = unsafe { install() };
            // Keys may have been released while unhooked.
            if let Some(s) = SHARED.get() {
                s.engine.lock().sync_modifiers(&super::input::modifiers_down());
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unsafe {
        let _ = KillTimer(None, timer);
        uninstall(hooks);
    }
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// Handles one key/button event; returns true if it should be swallowed.
fn handle(code: u32, down: bool) -> bool {
    let Some(s) = SHARED.get() else { return false };
    if s.capture_event(code, down) {
        return true;
    }
    if s.paused.load(Ordering::SeqCst) {
        return false;
    }
    let outcome = {
        let mut engine = s.engine.lock();
        if down {
            engine.key_down(code, now_ms())
        } else {
            engine.key_up(code, now_ms())
        }
    };
    for a in outcome.actions {
        if a == Action::SuppressStartMenu {
            s.mask_win.store(true, Ordering::SeqCst);
        } else {
            s.send(HookEvent::Action(a));
        }
    }
    if !down && vk::normalize(code) == vk::WIN && s.mask_win.swap(false, Ordering::SeqCst) {
        // Tapping an unassigned key while Win is down stops Windows treating the
        // Win release as "open the Start menu".
        send_mask_key();
    }
    outcome.swallow
}

fn send_mask_key() {
    let mk = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_MARK },
        },
    };
    let inputs = [mk(KEYBD_EVENT_FLAGS(0)), mk(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let ours = kb.dwExtraInfo == INJECTED_MARK;
        let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
        if !ours {
            let msg = wparam.0 as u32;
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            // Injected keys from other tools (remappers, on-screen keyboards) still count,
            // except the Start-menu mask key some tools send.
            if (down || up) && !(injected && kb.vkCode == 0xE8) && handle(kb.vkCode, down) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if ms.dwExtraInfo != INJECTED_MARK {
            let msg = wparam.0 as u32;
            let button = match msg {
                WM_MBUTTONDOWN | WM_MBUTTONUP => Some(vk::MOUSE_MIDDLE),
                WM_XBUTTONDOWN | WM_XBUTTONUP => Some(if (ms.mouseData >> 16) & 0xFFFF == 1 { vk::MOUSE_X1 } else { vk::MOUSE_X2 }),
                _ => None,
            };
            if let Some(b) = button {
                let down = msg == WM_MBUTTONDOWN || msg == WM_XBUTTONDOWN;
                if handle(b, down) {
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
