//! Text insertion: put the text on the clipboard, press Ctrl+V in the focused app, then put
//! back whatever the user had copied (every format, not just text). Dictated text is kept
//! out of Windows clipboard history.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT,
};

use super::super::{FocusState, InsertOutcome};
use super::hook::INJECTED_MARK;

const CF_UNICODETEXT: u32 = 13;
static OWNER: AtomicIsize = AtomicIsize::new(0);

pub fn set_owner_hwnd(hwnd: isize) {
    OWNER.store(hwnd, Ordering::SeqCst);
}

fn owner() -> Option<HWND> {
    let h = OWNER.load(Ordering::SeqCst);
    (h != 0).then_some(HWND(h as _))
}

pub fn modifiers_down() -> Vec<u32> {
    let mut v = Vec::new();
    let down = |k: VIRTUAL_KEY| unsafe { (GetAsyncKeyState(k.0 as i32) as u16 & 0x8000) != 0 };
    if down(VK_LCONTROL) || down(VK_RCONTROL) {
        v.push(0x11);
    }
    if down(VK_LSHIFT) || down(VK_RSHIFT) {
        v.push(0x10);
    }
    if down(VK_LMENU) || down(VK_RMENU) {
        v.push(0x12);
    }
    if down(VK_LWIN) || down(VK_RWIN) {
        v.push(0x5B);
    }
    v
}

/// Waits until Ctrl/Shift/Alt/Win are all physically released, so our Ctrl+V isn't combined
/// with keys the user is still holding (e.g. after finishing hands-free with Ctrl+Win).
pub fn wait_for_modifiers_released(max_ms: u64) {
    let start = Instant::now();
    while !modifiers_down().is_empty() && start.elapsed() < Duration::from_millis(max_ms) {
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: INJECTED_MARK,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    n as usize == inputs.len()
}

/// Releases any modifiers the system still thinks are down (from the user's hotkey), then
/// presses the given modifier + key.
fn chord(modifier: VIRTUAL_KEY, k: VIRTUAL_KEY) -> bool {
    let mut v = Vec::new();
    for m in modifiers_down() {
        let vk = match m {
            0x11 => VK_CONTROL,
            0x10 => VK_SHIFT,
            0x12 => VK_MENU,
            _ => continue, // never synthesise a Win release: it can open the Start menu
        };
        v.push(key(vk, true));
    }
    v.push(key(modifier, false));
    v.push(key(k, false));
    v.push(key(k, true));
    v.push(key(modifier, true));
    send(&v)
}

pub fn send_paste(exe: &str) -> bool {
    // Some terminals don't take Ctrl+V; Shift+Insert works everywhere there.
    let shift_insert = matches!(exe, "mintty.exe" | "putty.exe" | "kitty.exe" | "conemu64.exe");
    if shift_insert {
        chord(VK_SHIFT, VK_INSERT)
    } else {
        chord(VK_CONTROL, VIRTUAL_KEY(0x56))
    }
}

pub fn send_copy() -> bool {
    chord(VK_CONTROL, VIRTUAL_KEY(0x43))
}

pub fn send_undo() {
    chord(VK_CONTROL, VIRTUAL_KEY(0x5A));
}

struct ClipGuard;

impl ClipGuard {
    fn open() -> Option<ClipGuard> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(owner()) }.is_ok() {
                return Some(ClipGuard);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        None
    }
}

impl Drop for ClipGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// A saved copy of every clipboard format that is plain memory.
pub struct ClipboardSnapshot {
    items: Vec<(u32, Vec<u8>)>,
}

fn is_memory_format(f: u32) -> bool {
    // Handle-based formats (bitmaps, metafiles, palettes, owner display) can't be copied as
    // bytes; Windows re-creates CF_BITMAP from CF_DIB on its own.
    !matches!(f, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E)
        // CF_TEXT, CF_OEMTEXT and CF_LOCALE are synthesised from CF_UNICODETEXT.
        && !matches!(f, 1 | 7 | 16)
}

pub fn snapshot_clipboard() -> Option<ClipboardSnapshot> {
    let _g = ClipGuard::open()?;
    let mut items = Vec::new();
    let mut f = 0u32;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 {
            break;
        }
        if !is_memory_format(f) {
            continue;
        }
        let Ok(h) = (unsafe { GetClipboardData(f) }) else { continue };
        let hg = HGLOBAL(h.0);
        unsafe {
            let size = GlobalSize(hg);
            if size == 0 || size > 256 * 1024 * 1024 {
                continue;
            }
            let p = GlobalLock(hg) as *const u8;
            if p.is_null() {
                continue;
            }
            let bytes = std::slice::from_raw_parts(p, size).to_vec();
            let _ = GlobalUnlock(hg);
            items.push((f, bytes));
        }
    }
    Some(ClipboardSnapshot { items })
}

fn alloc_global(bytes: &[u8]) -> Option<HANDLE> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(HANDLE(h.0))
    }
}

