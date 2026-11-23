mod app;
mod audio;
mod hook;
mod input;
mod uia;

pub use app::{admin_window_in_front, foreground_app, foreground_window, refocus};
pub use audio::{duck_other_audio, play_sound, restore_after_crash, restore_other_audio, DuckToken};
pub use hook::{os_key_down, refresh_hooks, start_hook};
pub use input::{copy_selection_via_clipboard, insert_text, send_undo, set_clipboard_text, set_owner_hwnd, wait_for_modifiers_released};

pub fn read_context(timeout_ms: u64) -> super::FieldContext {
    uia::read_context(timeout_ms)
}
