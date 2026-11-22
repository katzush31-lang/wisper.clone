//! Everything that touches Windows directly lives here, behind small functions with
//! no-op stand-ins on other systems so the rest of the app builds and tests anywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use parking_lot::Mutex;
use wisper_core::hotkey::{Action, GestureEngine};

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
}

/// State shared between the keyboard hook thread and the rest of the app.
pub struct HookShared {
    pub engine: Mutex<GestureEngine>,
    pub tx: Mutex<Sender<HookEvent>>,
    pub capturing: AtomicBool,
    pub capture_keys: Mutex<Vec<u32>>,
    pub capture_down: Mutex<Vec<u32>>,
    /// Install the low-level mouse hook (only when a shortcut uses a mouse button).
    pub mouse_enabled: AtomicBool,
    /// A Win-key combo was used; mask the Win key-up so the Start menu doesn't open.
    pub mask_win: AtomicBool,
    /// Shortcuts switched off from the tray menu.
    pub paused: AtomicBool,
}

impl HookShared {
    pub fn new(engine: GestureEngine, tx: Sender<HookEvent>) -> Arc<Self> {
        Arc::new(Self {
            engine: Mutex::new(engine),
            tx: Mutex::new(tx),
            capturing: AtomicBool::new(false),
            capture_keys: Mutex::new(Vec::new()),
            capture_down: Mutex::new(Vec::new()),
            mouse_enabled: AtomicBool::new(false),
            mask_win: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        })
    }

    pub fn send(&self, e: HookEvent) {
        let _ = self.tx.lock().send(e);
    }

    pub fn start_capture(&self) {
        self.capture_keys.lock().clear();
        self.capture_down.lock().clear();
        self.capturing.store(true, Ordering::SeqCst);
    }

    pub fn stop_capture(&self) {
        self.capturing.store(false, Ordering::SeqCst);
    }

    /// Handles a key event while capturing. Returns true (swallow) when capturing.
    pub fn capture_event(&self, vk: u32, down: bool) -> bool {
        if !self.capturing.load(Ordering::SeqCst) {
            return false;
        }
        let vk = wisper_core::hotkey::vk::normalize(vk);
        let mut keys = self.capture_keys.lock();
        let mut held = self.capture_down.lock();
        if down {
            if !held.contains(&vk) {
                held.push(vk);
            }
            if !keys.contains(&vk) && keys.len() < 4 {
                keys.push(vk);
            }
            self.send(HookEvent::Captured { keys: keys.clone(), done: false });
        } else {
            held.retain(|&k| k != vk);
            if held.is_empty() && !keys.is_empty() {
                self.send(HookEvent::Captured { keys: keys.clone(), done: true });
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