fn restore_snapshot(snap: &ClipboardSnapshot) -> bool {
    let Some(_g) = ClipGuard::open() else { return false };
    unsafe {
        let _ = EmptyClipboard();
        for (f, bytes) in &snap.items {
            if let Some(h) = alloc_global(bytes) {
                let _ = SetClipboardData(*f, Some(h));
            }
        }
    }
    true
}

fn utf16z(text: &str) -> Vec<u8> {
    let mut v: Vec<u16> = text.encode_utf16().collect();
    v.push(0);
    v.iter().flat_map(|c| c.to_le_bytes()).collect()
}

fn register(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

/// Puts text on the clipboard. `private` keeps it out of clipboard history/cloud sync.
pub fn put_text(text: &str, private: bool) -> bool {
    let Some(_g) = ClipGuard::open() else { return false };
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let Some(h) = alloc_global(&utf16z(text)) else { return false };
        if SetClipboardData(CF_UNICODETEXT, Some(h)).is_err() {
            return false;
        }
        if private {
            let zero = 0u32.to_le_bytes();
            for name in [w!("CanIncludeInClipboardHistory"), w!("CanUploadToCloudClipboard")] {
                if let Some(h) = alloc_global(&zero) {
                    let _ = SetClipboardData(register(name), Some(h));
                }
            }
            if let Some(h) = alloc_global(&zero) {
                let _ = SetClipboardData(register(w!("ExcludeClipboardContentFromMonitorProcessing")), Some(h));
            }
        }
    }
    true
}

pub fn read_text() -> Option<String> {
    let _g = ClipGuard::open()?;
    unsafe {
        let h = GetClipboardData(CF_UNICODETEXT).ok()?;
        let hg = HGLOBAL(h.0);
        let p = GlobalLock(hg) as *const u16;
        if p.is_null() {
            return None;
        }
        let max = GlobalSize(hg) / 2;
        let mut len = 0;
        while len < max && *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        let _ = GlobalUnlock(hg);
        Some(s)
    }
}

pub fn set_clipboard_text(text: &str) -> Result<(), String> {
    if put_text(text, false) {
        Ok(())
    } else {
        Err("Couldn't open the clipboard (another app is using it). Try again.".into())
    }
}

/// How long to wait before restoring the clipboard: slow apps read it late.
fn restore_delay(exe: &str) -> Duration {
    let slow = ["winword.exe", "outlook.exe", "olk.exe", "excel.exe", "powerpnt.exe", "onenote.exe", "teams.exe", "ms-teams.exe"];
    if exe.contains("whatsapp") || slow.contains(&exe) {
        Duration::from_millis(1500)
    } else if matches!(exe, "mstsc.exe" | "msrdc.exe" | "vmconnect.exe" | "wfica32.exe" | "cdviewer.exe") {
        Duration::from_millis(4000)
    } else {
        Duration::from_millis(600)
    }
}

pub fn insert_text(text: &str, exe: &str) -> InsertOutcome {
    if text.is_empty() {
        return InsertOutcome::Pasted;
    }
    wait_for_modifiers_released(2500);
    if super::uia::focus_state(400) == FocusState::NotEditable {
        return InsertOutcome::NoTextField;
    }
    let snapshot = snapshot_clipboard();
    if !put_text(text, true) {
        return InsertOutcome::Failed("Couldn't use the clipboard (another app is holding it).".into());
    }
    let seq_after_set = unsafe { GetClipboardSequenceNumber() };
    // Give clipboard listeners a moment, then paste.
    std::thread::sleep(Duration::from_millis(30));
    if !send_paste(exe) {
        return InsertOutcome::Failed(
            "Windows blocked the paste. If the app is running as administrator, Wisper can't type into it.".into(),
        );
    }
    let delay = restore_delay(exe);
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        // Only restore if nothing new was copied in the meantime.
        if unsafe { GetClipboardSequenceNumber() } == seq_after_set {
            if let Some(s) = snapshot {
                restore_snapshot(&s);
            }
        }
    });
    InsertOutcome::Pasted
}

/// Gets the selected text by pressing Ctrl+C (fallback when UI Automation can't read it),
/// restoring the user's clipboard afterwards.
pub fn copy_selection_via_clipboard() -> Option<String> {
    wait_for_modifiers_released(2500);
    let snapshot = snapshot_clipboard();
    let before = unsafe { GetClipboardSequenceNumber() };
    if !send_copy() {
        return None;
    }
    let start = Instant::now();
    let mut text = None;
    while start.elapsed() < Duration::from_millis(500) {
        std::thread::sleep(Duration::from_millis(20));
        if unsafe { GetClipboardSequenceNumber() } != before {
            std::thread::sleep(Duration::from_millis(30));
            text = read_text();
            break;
        }
    }
    if let Some(s) = snapshot {
        restore_snapshot(&s);
    }
    text.filter(|t| !t.trim().is_empty())
}
