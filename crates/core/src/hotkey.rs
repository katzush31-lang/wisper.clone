//! Hotkey gesture engine: turns raw key and mouse-button down/up events into dictation actions.
//!
//! Pure logic (no OS calls) so it can be unit-tested. The Windows hooks feed it normalised
//! virtual-key codes and ask it, synchronously, whether to swallow each event.
//!
//! Rules (deliberately simple, unlike Wispr's overlapping timers):
//! - Recording starts the instant a dictation combo is pressed, so first words are never lost.
//! - Hold the combo and release → finish and paste.
//! - Tap twice quickly → hands-free; tap once more to finish.
//! - A single short tap with no second tap → cancelled (nothing worth keeping was recorded).
//! - The hands-free combo starts/stops hands-free directly.
//! - Esc cancels while recording.
//!
//! Mouse buttons:
//! - With Ctrl/Alt/Shift/Win (e.g. Ctrl + Middle click), or a side button set to "single
//!   press", a button behaves exactly like a key: hold to talk, double-press for hands-free.
//!   Those clicks never reach apps.
//! - A button on its own set to "double-click" (always the case for the middle button):
//!   single clicks keep working in apps; a double-click starts hands-free (the second click is
//!   kept from the app); one click while recording finishes (that click is kept from the app).
//!   There is no hold-to-talk here: the press has already gone to the app before we could know
//!   it would be a hold, so holding would also drag/auto-scroll in the app, and every ordinary
//!   click would have to switch the microphone on and off. Use Ctrl + the button for that.
//!
//! Missed key releases (focus moved to an admin window, the lock screen, a gap while Windows
//! re-installed the hook) are repaired with `sync_keys`, fed the OS's own key state.

use serde::{Deserialize, Serialize};

/// Windows virtual-key codes we normalise to (left/right variants are merged).
pub mod vk {
    pub const BACKSPACE: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const ENTER: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const CTRL: u32 = 0x11;
    pub const ALT: u32 = 0x12;
    pub const CAPSLOCK: u32 = 0x14;
    pub const ESC: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const WIN: u32 = 0x5B;
    pub const LSHIFT: u32 = 0xA0;
    pub const RSHIFT: u32 = 0xA1;
    pub const LCTRL: u32 = 0xA2;
    pub const RCTRL: u32 = 0xA3;
    pub const LALT: u32 = 0xA4;
    pub const RALT: u32 = 0xA5;
    pub const RWIN: u32 = 0x5C;
    pub const F1: u32 = 0x70;
    pub const F24: u32 = 0x87;
    /// Mouse buttons use Windows' own codes for them (VK_LBUTTON … VK_XBUTTON2).
    pub const MOUSE_LEFT: u32 = 0x01;
    pub const MOUSE_RIGHT: u32 = 0x02;
    pub const MOUSE_MIDDLE: u32 = 0x04;
    /// The "back" side button.
    pub const MOUSE_X1: u32 = 0x05;
    /// The "forward" side button.
    pub const MOUSE_X2: u32 = 0x06;
    /// The modifiers, in their generic (merged) form.
    pub const MODIFIERS: [u32; 4] = [SHIFT, CTRL, ALT, WIN];

    /// Merges left/right modifier codes into the generic code.
    pub fn normalize(code: u32) -> u32 {
        match code {
            LSHIFT | RSHIFT => SHIFT,
            LCTRL | RCTRL => CTRL,
            LALT | RALT => ALT,
            RWIN => WIN,
            c => c,
        }
    }

    pub fn is_modifier(code: u32) -> bool {
        matches!(normalize(code), SHIFT | CTRL | ALT | WIN)
    }

    pub fn is_mouse(code: u32) -> bool {
        matches!(code, MOUSE_LEFT | MOUSE_RIGHT | MOUSE_MIDDLE | MOUSE_X1 | MOUSE_X2)
    }

    /// Human-readable key name for the settings UI.
    pub fn name(code: u32) -> String {
        match normalize(code) {
            SHIFT => "Shift".into(),
            CTRL => "Ctrl".into(),
            ALT => "Alt".into(),
            WIN => "Win".into(),
            SPACE => "Space".into(),
            ESC => "Esc".into(),
            ENTER => "Enter".into(),
            TAB => "Tab".into(),
            BACKSPACE => "Backspace".into(),
            CAPSLOCK => "Caps Lock".into(),
            MOUSE_LEFT => "Left click".into(),
            MOUSE_RIGHT => "Right click".into(),
            MOUSE_MIDDLE => "Middle click".into(),
            MOUSE_X1 => "Back button".into(),
            MOUSE_X2 => "Forward button".into(),
            c @ F1..=F24 => format!("F{}", c - F1 + 1),
            c @ 0x30..=0x39 | c @ 0x41..=0x5A => char::from_u32(c).unwrap().to_string(),
            0x60..=0x69 => format!("Num {}", code - 0x60),
            0xBA => ";".into(),
            0xBB => "=".into(),
            0xBC => ",".into(),
            0xBD => "-".into(),
            0xBE => ".".into(),
            0xBF => "/".into(),
            0xC0 => "`".into(),
            0xDB => "[".into(),
            0xDC => "\\".into(),
            0xDD => "]".into(),
            0xDE => "'".into(),
            0x21 => "Page Up".into(),
            0x22 => "Page Down".into(),
            0x23 => "End".into(),
            0x24 => "Home".into(),
            0x25 => "Left".into(),
            0x26 => "Up".into(),
            0x27 => "Right".into(),
            0x28 => "Down".into(),
            0x2D => "Insert".into(),
            0x2E => "Delete".into(),
            other => format!("Key {other:#04x}"),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A set of keys (and at most one mouse button) pressed together. Order is irrelevant;
/// stored sorted and normalised.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct KeyCombo {
    pub keys: Vec<u32>,
    /// Only for a mouse button on its own: double-click it to start, so single clicks keep
    /// working in apps. (The middle button on its own always works this way.)
    #[serde(default, rename = "doubleClick", skip_serializing_if = "is_false")]
    pub double_click: bool,
}

impl KeyCombo {
    pub fn new(keys: &[u32]) -> Self {
        let mut k: Vec<u32> = keys.iter().map(|&c| vk::normalize(c)).collect();
        k.sort_unstable();
        k.dedup();
        Self { keys: k, double_click: false }
    }

    /// A mouse button on its own, used by double-clicking it.
    pub fn double_click(button: u32) -> Self {
        Self { keys: vec![button], double_click: true }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn contains(&self, code: u32) -> bool {
        self.keys.contains(&vk::normalize(code))
    }

    pub fn has_win(&self) -> bool {
        self.contains(vk::WIN)
    }

    /// The mouse button, if this shortcut is a double-click of a button on its own. The middle
    /// button on its own always is: a plain middle click must keep working in apps.
    pub fn click_button(&self) -> Option<u32> {
        match self.keys.as_slice() {
            [b] if matches!(*b, vk::MOUSE_MIDDLE | vk::MOUSE_X1 | vk::MOUSE_X2) && (self.double_click || *b == vk::MOUSE_MIDDLE) => Some(*b),
            _ => None,
        }
    }

    /// Mouse buttons used by this shortcut.
    pub fn mouse_buttons(&self) -> impl Iterator<Item = u32> + '_ {
        self.keys.iter().copied().filter(|&k| vk::is_mouse(k))
    }

    /// A left or right button with no modifier: never used, it would break ordinary clicking.
    fn is_bare_left_right(&self) -> bool {
        self.keys.iter().any(|&k| matches!(k, vk::MOUSE_LEFT | vk::MOUSE_RIGHT)) && !self.keys.iter().any(|&k| vk::is_modifier(k))
    }

    /// Display names, modifiers first in a fixed order, e.g. ["Ctrl", "Win", "Space"] or
    /// ["Double-click", "Middle click"].
    pub fn names(&self) -> Vec<String> {
        let order = |c: &u32| match *c {
            vk::CTRL => 0,
            vk::WIN => 1,
            vk::ALT => 2,
            vk::SHIFT => 3,
            _ => 4,
        };
        let mut keys = self.keys.clone();
        keys.sort_by_key(|c| (order(c), *c));
        let mut names: Vec<String> = keys.iter().map(|&c| vk::name(c)).collect();
        if self.click_button().is_some() {
            names.insert(0, "Double-click".into());
        }
        names
    }

    /// e.g. "Ctrl + Win + Space" (modifiers first, in a fixed order).
    pub fn label(&self) -> String {
        self.names().join(" + ")
    }

    /// Validates a user-chosen combo. Returns a plain-English problem if it can't be used.
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Err("Press at least one key or mouse button.".into());
        }
        if self.keys.len() > 3 {
            return Err("Use 3 keys or fewer.".into());
        }
        let mouse: Vec<u32> = self.mouse_buttons().collect();
        if mouse.len() > 1 {
            return Err("Use only one mouse button in a shortcut.".into());
        }
        if self.is_bare_left_right() {
            return Err("The left and right mouse buttons need Ctrl, Alt, Shift or Win with them, so ordinary clicking keeps working.".into());
        }
        if self.double_click && !(self.keys.len() == 1 && mouse.len() == 1) {
            return Err("Double-click only works with a mouse button on its own.".into());
        }
        let has_modifier = self.keys.iter().any(|&k| vk::is_modifier(k));
        let is_fkey = self.keys.iter().any(|&k| (vk::F1..=vk::F24).contains(&k));
        let single_ok = self.keys.len() == 1 && (!mouse.is_empty() || is_fkey || self.keys[0] == vk::CAPSLOCK);
        if !has_modifier && !single_ok {
            return Err("Include Ctrl, Alt, Shift or Win (or use an F-key, Caps Lock or a mouse button).".into());
        }
        if self.keys == [vk::CTRL] || self.keys == [vk::SHIFT] || self.keys == [vk::ALT] {
            return Err("A single Ctrl, Shift or Alt key would trigger constantly while you type.".into());
        }
        let reserved: &[&[u32]] = &[
            &[vk::CTRL, 0x43],       // Ctrl+C
            &[vk::CTRL, 0x56],       // Ctrl+V
            &[vk::CTRL, 0x58],       // Ctrl+X
            &[vk::CTRL, 0x5A],       // Ctrl+Z
            &[vk::CTRL, 0x59],       // Ctrl+Y
            &[vk::CTRL, 0x41],       // Ctrl+A
            &[vk::CTRL, 0x53],       // Ctrl+S
            &[vk::ALT, vk::TAB],     // Alt+Tab
            &[vk::ALT, 0x73],        // Alt+F4
            &[vk::WIN, 0x4C],        // Win+L
            &[vk::WIN, 0x44],        // Win+D
            &[vk::WIN, 0x45],        // Win+E
            &[vk::WIN, 0x52],        // Win+R
            &[vk::WIN, vk::TAB],     // Win+Tab
            &[vk::CTRL, vk::MOUSE_LEFT],  // Ctrl+click: open in new tab, pick several items
            &[vk::SHIFT, vk::MOUSE_LEFT], // Shift+click: select a range
        ];
        for r in reserved {
            if KeyCombo::new(r).keys == self.keys {
                return Err(format!("{} is used by Windows or most apps.", self.label()));
            }
        }
        Ok(())
    }
}

