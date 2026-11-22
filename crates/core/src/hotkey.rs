//! Hotkey gesture engine: turns raw key down/up events into dictation actions.
//!
//! Pure logic (no OS calls) so it can be unit-tested. The Windows keyboard hook feeds it
//! normalised virtual-key codes and asks it, synchronously, whether to swallow each event.
//!
//! Rules (deliberately simple, unlike Wispr's overlapping timers):
//! - Recording starts the instant a dictation combo is pressed, so first words are never lost.
//! - Hold the combo and release → finish and paste.
//! - Tap twice quickly → hands-free; tap once more to finish.
//! - A single short tap with no second tap → cancelled (nothing worth keeping was recorded).
//! - The hands-free combo starts/stops hands-free directly.
//! - Esc cancels while recording.

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
    /// Mouse buttons are mapped into a private range so combos can include them.
    pub const MOUSE_MIDDLE: u32 = 0x04;
    pub const MOUSE_X1: u32 = 0x05;
    pub const MOUSE_X2: u32 = 0x06;

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
            MOUSE_MIDDLE => "Middle mouse".into(),
            MOUSE_X1 => "Mouse 4".into(),
            MOUSE_X2 => "Mouse 5".into(),
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

/// A set of keys pressed together. Order is irrelevant; stored sorted and normalised.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct KeyCombo {
    pub keys: Vec<u32>,
}

impl KeyCombo {
    pub fn new(keys: &[u32]) -> Self {
        let mut k: Vec<u32> = keys.iter().map(|&c| vk::normalize(c)).collect();
        k.sort_unstable();
        k.dedup();
        Self { keys: k }
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

    /// e.g. "Ctrl + Win + Space" (modifiers first, in a fixed order).
    pub fn label(&self) -> String {
        let order = |c: &u32| match *c {
            vk::CTRL => 0,
            vk::WIN => 1,
            vk::ALT => 2,
            vk::SHIFT => 3,
            _ => 4,
        };
        let mut keys = self.keys.clone();
        keys.sort_by_key(|c| (order(c), *c));
        keys.iter().map(|&c| vk::name(c)).collect::<Vec<_>>().join(" + ")
    }

    /// Validates a user-chosen combo. Returns a plain-English problem if it can't be used.
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Err("Press at least one key.".into());
        }
        if self.keys.len() > 3 {
            return Err("Use 3 keys or fewer.".into());
        }
        let has_modifier = self.keys.iter().any(|&k| vk::is_modifier(k));
        let is_mouse = self
            .keys
            .iter()
            .any(|&k| matches!(k, vk::MOUSE_MIDDLE | vk::MOUSE_X1 | vk::MOUSE_X2));
        let is_fkey = self.keys.iter().any(|&k| (vk::F1..=vk::F24).contains(&k));
        let single_ok = self.keys.len() == 1 && (is_mouse || is_fkey || self.keys[0] == vk::CAPSLOCK);
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
        ];
        for r in reserved {
            if KeyCombo::new(r) == *self {
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
}

impl Default for Timing {
    fn default() -> Self {
        Self { tap_max_ms: 350, double_tap_window_ms: 450, other_key_grace_ms: 800 }
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

pub struct GestureEngine {
    config: HotkeyConfig,
    /// Extra one-shot bindings (e.g. transforms).
    extra: Vec<(OneShot, KeyCombo)>,
    timing: Timing,
    pressed: Vec<u32>,
    state: State,
    /// The combo that is currently fully pressed (by exact match), if any.
    active: Option<(Binding, KeyCombo)>,
    /// Non-modifier keys we swallowed on key-down; their key-up is swallowed too.
    swallowed: Vec<u32>,
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
            state: State::Idle,
            active: None,
            swallowed: Vec::new(),
            started_at: 0,
            cancel_enabled: false,
        }
    }

    pub fn set_config(&mut self, config: HotkeyConfig) {
        self.config = config;
        self.active = None;
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
        self.swallowed.clear();
        self.active = None;
    }

    /// Replace our idea of which modifiers are down with the OS's (guards against missed key-ups).
    pub fn sync_modifiers(&mut self, down: &[u32]) {
        let down: Vec<u32> = down.iter().map(|&c| vk::normalize(c)).collect();
        self.pressed.retain(|k| !vk::is_modifier(*k) || down.contains(k));
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
            // Auto-repeat: keep swallowing keys we swallowed.
            out.swallow = self.swallowed.contains(&code);
            return out;
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
                // really pressing some other shortcut (e.g. Ctrl+Win+Right): cancel.
                if !vk::is_modifier(code) {
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
        if let Some(pos) = self.swallowed.iter().position(|&k| k == code) {
            self.swallowed.remove(pos);
            out.swallow = true;
        }
        if self.state == State::WaitRelease && self.pressed.is_empty() {
            self.state = State::Idle;
        }
        let Some((binding, combo)) = self.active.clone() else {
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
        self.active = None;
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
        out
    }

    fn match_exact(&self) -> Option<(Binding, KeyCombo)> {
        let current = KeyCombo::new(&self.pressed);
        let groups: [(Binding, &Vec<KeyCombo>); 3] = [
            (Binding::HandsFree, &self.config.hands_free),
            (Binding::Command, &self.config.command),
            (Binding::Dictate, &self.config.dictate),
        ];
        for (b, combos) in groups {
            if combos.iter().any(|c| !c.is_empty() && *c == current) {
                return Some((b, current));
            }
        }
        let shots: [(OneShot, &Vec<KeyCombo>); 5] = [
            (OneShot::PasteLast, &self.config.paste_last),
            (OneShot::CopyLast, &self.config.copy_last),
            (OneShot::UndoCleanup, &self.config.undo_cleanup),
            (OneShot::RerunLast, &self.config.rerun_last),
            (OneShot::OpenScratchpad, &self.config.open_scratchpad),
        ];
        for (s, combos) in shots {
            if combos.iter().any(|c| !c.is_empty() && *c == current) {
                return Some((Binding::OneShot(s), current));
            }
        }
        for (s, combo) in &self.extra {
            if !combo.is_empty() && *combo == current {
                return Some((Binding::OneShot(*s), current));
            }
        }
        None
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
                        if !releasing {
                            self.finish_by_press(out);
                        }
                    }
                    State::Processing => out.actions.push(Action::Busy),
                    State::WaitRelease => {}
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
                State::HandsFree { releasing: true, .. } | State::WaitRelease => {}
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

    fn without_suppress(a: Vec<Action>) -> Vec<Action> {
        a.into_iter().filter(|x| *x != Action::SuppressStartMenu).collect()
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
        e.sync_modifiers(&[]);
        let a = without_suppress(press(&mut e, &[CTRL, WIN], 100));
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
}
