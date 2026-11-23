//! Everything that touches Windows directly lives here, behind small functions with
//! no-op stand-ins on other systems so the rest of the app builds and tests anywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wisper_core::hotkey::{vk, Action, GestureEngine};

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::*;

#[derive(Debug, Clone)]
pub enum HookEvent {
    Action(Action),
    /// Settings → Shortcuts is capturing a new key combination.
    Captured { keys: Vec<u32>, done: bool },
    /// Keys are going to a window running as administrator, which Windows hides from Wisper.
    BlockedByAdminWindow,
}

/// Keys pressed so far while Settings → Shortcuts records a new shortcut.
#[derive(Default)]
pub struct CaptureState {
    keys: Vec<u32>,
    held: Vec<u32>,
}

/// How long the hook may wait for a lock before giving up on an event. Every holder only does
/// a few memory operations, so this is never reached in practice; it just guarantees the
/// hook can't be held up long enough for Windows to remove it.
pub const HOOK_LOCK_WAIT: Duration = Duration::from_millis(30);

/// State shared between the keyboard hook thread and the rest of the app.
///
/// Rule: whoever locks `engine` or `capture` only calls the engine's quick methods while
/// holding it (never the UI, another lock, or anything that can wait), and the hook itself
/// never waits longer than `HOOK_LOCK_WAIT`.
pub struct HookShared {
    pub engine: Mutex<GestureEngine>,
    tx: Sender<HookEvent>,
    pub capturing: AtomicBool,
    capture: Mutex<CaptureState>,
    /// Mouse buttons (bit = button code) that shortcuts use; the mouse hook is installed when
    /// this is non-zero or while a shortcut is being recorded.
    pub mouse_buttons: AtomicU32,
    /// Shortcuts switched off from the tray menu.
    pub paused: AtomicBool,
    /// Counts ordinary key presses, so "undo" can tell whether the user typed since.
    pub typed: AtomicU64,
    /// Key events the hook let through unprocessed because the engine was busy (should stay 0).
    pub lock_misses: AtomicU64,
}

impl HookShared {
    pub fn new(engine: GestureEngine, tx: Sender<HookEvent>) -> Arc<Self> {
        Arc::new(Self {
            engine: Mutex::new(engine),
            tx,
            capturing: AtomicBool::new(false),
            capture: Mutex::new(CaptureState::default()),
            mouse_buttons: AtomicU32::new(0),
            paused: AtomicBool::new(false),
            typed: AtomicU64::new(0),
            lock_misses: AtomicU64::new(0),
        })
    }

    /// Never blocks (unbounded channel).
    pub fn send(&self, e: HookEvent) {
        let _ = self.tx.send(e);
    }

    pub fn start_capture(&self) {
        *self.capture.lock() = CaptureState::default();
        self.capturing.store(true, Ordering::SeqCst);
    }

    pub fn stop_capture(&self) {
        self.capturing.store(false, Ordering::SeqCst);
    }

    /// Whether the mouse hook is needed right now.
    pub fn mouse_wanted(&self) -> bool {
        self.mouse_buttons.load(Ordering::SeqCst) != 0 || self.capturing.load(Ordering::SeqCst)
    }

    /// Whether a shortcut uses this mouse button (only those reach the gesture engine).
    pub fn uses_button(&self, button: u32) -> bool {
        self.mouse_buttons.load(Ordering::Relaxed) & (1 << button) != 0
    }

    /// Handles a key or mouse-button event while capturing. Returns true (swallow) when capturing.
    pub fn capture_event(&self, code: u32, down: bool) -> bool {
        if !self.capturing.load(Ordering::SeqCst) {
            return false;
        }
        let code = vk::normalize(code);
        let Some(mut c) = self.capture.try_lock_for(HOOK_LOCK_WAIT) else { return false };
        if down {
            // Left/right clicks only count with a modifier held, so the dialog's own buttons
            // can still be clicked.
            let bare_click = matches!(code, vk::MOUSE_LEFT | vk::MOUSE_RIGHT) && !c.held.iter().any(|&k| vk::is_modifier(k));
            if bare_click {
                return false;
            }
            if !c.held.contains(&code) {
                c.held.push(code);
            }
            if !c.keys.contains(&code) && c.keys.len() < 4 {
                c.keys.push(code);
            }
            self.send(HookEvent::Captured { keys: c.keys.clone(), done: false });
        } else {
            // A key that was already down before capture started: let its release through,
            // otherwise the app would think it is still held.
            if !c.held.contains(&code) {
                return false;
            }
            c.held.retain(|&k| k != code);
            if c.held.is_empty() && !c.keys.is_empty() {
                self.send(HookEvent::Captured { keys: c.keys.clone(), done: true });
                self.capturing.store(false, Ordering::SeqCst);
            }
        }
        true
    }
}

/// Whether the focused control can take text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusState {
    Editable,
    /// Definitely not a text field (desktop, taskbar, a button…).
    NotEditable,
    /// Couldn't tell; try pasting anyway.
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct FieldContext {
    pub before: Option<String>,
    pub after: Option<String>,
    pub selection: Option<String>,
    pub is_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    Pasted,
    NoTextField,
    Failed(String),
}