/// What a recording is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingKind {
    Dictation,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelReason {
    /// A single short tap (not a hold, no second tap).
    SingleTap,
    /// User pressed Esc or the bar's ✕.
    User,
    /// Another shortcut was pressed while starting (e.g. Ctrl+Win+Left to switch desktops).
    OtherShortcut,
}

/// One-shot actions bound to their own combos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OneShot {
    PasteLast,
    CopyLast,
    UndoCleanup,
    RerunLast,
    OpenScratchpad,
    Transform(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Start { kind: RecordingKind, hands_free: bool },
    /// The current recording continues, now hands-free (no key needs holding).
    BecameHandsFree,
    /// The current recording becomes a command (e.g. Alt added to Ctrl+Win).
    BecameCommand,
    Finish,
    Cancel(CancelReason),
    OneShot(OneShot),
    /// A dictation shortcut was pressed while the previous dictation is still being
    /// processed; tell the user instead of silently ignoring it.
    Busy,
    /// Ask the OS layer to tap a harmless key now, so releasing Win/Alt afterwards doesn't
    /// open the Start menu, switch keyboard layout (Alt+Shift) or focus an app's menu bar.
    SuppressStartMenu,
    /// A dictation shortcut was pressed but deliberately did nothing (for the log only, so a
    /// "the shortcut didn't work" report can be explained). Never contains typed text.
    Ignored(&'static str),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct HotkeyConfig {
    pub dictate: Vec<KeyCombo>,
    pub hands_free: Vec<KeyCombo>,
    pub command: Vec<KeyCombo>,
    pub paste_last: Vec<KeyCombo>,
    pub copy_last: Vec<KeyCombo>,
    pub undo_cleanup: Vec<KeyCombo>,
    pub rerun_last: Vec<KeyCombo>,
    pub open_scratchpad: Vec<KeyCombo>,
}

impl HotkeyConfig {
    /// Wispr Flow's Windows defaults, plus Alt+Shift+U / R for undo and re-run.
    pub fn defaults() -> Self {
        use vk::*;
        Self {
            dictate: vec![KeyCombo::new(&[CTRL, WIN])],
            hands_free: vec![KeyCombo::new(&[CTRL, WIN, SPACE])],
            command: vec![KeyCombo::new(&[CTRL, WIN, ALT])],
            paste_last: vec![KeyCombo::new(&[ALT, SHIFT, 0x5A])],
            copy_last: vec![KeyCombo::new(&[ALT, SHIFT, 0x58])],
            undo_cleanup: vec![KeyCombo::new(&[ALT, SHIFT, 0x55])],
            rerun_last: vec![KeyCombo::new(&[ALT, SHIFT, 0x52])],
            open_scratchpad: vec![KeyCombo::new(&[WIN, ALT, 0x53])],
        }
    }

    /// Every combo in the config.
    pub fn all(&self) -> impl Iterator<Item = &KeyCombo> {
        [&self.dictate, &self.hands_free, &self.command, &self.paste_last, &self.copy_last, &self.undo_cleanup, &self.rerun_last, &self.open_scratchpad]
            .into_iter()
            .flatten()
    }
}

/// Bit mask (bit = button code) of the mouse buttons the hook must watch for these combos.
/// Bare left/right buttons are never watched.
pub fn mouse_button_mask<'a>(combos: impl IntoIterator<Item = &'a KeyCombo>) -> u32 {
    let mut mask = 0;
    for c in combos {
        if c.is_bare_left_right() {
            continue;
        }
        for b in c.mouse_buttons() {
            mask |= 1 << b;
        }
    }
    mask
}

/// Timings, in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// A press shorter than this counts as a tap rather than a hold.
    pub tap_max_ms: u64,
    /// After a tap, a second press within this window turns on hands-free.
    pub double_tap_window_ms: u64,
    /// Another key pressed this soon after starting cancels (it was a different shortcut).
    pub other_key_grace_ms: u64,
    /// `sync_keys` only forgets a key after this long without any event for it (a held key
    /// auto-repeats), so it can't race a key press that the OS hasn't recorded yet.
    pub stale_key_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self { tap_max_ms: 350, double_tap_window_ms: 450, other_key_grace_ms: 800, stale_key_ms: 1000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Combo held; recording. `since` = press time.
    Holding { kind: RecordingKind, since: u64 },
    /// Released quickly; recording continues while we wait for a possible second tap.
    TapPending { kind: RecordingKind, deadline: u64 },
    /// Hands-free recording; the combo that started it may still be held (`releasing`).
    HandsFree { kind: RecordingKind, releasing: bool },
    /// The outside world says we're busy processing; start presses are ignored.
    Processing,
    /// A recording was just finished by a key press; ignore combos until every key is up
    /// (so finishing with Ctrl+Win+Space doesn't immediately start a new recording).
    WaitRelease,
}

