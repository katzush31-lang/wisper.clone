//! Low-level keyboard hook (and a mouse hook while a shortcut uses a mouse button) on its own
//! thread with a message loop.
//!
//! Windows silently removes a low-level hook whose callback takes too long
//! (LowLevelHooksTimeout, at most 1 s), and from then on no shortcut works. So the callbacks
//! only touch memory: they run the gesture engine (never waiting more than `HOOK_LOCK_WAIT`
//! for it) and queue the resulting actions. Anything that calls into other processes happens
//! on other threads; that includes the Start-menu mask key, because `SendInput` runs every
//! other app's keyboard hooks before it returns.
//!
//! A watchdog listens to raw keyboard input (which Windows never takes away). When a key
//! arrives there that the hook didn't see, the hook has been removed: it is re-installed on the
//! spot, so at most one key press is missed. When the keys are going to a window that runs as
//! administrator, Windows hides them from us on purpose (UIPI); that is reported instead.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, OnceLock};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DispatchMessageW, GetMessageW, KillTimer, PostThreadMessageW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::super::{HookEvent, HookShared, HOOK_LOCK_WAIT};
use wisper_core::hotkey::{vk, Action};

/// Marks input we inject ourselves so the hook ignores it.
pub const INJECTED_MARK: usize = 0x5749_5350; // "WISP"

static SHARED: OnceLock<Arc<HookShared>> = OnceLock::new();
static THREAD_ID: OnceLock<u32> = OnceLock::new();
/// Wakes the thread that sends the Start-menu mask key.
static MASK: OnceLock<Sender<()>> = OnceLock::new();
/// When the keyboard hook was last called (ms since boot), for the watchdog.
static LAST_KEYBOARD_CALL: AtomicU64 = AtomicU64::new(0);

const MSG_REINSTALL: u32 = WM_APP + 1;
/// Re-install the hooks now and then anyway (the watchdog only watches the keyboard hook).
const REINSTALL_EVERY_MS: u32 = 2 * 60 * 1000;
/// A raw key event with no keyboard-hook call this recently means the hook is gone.
const HOOK_SILENT_MS: u64 = 500;
/// The watchdog looks a moment after a raw key event, so it doesn't matter whether Windows
/// calls the hook just before or just after delivering raw input.
const WATCHDOG_TIMER: usize = 7;
const WATCHDOG_DELAY_MS: u32 = 40;

pub fn start_hook(shared: Arc<HookShared>) {
    if SHARED.set(shared).is_err() {
        return;
    }
    let (tx, rx) = channel::<()>();
    let _ = MASK.set(tx);
    std::thread::Builder::new()
        .name("wisper-mask-key".into())
        .spawn(move || {
            for () in rx {
                send_mask_key();
            }
        })
        .expect("spawn mask-key thread");
    std::thread::Builder::new()
        .name("wisper-hook".into())
        .spawn(|| unsafe { hook_thread() })
        .expect("spawn hook thread");
}

/// Asks the hook thread to re-install its hooks (e.g. after a mouse shortcut was added, or
/// while a shortcut is being recorded).
pub fn refresh_hooks() {
    if let Some(&tid) = THREAD_ID.get() {
        unsafe {
            let _ = PostThreadMessageW(tid, MSG_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }
}

/// Whether the OS says this key or mouse button is physically held (generic modifier codes
/// cover both left and right keys).
pub fn os_key_down(code: u32) -> bool {
    let down = |k: u32| unsafe { (GetAsyncKeyState(k as i32) as u16 & 0x8000) != 0 };
    match code {
        vk::WIN => down(vk::WIN) || down(vk::RWIN),
        c => down(c),
    }
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

#[derive(Default)]
struct Hooks {
    keyboard: Option<HHOOK>,
    mouse: Option<HHOOK>,
}

/// Installs fresh hooks, then removes the old ones (in that order, so no event is missed in
/// between: callbacks only run when this thread reads its messages).
unsafe fn install(old: Hooks, why: &str) -> Hooks {
    let keyboard = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) } {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::error!("couldn't install keyboard hook: {e}");
            None
        }
    };
    let want_mouse = SHARED.get().map(|s| s.mouse_wanted()).unwrap_or(false);
    let mouse = if want_mouse {
        match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("couldn't install mouse hook: {e}");
                None
            }
        }
    } else {
        None
    };
    let had_mouse = old.mouse.is_some();
    unsafe { uninstall(old) };
    LAST_KEYBOARD_CALL.store(now_ms(), Ordering::Relaxed);
    if why.is_empty() {
        tracing::debug!("hooks refreshed");
    } else {
        tracing::info!("hooks installed ({why}): keyboard {}, mouse {}", keyboard.is_some(), mouse.is_some());
    }
    if had_mouse && mouse.is_none() {
        tracing::info!("mouse hook removed (no shortcut uses a mouse button)");
    }
    Hooks { keyboard, mouse }
}

