//! Wisper core: everything that doesn't need Windows, so it can be unit-tested anywhere.

pub mod apps;
pub mod audio;
pub mod cloud;
pub mod db;
pub mod common_words;
pub mod dictionary;
pub mod hebrew;
pub mod hotkey;
pub mod live;
pub mod modes;
pub mod pipeline;
pub mod prompt_preview;
pub mod prompts;
pub mod quick;
pub mod providers;
pub mod settings;
pub mod snippets;
pub mod sounds;
pub mod text;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