/// The output of feeding one key event.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub actions: Vec<Action>,
    /// True if the OS should not pass this key event on to other apps.
    pub swallow: bool,
}

/// Bookkeeping for double-click shortcuts (a mouse button on its own).
#[derive(Debug, Default)]
struct Clicks {
    /// A press that went through to the app: (button, time).
    down: Option<(u32, u64)>,
    /// A quick click just ended: (button, release time). A second press soon after is a double-click.
    last: Option<(u32, u64)>,
    /// After finishing with a click, the second click of a double-click is kept from the app too.
    swallow_next: Option<(u32, u64)>,
}

pub struct GestureEngine {
    config: HotkeyConfig,
    /// Extra one-shot bindings (e.g. transforms).
    extra: Vec<(OneShot, KeyCombo)>,
    timing: Timing,
    pressed: Vec<u32>,
    /// When each held key last sent an event (for `sync_keys`).
    touched: Vec<(u32, u64)>,
    state: State,
    /// The combo that is currently fully pressed (by exact match), if any.
    active: Option<(Binding, KeyCombo)>,
    /// Non-modifier keys we swallowed on key-down; their key-up is swallowed too.
    swallowed: Vec<u32>,
    clicks: Clicks,
    started_at: u64,
    /// Whether Esc should cancel (recording or processing is underway).
    cancel_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    Dictate,
    HandsFree,
    Command,
    OneShot(OneShot),
}

impl GestureEngine {
    pub fn new(config: HotkeyConfig, timing: Timing) -> Self {
        Self {
            config,
            extra: Vec::new(),
            timing,
            pressed: Vec::new(),
            touched: Vec::new(),
            state: State::Idle,
            active: None,
            swallowed: Vec::new(),
            clicks: Clicks::default(),
            started_at: 0,
            cancel_enabled: false,
        }
    }

    pub fn set_config(&mut self, config: HotkeyConfig) {
        self.config = config;
        self.active = None;
        self.clicks = Clicks::default();
    }

    pub fn set_extra(&mut self, extra: Vec<(OneShot, KeyCombo)>) {
        self.extra = extra;
        self.active = None;
    }

    pub fn is_recording(&self) -> bool {
        matches!(
            self.state,
            State::Holding { .. } | State::TapPending { .. } | State::HandsFree { .. }
        )
    }

    /// Called by the app when processing starts/ends so new presses are ignored meanwhile
    /// and Esc can cancel processing.
    pub fn set_processing(&mut self, processing: bool) {
        if processing {
            self.state = State::Processing;
            self.cancel_enabled = true;
        } else if self.state == State::Processing {
            self.state = State::Idle;
            self.cancel_enabled = false;
        }
    }

    /// A recording was started outside the keyboard (bar button, Scratchpad): treat it as
    /// hands-free so the dictation shortcut finishes it and Esc cancels it.
    pub fn start_external(&mut self, kind: RecordingKind, now: u64) {
        self.state = State::HandsFree { kind, releasing: false };
        self.started_at = now;
        self.cancel_enabled = true;
    }

    /// Called when the recording ends for reasons outside the engine (bar ✕/✓ clicked, error).
    pub fn reset_recording(&mut self) {
        if self.is_recording() {
            self.state = State::Idle;
        }
        self.cancel_enabled = false;
    }

    /// Forget all pressed keys (e.g. after the workstation is locked and key-ups were missed).
    pub fn clear_pressed(&mut self) {
        self.pressed.clear();
        self.touched.clear();
        self.swallowed.clear();
        self.active = None;
    }

    /// Brings our idea of which keys are held in line with the OS, which knows better when we
    /// missed a key-down or key-up: focus moved to an admin window or the lock screen, or
    /// Windows dropped the hook for a moment. `os_down(code)` asks the OS whether a key
    /// (generic modifier codes for modifiers) is physically held.
    ///
    /// - A modifier the OS says is held is added (its press was missed), so the next key
    ///   completes the combo.
    /// - A key the OS says is up, with no event for `stale_key_ms`, is dropped. Keys we
    ///   swallowed never look held to the OS, so for those only the silence counts (a held key
    ///   keeps auto-repeating), except while they are being held to talk (auto-repeat can be
    ///   switched off); a swallowed mouse button (no auto-repeat) is never dropped.
    ///
    /// Dropping keys finishes a hold the user already let go of, and ends "wait until the
    /// keys are released" states, so the next press of the shortcut works. Called by the
    /// hook before every key-down and by the app every half second.
    pub fn sync_keys(&mut self, os_down: &dyn Fn(u32) -> bool, now: u64) -> Vec<Action> {
        for m in vk::MODIFIERS {
            if !self.pressed.contains(&m) && os_down(m) {
                self.pressed.push(m);
                self.touch(m, now);
            }
        }
        let stale = |k: u32, me: &Self| now.saturating_sub(me.touched_at(k)) >= me.timing.stale_key_ms;
        let gone: Vec<u32> = self
            .pressed
            .iter()
            .copied()
            .filter(|&k| {
                if self.swallowed.contains(&k) {
                    let held_to_talk = matches!(self.state, State::Holding { .. }) && self.active.as_ref().is_some_and(|(_, c)| c.contains(k));
                    !vk::is_mouse(k) && !held_to_talk && stale(k, self)
                } else {
                    !os_down(k) && stale(k, self)
                }
            })
            .collect();
        let mut out = Outcome::default();
        self.drop_keys(&gone, now, &mut out);
        out.actions
    }

    /// Whether one of the recording shortcuts (dictate, hands-free, command) is fully held
    /// according to the OS, for explaining why it did nothing when Windows hid the keys from
    /// us. Double-click shortcuts don't count.
    pub fn start_combo_held(&self, os_down: &dyn Fn(u32) -> bool) -> bool {
        let c = &self.config;
        [&c.dictate, &c.hands_free, &c.command]
            .into_iter()
            .flatten()
            .any(|k| !k.is_empty() && k.click_button().is_none() && k.keys.iter().all(|&key| os_down(key)))
    }

    /// Keys we think are held (for asking the OS about them).
    pub fn pressed_keys(&self) -> Vec<u32> {
        self.pressed.clone()
    }

