//! Non-Windows stand-ins (development builds and CI on Linux).
#![allow(dead_code)]
use std::sync::Arc;

use super::{FieldContext, HookShared, InsertOutcome};
use wisper_core::settings::OtherAudio;
use wisper_core::sounds::Sound;

pub fn set_owner_hwnd(_hwnd: isize) {}
pub fn start_hook(_shared: Arc<HookShared>) {}
pub fn refresh_hooks() {}
pub fn modifiers_down() -> Vec<u32> {
    Vec::new()
}
pub fn foreground_app() -> (String, String) {
    (String::new(), String::new())
}
pub fn read_context(_timeout_ms: u64) -> FieldContext {
    FieldContext::default()
}
pub fn insert_text(_text: &str, _exe: &str) -> InsertOutcome {
    InsertOutcome::Failed("Inserting text only works on Windows.".into())
}
pub fn copy_selection_via_clipboard() -> Option<String> {
    None
}
pub fn set_clipboard_text(_text: &str) -> Result<(), String> {
    Err("Clipboard only works on Windows.".into())
}
pub fn send_undo() {}
pub fn wait_for_modifiers_released(_max_ms: u64) {}
pub fn play_sound(_s: Sound, _volume: f32) {}

pub struct DuckToken;
pub fn duck_other_audio(_mode: OtherAudio, _state_file: &std::path::Path) -> DuckToken {
    DuckToken
}
pub fn restore_other_audio(_t: DuckToken, _state_file: &std::path::Path) {}
pub fn restore_after_crash(_state_file: &std::path::Path) {}