unsafe fn uninstall(h: Hooks) {
    unsafe {
        if let Some(k) = h.keyboard {
            let _ = UnhookWindowsHookEx(k);
        }
        if let Some(m) = h.mouse {
            let _ = UnhookWindowsHookEx(m);
        }
    }
}

/// Raw keyboard input for the watchdog, delivered to a hidden message-only window even when
/// Wisper isn't in front.
unsafe fn watch_raw_keyboard() -> Option<HWND> {
    let hwnd: HWND = match unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Wisper hook watchdog"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)
    } {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("hook watchdog unavailable (window): {e}");
            return None;
        }
    };
    let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    match unsafe { RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32) } {
        Ok(()) => Some(hwnd),
        Err(e) => {
            tracing::warn!("hook watchdog unavailable (raw input): {e}");
            None
        }
    }
}

unsafe fn hook_thread() {
    let _ = THREAD_ID.set(unsafe { GetCurrentThreadId() });
    // The hook must answer quickly even when the computer is busy.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let mut hooks = unsafe { install(Hooks::default(), "start") };
    let watch = unsafe { watch_raw_keyboard() };
    let timer = unsafe { SetTimer(None, 0, REINSTALL_EVERY_MS, None) };
    let mut last_repair = 0u64;
    let mut last_admin_note = 0u64;
    // Time of a raw key event whose check is pending.
    let mut raw_at: Option<u64> = None;
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        match msg.message {
            MSG_REINSTALL => {
                hooks = unsafe { install(std::mem::take(&mut hooks), "shortcut settings changed") };
                continue;
            }
            WM_TIMER if msg.hwnd.0.is_null() => {
                hooks = unsafe { install(std::mem::take(&mut hooks), "") };
                continue;
            }
            WM_INPUT if watch.is_some() => {
                if raw_at.is_none() {
                    raw_at = Some(now_ms());
                    unsafe { SetTimer(watch, WATCHDOG_TIMER, WATCHDOG_DELAY_MS, None) };
                }
                // Fall through: DefWindowProc releases the raw input data.
            }
            WM_TIMER if watch.is_some() && msg.hwnd == watch.unwrap_or_default() && msg.wParam.0 == WATCHDOG_TIMER => {
                unsafe {
                    let _ = KillTimer(watch, WATCHDOG_TIMER);
                }
                let Some(at) = raw_at.take() else { continue };
                // Every key reaches the keyboard hook too, so it must have been called around
                // then. If not, Windows took it away (or an admin window has the keys).
                if LAST_KEYBOARD_CALL.load(Ordering::Relaxed) + HOOK_SILENT_MS < at {
                    let now = now_ms();
                    if super::app::admin_window_in_front() {
                        // Keys typed into an admin app are none of our business, unless they
                        // are our shortcut: then explain why it does nothing.
                        let pressed_shortcut = SHARED
                            .get()
                            .and_then(|s| s.engine.try_lock_for(HOOK_LOCK_WAIT).map(|e| e.start_combo_held(&os_key_down)))
                            .unwrap_or(false);
                        if pressed_shortcut && now.saturating_sub(last_admin_note) > 10_000 {
                            last_admin_note = now;
                            tracing::info!("shortcut pressed in an app running as administrator; Windows hides keys there from Wisper");
                            if let Some(s) = SHARED.get() {
                                s.send(HookEvent::BlockedByAdminWindow);
                            }
                        }
                    } else if now.saturating_sub(last_repair) > 1000 {
                        last_repair = now;
                        tracing::warn!("keyboard hook stopped receiving keys (Windows removed it?); re-installing");
                        hooks = unsafe { install(std::mem::take(&mut hooks), "watchdog") };
                    }
                }
                continue;
            }
            _ => {}
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    tracing::warn!("hook thread message loop ended; removing hooks");
    unsafe {
        let _ = KillTimer(None, timer);
        uninstall(hooks);
    }
}

/// Handles one key/button event; returns true if it should be swallowed. Only memory work and
/// GetAsyncKeyState (a quick read of the input state) happen here.
fn handle(s: &HookShared, code: u32, down: bool) -> bool {
    if s.paused.load(Ordering::Relaxed) {
        return false;
    }
    let now = now_ms();
    let Some(mut engine) = s.engine.try_lock_for(HOOK_LOCK_WAIT) else {
        // Never expected (lock holders only do quick memory work); let the event through
        // rather than risk the hook. The app logs the count, and `sync_keys` repairs the state.
        s.lock_misses.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    let mut actions = Vec::new();
    let outcome = if down {
        // Missed key-ups/downs (focus moved to an admin window, the lock screen, a hook gap)
        // are repaired from the OS's own key state before the press is matched.
        actions = engine.sync_keys(&os_key_down, now);
        engine.key_down(code, now)
    } else {
        engine.key_up(code, now)
    };
    drop(engine);
    actions.extend(outcome.actions);
    for a in actions {
        if a == Action::SuppressStartMenu {
            // Tapping an unassigned key while Win/Alt are down stops Windows treating their
            // release as "open the Start menu", "switch keyboard layout" (Alt+Shift) or
            // "focus the menu bar" (Alt). Sent from another thread: see the module comment.
            if let Some(m) = MASK.get() {
                let _ = m.send(());
            }
        } else {
            s.send(HookEvent::Action(a));
        }
    }
    if down && !outcome.swallow && !vk::is_modifier(code) {
        s.typed.fetch_add(1, Ordering::Relaxed);
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
        LAST_KEYBOARD_CALL.store(now_ms(), Ordering::Relaxed);
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let ours = kb.dwExtraInfo == INJECTED_MARK;
        let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
        if !ours {
            let msg = wparam.0 as u32;
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            // Injected keys from other tools (remappers, on-screen keyboards) still count,
            // except the Start-menu mask key some tools send.
            if (down || up) && !(injected && kb.vkCode == 0xE8) {
                if let Some(s) = SHARED.get() {
                    if s.capture_event(kb.vkCode, down) || handle(s, kb.vkCode, down) {
                        return LRESULT(1);
                    }
                }
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
                WM_LBUTTONDOWN | WM_LBUTTONUP => Some(vk::MOUSE_LEFT),
                WM_RBUTTONDOWN | WM_RBUTTONUP => Some(vk::MOUSE_RIGHT),
                WM_MBUTTONDOWN | WM_MBUTTONUP => Some(vk::MOUSE_MIDDLE),
                WM_XBUTTONDOWN | WM_XBUTTONUP => Some(if (ms.mouseData >> 16) & 0xFFFF == 1 { vk::MOUSE_X1 } else { vk::MOUSE_X2 }),
                _ => None,
            };
            if let (Some(b), Some(s)) = (button, SHARED.get()) {
                let down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN);
                // Only buttons that shortcuts use reach the gesture engine.
                if s.capture_event(b, down) || (s.uses_button(b) && handle(s, b, down)) {
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