    /// Earliest time at which `tick` needs to be called.
    pub fn next_deadline(&self) -> Option<u64> {
        match self.state {
            State::TapPending { deadline, .. } => Some(deadline),
            _ => None,
        }
    }

    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        if let State::TapPending { deadline, .. } = self.state {
            if now >= deadline {
                self.state = State::Idle;
                self.cancel_enabled = false;
                return vec![Action::Cancel(CancelReason::SingleTap)];
            }
        }
        Vec::new()
    }

    pub fn key_down(&mut self, code: u32, now: u64) -> Outcome {
        let code = vk::normalize(code);
        let mut out = Outcome::default();
        if self.pressed.contains(&code) {
            if vk::is_mouse(code) {
                // Mouse buttons don't auto-repeat: we missed this button's release.
                self.drop_keys(&[code], now, &mut out);
            } else {
                // Auto-repeat: keep swallowing keys we swallowed.
                self.touch(code, now);
                out.swallow = self.swallowed.contains(&code);
                return out;
            }
        }
        self.touch(code, now);

        // A mouse button that is a double-click shortcut, with no modifier held.
        if !self.pressed.iter().any(|&k| vk::is_modifier(k)) {
            if let Some(binding) = self.click_binding(code) {
                self.pressed.push(code);
                self.click_down(code, binding, now, &mut out);
                return out;
            }
        }
        self.pressed.push(code);

        // Esc cancels while recording or processing.
        if code == vk::ESC && self.cancel_enabled {
            self.state = State::Idle;
            self.cancel_enabled = false;
            self.swallowed.push(code);
            out.swallow = true;
            out.actions.push(Action::Cancel(CancelReason::User));
            return out;
        }

        let matched = self.match_exact();
        match matched {
            Some((binding, combo)) => {
                self.active = Some((binding, combo.clone()));
                self.on_combo_pressed(binding, &combo, now, &mut out);
                if !vk::is_modifier(code) {
                    self.swallowed.push(code);
                    out.swallow = true;
                }
            }
            None => {
                // A key outside any combo. If it extends a just-started hold, the user was
                // really pressing some other shortcut (e.g. Ctrl+Win+Right): cancel. A mouse
                // click (e.g. into the text box) is not another shortcut.
                if !vk::is_modifier(code) && !vk::is_mouse(code) {
                    if let State::Holding { since, .. } = self.state {
                        if now.saturating_sub(since) <= self.timing.other_key_grace_ms {
                            self.state = State::Idle;
                            self.cancel_enabled = false;
                            out.actions.push(Action::Cancel(CancelReason::OtherShortcut));
                        }
                    }
                }
                self.active = None;
            }
        }
        out
    }

    pub fn key_up(&mut self, code: u32, now: u64) -> Outcome {
        let code = vk::normalize(code);
        let mut out = Outcome::default();
        self.pressed.retain(|&k| k != code);
        self.touched.retain(|&(k, _)| k != code);
        if let Some(pos) = self.swallowed.iter().position(|&k| k == code) {
            self.swallowed.remove(pos);
            out.swallow = true;
        }
        if let Some((b, t)) = self.clicks.down {
            if b == code {
                self.clicks.down = None;
                // Only a quick click can be the first half of a double-click.
                self.clicks.last = (now.saturating_sub(t) < self.timing.tap_max_ms).then_some((code, now));
            }
        }
        if self.state == State::WaitRelease && self.pressed.is_empty() {
            self.state = State::Idle;
        }
        let Some((_, combo)) = self.active.clone() else {
            if let State::HandsFree { kind, releasing: true } = self.state {
                if self.pressed.is_empty() {
                    self.state = State::HandsFree { kind, releasing: false };
                }
            }
            return out;
        };
        if !combo.contains(code) {
            return out;
        }
        self.release_active(now, &mut out);
        out
    }

    fn touch(&mut self, code: u32, now: u64) {
        match self.touched.iter_mut().find(|(k, _)| *k == code) {
            Some(t) => t.1 = now,
            None => self.touched.push((code, now)),
        }
    }

    fn touched_at(&self, code: u32) -> u64 {
        self.touched.iter().find(|(k, _)| *k == code).map(|t| t.1).unwrap_or(0)
    }

    /// The active combo was let go of.
    fn release_active(&mut self, now: u64, out: &mut Outcome) {
        let Some((binding, _)) = self.active.take() else { return };
        match (binding, self.state) {
            (Binding::Dictate | Binding::Command, State::Holding { kind, since }) => {
                if now.saturating_sub(since) >= self.timing.tap_max_ms {
                    self.state = State::Idle;
                    out.actions.push(Action::Finish);
                } else {
                    self.state = State::TapPending {
                        kind,
                        deadline: now + self.timing.double_tap_window_ms,
                    };
                }
            }
            (_, State::HandsFree { kind, releasing: true }) => {
                self.state = State::HandsFree { kind, releasing: false };
            }
            _ => {}
        }
    }

    /// Forgets keys whose release we never saw, as if they had just been released.
    fn drop_keys(&mut self, gone: &[u32], now: u64, out: &mut Outcome) {
        if gone.is_empty() {
            return;
        }
        self.pressed.retain(|k| !gone.contains(k));
        self.swallowed.retain(|k| !gone.contains(k));
        self.touched.retain(|(k, _)| !gone.contains(k));
        let active_released = self.active.as_ref().map(|(_, c)| gone.iter().any(|&k| c.contains(k))).unwrap_or(false);
        let mut finished = false;
        if active_released {
            let before = out.actions.len();
            self.release_active(now, out);
            finished = out.actions[before..].contains(&Action::Finish);
            if finished {
                // The key press that revealed this must not start a new recording straight away.
                self.state = State::WaitRelease;
            }
        }
        if self.pressed.is_empty() && !finished {
            match self.state {
                State::WaitRelease => self.state = State::Idle,
                State::HandsFree { kind, releasing: true } => self.state = State::HandsFree { kind, releasing: false },
                _ => {}
            }
        }
    }

    /// Every binding with its combos, in matching priority order.
    fn bindings(&self) -> Vec<(Binding, &KeyCombo)> {
        let c = &self.config;
        let groups: [(Binding, &Vec<KeyCombo>); 8] = [
            (Binding::HandsFree, &c.hands_free),
            (Binding::Command, &c.command),
            (Binding::Dictate, &c.dictate),
            (Binding::OneShot(OneShot::PasteLast), &c.paste_last),
            (Binding::OneShot(OneShot::CopyLast), &c.copy_last),
            (Binding::OneShot(OneShot::UndoCleanup), &c.undo_cleanup),
            (Binding::OneShot(OneShot::RerunLast), &c.rerun_last),
            (Binding::OneShot(OneShot::OpenScratchpad), &c.open_scratchpad),
        ];
        let mut v: Vec<(Binding, &KeyCombo)> = Vec::new();
        for (b, combos) in groups {
            v.extend(combos.iter().map(|k| (b, k)));
        }
        v.extend(self.extra.iter().map(|(s, k)| (Binding::OneShot(*s), k)));
        v
    }

    fn match_exact(&self) -> Option<(Binding, KeyCombo)> {
        let current = KeyCombo::new(&self.pressed);
        self.bindings()
            .into_iter()
            .find(|(_, c)| !c.is_empty() && c.click_button().is_none() && !c.is_bare_left_right() && c.keys == current.keys)
            .map(|(b, _)| (b, current))
    }

    /// The binding whose double-click shortcut is this mouse button, if any.
    fn click_binding(&self, button: u32) -> Option<Binding> {
        if !vk::is_mouse(button) {
            return None;
        }
        self.bindings().into_iter().find(|(_, c)| c.click_button() == Some(button)).map(|(b, _)| b)
    }

    fn swallow_click(&mut self, button: u32, out: &mut Outcome) {
        self.swallowed.push(button);
        out.swallow = true;
    }

    /// A press of a double-click shortcut button. Single clicks pass through to the app.
    fn click_down(&mut self, button: u32, binding: Binding, now: u64, out: &mut Outcome) {
        let window = self.timing.double_tap_window_ms;
        if let Some((b, until)) = self.clicks.swallow_next.take() {
            if b == button && now <= until {
                // Second click of a double-click used to stop: keep it from the app too.
                self.swallow_click(button, out);
                return;
            }
        }
        let recording_binding = !matches!(binding, Binding::OneShot(_));
        match self.state {
            State::HandsFree { .. } if recording_binding => {
                // One click while recording hands-free finishes it; the click doesn't reach the
                // app. Clicks right after starting (a triple-click) are just swallowed.
                self.swallow_click(button, out);
                if now.saturating_sub(self.started_at) >= window {
                    self.finish_by_press(out);
                    self.clicks.swallow_next = Some((button, now + window));
                } else {
                    out.actions.push(Action::Ignored("click right after starting"));
                }
            }
            State::Idle | State::Processing => match self.clicks.last {
                Some((b, t)) if b == button && now.saturating_sub(t) <= window => {
                    self.clicks.last = None;
                    self.swallow_click(button, out);
                    match binding {
                        Binding::OneShot(s) => out.actions.push(Action::OneShot(s)),
                        _ if self.state == State::Processing => out.actions.push(Action::Busy),
                        _ => {
                            let kind = if binding == Binding::Command { RecordingKind::Command } else { RecordingKind::Dictation };
                            self.state = State::HandsFree { kind, releasing: true };
                            self.started_at = now;
                            self.cancel_enabled = true;
                            out.actions.push(Action::Start { kind, hands_free: true });
                        }
                    }
                }
                _ => {
                    self.clicks.last = None;
                    self.clicks.down = Some((button, now));
                }
            },
            // A keyboard shortcut is in progress, or this button only runs a one-shot: an
            // ordinary click.
            _ => {}
        }
    }

    fn on_combo_pressed(
        &mut self,
        binding: Binding,
        combo: &KeyCombo,
        now: u64,
        out: &mut Outcome,
    ) {
        if combo.has_win() || combo.contains(vk::ALT) {
            out.actions.push(Action::SuppressStartMenu);
        }
        match binding {
            Binding::OneShot(s) => {
                if !self.is_recording() {
                    out.actions.push(Action::OneShot(s));
                }
            }
            Binding::Dictate | Binding::Command => {
                let kind = if binding == Binding::Command {
                    RecordingKind::Command
                } else {
                    RecordingKind::Dictation
                };
                match self.state {
                    State::Idle => {
                        self.state = State::Holding { kind, since: now };
                        self.started_at = now;
                        self.cancel_enabled = true;
                        out.actions.push(Action::Start { kind, hands_free: false });
                    }
                    State::Holding { kind: held, since } => {
                        // Ctrl+Win held, then Alt added → this recording is a command.
                        if binding == Binding::Command && held == RecordingKind::Dictation {
                            self.state = State::Holding { kind: RecordingKind::Command, since };
                            out.actions.push(Action::BecameCommand);
                        }
                    }
                    State::TapPending { kind: pending, deadline } if now <= deadline => {
                        self.state = State::HandsFree { kind: pending, releasing: true };
                        out.actions.push(Action::BecameHandsFree);
                    }
                    State::TapPending { .. } => {
                        // Deadline passed without a tick; treat as a fresh start.
                        self.state = State::Holding { kind, since: now };
                        self.started_at = now;
                        out.actions.push(Action::Cancel(CancelReason::SingleTap));
                        out.actions.push(Action::Start { kind, hands_free: false });
                    }
                    State::HandsFree { releasing, .. } => {
                        if releasing {
                            out.actions.push(Action::Ignored("keys that started hands-free are still held"));
                        } else {
                            self.finish_by_press(out);
                        }
                    }
                    State::Processing => out.actions.push(Action::Busy),
                    State::WaitRelease => out.actions.push(Action::Ignored("waiting for the last shortcut's keys to be released")),
                }
            }
            Binding::HandsFree => match self.state {
                State::Idle => {
                    self.state = State::HandsFree { kind: RecordingKind::Dictation, releasing: true };
                    self.started_at = now;
                    self.cancel_enabled = true;
                    out.actions.push(Action::Start { kind: RecordingKind::Dictation, hands_free: true });
                }
                State::Holding { kind, .. } | State::TapPending { kind, .. } => {
                    self.state = State::HandsFree { kind, releasing: true };
                    out.actions.push(Action::BecameHandsFree);
                }
                State::HandsFree { releasing: false, .. } => self.finish_by_press(out),
                State::Processing => out.actions.push(Action::Busy),
                State::HandsFree { releasing: true, .. } => {
                    out.actions.push(Action::Ignored("keys that started hands-free are still held"))
                }
                State::WaitRelease => out.actions.push(Action::Ignored("waiting for the last shortcut's keys to be released")),
            },
        }
    }

    fn finish_by_press(&mut self, out: &mut Outcome) {
        self.state = State::WaitRelease;
        self.cancel_enabled = false;
        out.actions.push(Action::Finish);
    }
}

#[cfg(test)]
mod tests {
    use super::vk::*;
    use super::*;

    fn engine() -> GestureEngine {
        GestureEngine::new(HotkeyConfig::defaults(), Timing::default())
    }

    fn press(e: &mut GestureEngine, keys: &[u32], t: u64) -> Vec<Action> {
        let mut acts = vec![];
        for &k in keys {
            acts.extend(e.key_down(k, t).actions);
        }
        acts
    }

    fn release(e: &mut GestureEngine, keys: &[u32], t: u64) -> Vec<Action> {
        let mut acts = vec![];
        for &k in keys {
            acts.extend(e.key_up(k, t).actions);
        }
        acts
    }

    /// Drops the OS-only and log-only actions.
    fn without_suppress(a: Vec<Action>) -> Vec<Action> {
        a.into_iter().filter(|x| *x != Action::SuppressStartMenu && !matches!(x, Action::Ignored(_))).collect()
    }

    #[test]
    fn hold_to_talk_starts_immediately_and_finishes_on_release() {
        let mut e = engine();
        let a = without_suppress(press(&mut e, &[LCTRL, LWIN_ALIAS], 1000));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
        let a = release(&mut e, &[LWIN_ALIAS, LCTRL], 3000);
        assert_eq!(a, vec![Action::Finish]);
        assert!(!e.is_recording());
    }

    // Left Win's code is the generic WIN code.
    const LWIN_ALIAS: u32 = WIN;

    #[test]
    fn win_combo_requests_start_menu_suppression() {
        let mut e = engine();
        let a = press(&mut e, &[CTRL, WIN], 0);
        assert!(a.contains(&Action::SuppressStartMenu));
    }

    #[test]
    fn double_tap_switches_to_hands_free_without_restarting() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        assert!(release(&mut e, &[WIN, CTRL], 120).is_empty());
        assert!(e.is_recording());
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 300));
        assert_eq!(a, vec![Action::BecameHandsFree]);
        // Releasing after the second tap does nothing.
        assert!(release(&mut e, &[WIN, CTRL], 380).is_empty());
        assert!(e.is_recording());
        // Tap again to finish.
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 20_000));
        assert_eq!(a, vec![Action::Finish]);
        assert!(release(&mut e, &[WIN, CTRL], 20_100).is_empty());
        assert!(!e.is_recording());
    }

    #[test]
    fn single_tap_cancels_after_window() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        release(&mut e, &[WIN, CTRL], 100);
        assert_eq!(e.next_deadline(), Some(100 + 450));
        assert!(e.tick(400).is_empty());
        assert_eq!(e.tick(560), vec![Action::Cancel(CancelReason::SingleTap)]);
        assert!(!e.is_recording());
    }

    #[test]
    fn hands_free_combo_toggles() {
        let mut e = engine();
        // Typical order: Ctrl, Win, then Space. Ctrl+Win starts a hold first.
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 0));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
        let out = e.key_down(SPACE, 60);
        assert!(out.swallow, "space must not reach the app");
        assert_eq!(without_suppress(out.actions), vec![Action::BecameHandsFree]);
        assert!(e.key_up(SPACE, 150).swallow);
        // Releasing Space leaves Ctrl+Win held; that must not stop the recording.
        release(&mut e, &[WIN, CTRL], 200);
        assert!(e.is_recording());
        // Press the hands-free combo again: Ctrl+Win alone already finishes...
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 9000));
        assert_eq!(a, vec![Action::Finish]);
        // ...and completing it with Space must NOT start a new recording.
        let out = e.key_down(SPACE, 9050);
        assert!(out.swallow);
        assert!(without_suppress(out.actions).is_empty());
        release(&mut e, &[SPACE, WIN, CTRL], 9100);
        assert!(!e.is_recording());
        // After everything is released, a new hold works again.
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 12_000));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn hands_free_combo_from_idle_when_space_pressed_first_is_impossible_but_direct_works() {
        let mut cfg = HotkeyConfig::defaults();
        cfg.dictate = vec![KeyCombo::new(&[CTRL, WIN])];
        cfg.hands_free = vec![KeyCombo::new(&[ALT, SHIFT, SPACE])];
        let mut e = GestureEngine::new(cfg, Timing::default());
        let a = without_suppress(press(&mut e, &[ALT, SHIFT, SPACE], 0));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: true }]);
        release(&mut e, &[SPACE, SHIFT, ALT], 100);
        assert!(e.is_recording());
        let a = without_suppress(press(&mut e, &[ALT, SHIFT, SPACE], 5000));
        assert_eq!(a, vec![Action::Finish]);
    }

    #[test]
    fn adding_alt_turns_dictation_into_command() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        let a = without_suppress(e.key_down(ALT, 40).actions);
        assert_eq!(a, vec![Action::BecameCommand]);
        let a = release(&mut e, &[ALT, WIN, CTRL], 2500);
        assert_eq!(a, vec![Action::Finish]);
    }

    #[test]
    fn command_combo_from_idle() {
        let mut cfg = HotkeyConfig::defaults();
        cfg.command = vec![KeyCombo::new(&[CTRL, ALT, 0x51])]; // Ctrl+Alt+Q
        let mut e = GestureEngine::new(cfg, Timing::default());
        press(&mut e, &[CTRL, ALT], 0);
        let out = e.key_down(0x51, 10);
        assert!(out.swallow);
        assert_eq!(without_suppress(out.actions), vec![Action::Start { kind: RecordingKind::Command, hands_free: false }]);
    }

    #[test]
    fn other_shortcut_soon_after_start_cancels() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        let out = e.key_down(0x27, 150); // Right arrow: Ctrl+Win+Right switches desktops
        assert!(!out.swallow);
        assert_eq!(out.actions, vec![Action::Cancel(CancelReason::OtherShortcut)]);
        assert!(!e.is_recording());
    }

    #[test]
    fn typing_after_grace_period_does_not_cancel() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        let out = e.key_down(0x41, 2000);
        assert!(out.actions.is_empty());
        assert!(e.is_recording());
    }

    #[test]
    fn esc_cancels_and_is_swallowed_only_while_active() {
        let mut e = engine();
        let out = e.key_down(ESC, 0);
        assert!(!out.swallow);
        e.key_up(ESC, 10);
        press(&mut e, &[CTRL, WIN], 100);
        release(&mut e, &[WIN, CTRL], 150);
        press(&mut e, &[CTRL, WIN], 300); // hands-free
        release(&mut e, &[WIN, CTRL], 350);
        let out = e.key_down(ESC, 5000);
        assert!(out.swallow);
        assert_eq!(out.actions, vec![Action::Cancel(CancelReason::User)]);
        assert!(e.key_up(ESC, 5050).swallow);
        assert!(!e.is_recording());
    }

    #[test]
    fn processing_blocks_new_recordings_but_esc_cancels() {
        let mut e = engine();
        e.set_processing(true);
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 0));
        assert_eq!(a, vec![Action::Busy]);
        release(&mut e, &[WIN, CTRL], 50);
        let out = e.key_down(ESC, 100);
        assert_eq!(out.actions, vec![Action::Cancel(CancelReason::User)]);
        e.key_up(ESC, 120);
        e.set_processing(false);
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 200));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn one_shots_fire_and_swallow() {
        let mut e = engine();
        press(&mut e, &[ALT, SHIFT], 0);
        let out = e.key_down(0x5A, 10);
        assert!(out.swallow);
        // Alt+Shift with the letter swallowed must be masked or Windows switches layout.
        assert_eq!(out.actions, vec![Action::SuppressStartMenu, Action::OneShot(OneShot::PasteLast)]);
        assert!(e.key_up(0x5A, 50).swallow);
    }

    #[test]
    fn extra_one_shots_for_transforms() {
        let mut e = engine();
        e.set_extra(vec![(OneShot::Transform(0), KeyCombo::new(&[WIN, ALT, 0x31]))]);
        press(&mut e, &[WIN, ALT], 0);
        let out = e.key_down(0x31, 5);
        assert!(out.swallow);
        assert!(out.actions.contains(&Action::OneShot(OneShot::Transform(0))));
    }

    #[test]
    fn externally_started_recording_stops_with_hotkey_or_esc() {
        let mut e = engine();
        e.start_external(RecordingKind::Dictation, 0);
        assert!(e.is_recording());
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 5000));
        assert_eq!(a, vec![Action::Finish]);
        release(&mut e, &[WIN, CTRL], 5100);
        e.start_external(RecordingKind::Dictation, 6000);
        assert_eq!(e.key_down(ESC, 7000).actions, vec![Action::Cancel(CancelReason::User)]);
    }

    #[test]
    fn auto_repeat_is_ignored() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        for t in (30..1000).step_by(30) {
            assert!(e.key_down(WIN, t).actions.is_empty());
        }
        assert_eq!(release(&mut e, &[WIN, CTRL], 1200), vec![Action::Finish]);
    }

    #[test]
    fn right_hand_modifiers_are_normalised() {
        let mut e = engine();
        let a = without_suppress(press(&mut e, &[RCTRL, RWIN], 0));
        assert_eq!(a.len(), 1);
        assert_eq!(release(&mut e, &[RWIN, RCTRL], 900), vec![Action::Finish]);
    }

    #[test]
    fn sync_modifiers_recovers_from_missed_key_up() {
        let mut e = engine();
        e.key_down(CTRL, 0);
        // Key-up for Ctrl was lost (e.g. lock screen). OS says nothing is down.
        e.sync_keys(&|_| false, 5000);
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 5100));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn combo_labels_and_validation() {
        assert_eq!(KeyCombo::new(&[WIN, CTRL]).label(), "Ctrl + Win");
        assert_eq!(KeyCombo::new(&[SPACE, WIN, LCTRL]).label(), "Ctrl + Win + Space");
        assert!(KeyCombo::new(&[CTRL, WIN]).validate().is_ok());
        assert!(KeyCombo::new(&[0x41]).validate().is_err());
        assert!(KeyCombo::new(&[CTRL]).validate().is_err());
        assert!(KeyCombo::new(&[CTRL, 0x56]).validate().is_err());
        assert!(KeyCombo::new(&[F1 + 8]).validate().is_ok());
        assert!(KeyCombo::new(&[MOUSE_X1]).validate().is_ok());
        assert!(KeyCombo::new(&[CTRL, ALT, SHIFT, 0x41]).validate().is_err());
    }

    // ---------------------------------------------------------------- missed keys / focus changes

    /// What the OS says is held: nothing.
    fn os_none(_: u32) -> bool {
        false
    }

    /// Like the hook: ask the OS (here: `held`) before every key-down.
    fn press_synced(e: &mut GestureEngine, keys: &[u32], held: &[u32], t: u64) -> Vec<Action> {
        let mut acts = vec![];
        for &k in keys {
            acts.extend(e.sync_keys(&|c| held.contains(&c), t));
            acts.extend(e.key_down(k, t).actions);
        }
        acts
    }

    #[test]
    fn hands_free_finishes_even_if_its_key_ups_were_missed() {
        // Double-tap for hands-free, then focus moves to an admin window before the keys are
        // let go, so their key-ups never arrive. Later the user presses the shortcut to finish.
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        release(&mut e, &[WIN, CTRL], 100);
        assert_eq!(without_suppress(press(&mut e, &[CTRL, WIN], 300)), vec![Action::BecameHandsFree]);
        // (key-ups lost)
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 8000));
        assert_eq!(a, vec![Action::Finish]);
    }

    #[test]
    fn without_sync_the_missed_key_ups_would_eat_the_finish_press() {
        // Documents why the hook syncs before each key-down.
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        release(&mut e, &[WIN, CTRL], 100);
        press(&mut e, &[CTRL, WIN], 300);
        // The new presses look like auto-repeat of keys still held: nothing happens.
        let a = press(&mut e, &[CTRL, WIN], 8000);
        assert!(a.is_empty());
    }

    #[test]
    fn ignored_presses_are_reported_for_the_log() {
        let mut e = engine();
        e.start_external(RecordingKind::Dictation, 0);
        assert_eq!(without_suppress(press(&mut e, &[CTRL, WIN], 5000)), vec![Action::Finish]);
        let a = e.key_down(SPACE, 5050).actions;
        assert!(a.iter().any(|x| matches!(x, Action::Ignored(_))), "{a:?}");
    }

    #[test]
    fn hold_whose_release_was_missed_finishes_on_next_press_without_restarting() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        // Released while an admin window had focus: key-ups lost. The user presses again.
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 6000));
        assert_eq!(a, vec![Action::Finish], "the press finishes the old recording and doesn't start a new one");
        e.set_processing(true);
        release(&mut e, &[WIN, CTRL], 6100);
        e.set_processing(false);
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 9000));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
    }

    #[test]
    fn periodic_sync_finishes_a_hold_that_was_let_go() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        // Too soon: the OS may simply not have recorded the press yet.
        assert!(e.sync_keys(&os_none, 500).is_empty());
        assert!(e.is_recording());
        assert_eq!(e.sync_keys(&os_none, 3000), vec![Action::Finish]);
        assert!(!e.is_recording());
    }

    #[test]
    fn sync_keeps_keys_the_os_says_are_held() {
        let mut e = engine();
        press(&mut e, &[CTRL, WIN], 0);
        assert!(e.sync_keys(&|k| k == CTRL || k == WIN, 60_000).is_empty());
        assert!(e.is_recording());
        assert_eq!(release(&mut e, &[WIN, CTRL], 61_000), vec![Action::Finish]);
    }

    #[test]
    fn missed_modifier_press_is_picked_up_from_the_os() {
        // Ctrl went down while the hook wasn't seeing keys (admin window, hook re-install).
        let mut e = engine();
        let a = without_suppress(press_synced(&mut e, &[WIN], &[CTRL], 1000));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
    }

    #[test]
    fn stuck_swallowed_key_is_forgotten_after_it_stops_repeating() {
        // Ctrl+Win+Space hands-free; Space (kept from the app, so the OS never saw it) is
        // released unseen. It must not turn the next Ctrl+Win into Ctrl+Win+Space.
        let mut e = engine();
        press(&mut e, &[CTRL, WIN, SPACE], 0);
        release(&mut e, &[WIN, CTRL], 100); // Space's key-up lost
        // Space still auto-repeating means it's held: kept.
        e.key_down(SPACE, 900);
        assert!(e.sync_keys(&os_none, 1200).is_empty());
        assert!(e.is_recording());
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 5000));
        assert_eq!(a, vec![Action::Finish], "Ctrl+Win finishes hands-free");
        e.set_processing(true);
        release(&mut e, &[WIN, CTRL], 5100);
        e.set_processing(false);
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 9000));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }], "dictate, not hands-free");
    }

    #[test]
    fn hold_to_talk_with_a_swallowed_key_survives_without_auto_repeat() {
        let mut cfg = HotkeyConfig::defaults();
        cfg.dictate = vec![KeyCombo::new(&[CAPSLOCK])];
        let mut e = GestureEngine::new(cfg, Timing::default());
        assert!(e.key_down(CAPSLOCK, 0).swallow);
        // No repeats (auto-repeat switched off); the OS never saw the swallowed press.
        assert!(e.sync_keys(&os_none, 5000).is_empty());
        assert!(e.is_recording());
        assert_eq!(e.key_up(CAPSLOCK, 6000).actions, vec![Action::Finish]);
    }

    #[test]
    fn wait_release_does_not_stick_when_key_ups_are_missed() {
        let mut e = engine();
        e.start_external(RecordingKind::Dictation, 0);
        assert_eq!(without_suppress(press(&mut e, &[CTRL, WIN], 5000)), vec![Action::Finish]);
        // The app never confirmed processing (e.g. the recording had already ended) and the
        // key-ups were lost. The next press must still work.
        let a = without_suppress(press_synced(&mut e, &[CTRL, WIN], &[], 9000));
        assert_eq!(a, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
    }

    // ---------------------------------------------------------------- mouse

    fn mouse_engine(dictate: KeyCombo) -> GestureEngine {
        let mut cfg = HotkeyConfig::defaults();
        cfg.dictate = vec![KeyCombo::new(&[CTRL, WIN]), dictate];
        GestureEngine::new(cfg, Timing::default())
    }

    fn click(e: &mut GestureEngine, b: u32, t: u64) -> (Outcome, Outcome) {
        let d = e.key_down(b, t);
        let u = e.key_up(b, t + 60);
        (d, u)
    }

    #[test]
    fn single_middle_click_passes_through_when_idle() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        let (d, u) = click(&mut e, MOUSE_MIDDLE, 0);
        assert!(!d.swallow && !u.swallow, "an ordinary middle click must reach the app");
        assert!(d.actions.is_empty() && u.actions.is_empty());
        assert!(!e.is_recording());
        // A second click long after is just another single click.
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 2000);
        assert!(!d.swallow && d.actions.is_empty());
    }

    #[test]
    fn double_middle_click_starts_hands_free_and_a_click_finishes() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        let (d1, u1) = click(&mut e, MOUSE_MIDDLE, 0);
        assert!(!d1.swallow && !u1.swallow, "the first click still reaches the app");
        let (d2, u2) = click(&mut e, MOUSE_MIDDLE, 200);
        assert!(d2.swallow && u2.swallow, "the second click is kept from the app");
        assert_eq!(d2.actions, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: true }]);
        assert!(e.is_recording());
        // A third click straight away (triple-click) doesn't stop it.
        let (d3, u3) = click(&mut e, MOUSE_MIDDLE, 350);
        assert!(d3.swallow && u3.swallow);
        assert!(e.is_recording());
        // Later, one click finishes; it doesn't reach the app.
        let (d, u) = click(&mut e, MOUSE_MIDDLE, 9000);
        assert!(d.swallow && u.swallow);
        assert_eq!(d.actions, vec![Action::Finish]);
        assert!(!e.is_recording());
        e.set_processing(true);
        // If the user double-clicked to stop, the second click is kept from the app too and
        // doesn't do anything else.
        let (d, u) = click(&mut e, MOUSE_MIDDLE, 9200);
        assert!(d.swallow && u.swallow && d.actions.is_empty());
        e.set_processing(false);
        // Back to normal clicks.
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 12_000);
        assert!(!d.swallow && d.actions.is_empty());
    }

    #[test]
    fn slow_or_long_clicks_are_not_a_double_click() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        click(&mut e, MOUSE_MIDDLE, 0);
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 60 + 600);
        assert!(!d.swallow && d.actions.is_empty(), "too slow");
        // A long press (auto-scroll, panning) followed by a click isn't a double-click either.
        let d = e.key_down(MOUSE_MIDDLE, 5000);
        assert!(!d.swallow);
        e.key_up(MOUSE_MIDDLE, 5800);
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 5900);
        assert!(!d.swallow && d.actions.is_empty());
    }

    #[test]
    fn plain_middle_button_combo_is_always_a_double_click() {
        // Saved before double-click existed: the middle button on its own never swallows a
        // single click.
        let mut e = mouse_engine(KeyCombo::new(&[MOUSE_MIDDLE]));
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 0);
        assert!(!d.swallow);
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 200);
        assert!(d.swallow);
        assert!(e.is_recording());
    }

    #[test]
    fn click_finishes_a_keyboard_started_hands_free_recording() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_X2));
        press(&mut e, &[CTRL, WIN], 0);
        release(&mut e, &[WIN, CTRL], 100);
        press(&mut e, &[CTRL, WIN], 300);
        release(&mut e, &[WIN, CTRL], 400);
        let (d, u) = click(&mut e, MOUSE_X2, 5000);
        assert!(d.swallow && u.swallow);
        assert_eq!(d.actions, vec![Action::Finish]);
    }

    #[test]
    fn double_click_with_a_modifier_held_is_an_ordinary_click() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        e.key_down(CTRL, 0);
        click(&mut e, MOUSE_MIDDLE, 10);
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 200);
        assert!(!d.swallow && d.actions.is_empty());
        assert!(!e.is_recording());
    }

    #[test]
    fn ctrl_middle_click_is_hold_to_talk() {
        let mut e = mouse_engine(KeyCombo::new(&[CTRL, MOUSE_MIDDLE]));
        e.key_down(CTRL, 0);
        let d = e.key_down(MOUSE_MIDDLE, 50);
        assert!(d.swallow);
        assert_eq!(without_suppress(d.actions), vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
        // Held for a while; the periodic sync must not drop the (swallowed) button.
        assert!(e.sync_keys(&|k| k == CTRL, 3000).is_empty());
        assert!(e.is_recording());
        let u = e.key_up(MOUSE_MIDDLE, 4000);
        assert!(u.swallow);
        assert_eq!(u.actions, vec![Action::Finish]);
        e.key_up(CTRL, 4050);
        // Without Ctrl, the middle button is an ordinary click.
        let (d, u) = click(&mut e, MOUSE_MIDDLE, 6000);
        assert!(!d.swallow && !u.swallow && d.actions.is_empty());
    }

    #[test]
    fn ctrl_middle_double_press_is_hands_free() {
        let mut e = mouse_engine(KeyCombo::new(&[CTRL, MOUSE_MIDDLE]));
        e.key_down(CTRL, 0);
        click(&mut e, MOUSE_MIDDLE, 10);
        let d = e.key_down(MOUSE_MIDDLE, 200);
        assert_eq!(without_suppress(d.actions), vec![Action::BecameHandsFree]);
        e.key_up(MOUSE_MIDDLE, 250);
        e.key_up(CTRL, 260);
        assert!(e.is_recording());
    }

    #[test]
    fn side_button_single_press_works_like_a_key() {
        let mut e = mouse_engine(KeyCombo::new(&[MOUSE_X1]));
        let d = e.key_down(MOUSE_X1, 0);
        assert!(d.swallow);
        assert_eq!(d.actions, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
        let u = e.key_up(MOUSE_X1, 2000);
        assert!(u.swallow);
        assert_eq!(u.actions, vec![Action::Finish]);
    }

    #[test]
    fn missed_mouse_button_release_doesnt_block_the_button() {
        let mut e = mouse_engine(KeyCombo::new(&[MOUSE_X1]));
        e.key_down(MOUSE_X1, 0); // up lost
        e.set_processing(true);
        e.set_processing(false);
        let d = e.key_down(MOUSE_X1, 5000);
        assert!(d.swallow);
        assert_eq!(d.actions, vec![Action::Start { kind: RecordingKind::Dictation, hands_free: false }]);
    }

    #[test]
    fn clicking_while_starting_a_keyboard_hold_does_not_cancel() {
        let mut cfg = HotkeyConfig::defaults();
        cfg.paste_last = vec![KeyCombo::new(&[ALT, MOUSE_LEFT])];
        let mut e = GestureEngine::new(cfg, Timing::default());
        press(&mut e, &[CTRL, WIN], 0);
        let d = e.key_down(MOUSE_LEFT, 100);
        assert!(!d.swallow && d.actions.is_empty());
        e.key_up(MOUSE_LEFT, 150);
        assert!(e.is_recording());
    }

    #[test]
    fn double_click_one_shot_fires_but_doesnt_finish_recordings() {
        let mut cfg = HotkeyConfig::defaults();
        cfg.paste_last = vec![KeyCombo::double_click(MOUSE_X2)];
        let mut e = GestureEngine::new(cfg, Timing::default());
        click(&mut e, MOUSE_X2, 0);
        let (d, _) = click(&mut e, MOUSE_X2, 200);
        assert_eq!(d.actions, vec![Action::OneShot(OneShot::PasteLast)]);
        e.start_external(RecordingKind::Dictation, 1000);
        let (d, _) = click(&mut e, MOUSE_X2, 5000);
        assert!(!d.swallow && d.actions.is_empty());
        assert!(e.is_recording());
    }

    #[test]
    fn double_click_while_processing_says_busy() {
        let mut e = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        e.set_processing(true);
        click(&mut e, MOUSE_MIDDLE, 0);
        let (d, _) = click(&mut e, MOUSE_MIDDLE, 200);
        assert!(d.swallow);
        assert_eq!(d.actions, vec![Action::Busy]);
    }

    #[test]
    fn mouse_combo_validation_and_labels() {
        assert_eq!(KeyCombo::double_click(MOUSE_MIDDLE).label(), "Double-click + Middle click");
        assert_eq!(KeyCombo::new(&[MOUSE_MIDDLE]).label(), "Double-click + Middle click");
        assert_eq!(KeyCombo::new(&[MOUSE_X1]).label(), "Back button");
        assert_eq!(KeyCombo::new(&[MOUSE_X2, LCTRL]).label(), "Ctrl + Forward button");
        assert!(KeyCombo::double_click(MOUSE_MIDDLE).validate().is_ok());
        assert!(KeyCombo::new(&[MOUSE_MIDDLE]).validate().is_ok());
        assert!(KeyCombo::double_click(MOUSE_X1).validate().is_ok());
        assert!(KeyCombo::new(&[MOUSE_X2]).validate().is_ok());
        assert!(KeyCombo::new(&[ALT, MOUSE_LEFT]).validate().is_ok());
        assert!(KeyCombo::new(&[CTRL, SHIFT, MOUSE_RIGHT]).validate().is_ok());
        let err = KeyCombo::new(&[MOUSE_LEFT]).validate().unwrap_err();
        assert!(err.contains("Ctrl, Alt, Shift or Win"), "{err}");
        assert!(KeyCombo::double_click(MOUSE_RIGHT).validate().is_err());
        assert!(KeyCombo::new(&[CTRL, MOUSE_LEFT]).validate().is_err(), "Ctrl+click is everyday clicking");
        assert!(KeyCombo::new(&[SHIFT, MOUSE_LEFT]).validate().is_err());
        assert!(KeyCombo::new(&[CTRL, MOUSE_MIDDLE, MOUSE_X1]).validate().is_err());
        let mut c = KeyCombo::new(&[CTRL, MOUSE_MIDDLE]);
        c.double_click = true;
        assert!(c.validate().is_err());
        assert!(KeyCombo::new(&[MOUSE_MIDDLE, 0x41]).validate().is_err());
    }

    #[test]
    fn mouse_mask_lists_watched_buttons() {
        let cfg = HotkeyConfig { dictate: vec![KeyCombo::double_click(MOUSE_MIDDLE)], paste_last: vec![KeyCombo::new(&[ALT, MOUSE_RIGHT])], ..HotkeyConfig::defaults() };
        assert_eq!(mouse_button_mask(cfg.all()), (1 << MOUSE_MIDDLE) | (1 << MOUSE_RIGHT));
        assert_eq!(mouse_button_mask(HotkeyConfig::defaults().all()), 0);
        assert_eq!(mouse_button_mask([&KeyCombo::new(&[MOUSE_LEFT])]), 0, "a bare left button is never watched");
    }

    #[test]
    fn start_combo_held_checks_recording_shortcuts_only() {
        let e = engine();
        assert!(e.start_combo_held(&|k| k == CTRL || k == WIN));
        assert!(!e.start_combo_held(&|k| k == CTRL));
        assert!(!e.start_combo_held(&|k| k == ALT || k == SHIFT || k == 0x5A), "paste-last isn't a recording shortcut");
        let m = mouse_engine(KeyCombo::double_click(MOUSE_MIDDLE));
        assert!(!m.start_combo_held(&|k| k == MOUSE_MIDDLE));
    }

    #[test]
    fn double_click_flag_round_trips_and_is_optional() {
        let c: KeyCombo = serde_json::from_str(r#"{"keys":[4]}"#).unwrap();
        assert!(!c.double_click);
        let json = serde_json::to_string(&KeyCombo::double_click(MOUSE_X1)).unwrap();
        assert_eq!(json, r#"{"keys":[5],"doubleClick":true}"#);
        assert_eq!(serde_json::to_string(&KeyCombo::new(&[CTRL, WIN])).unwrap(), r#"{"keys":[17,91]}"#);
    }
}
