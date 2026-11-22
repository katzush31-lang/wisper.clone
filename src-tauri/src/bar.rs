//! The floating dictation bar: a small, transparent, always-on-top window that never takes focus.
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

/// Everything the bar shows. Mirrors `BarState` in src/shared/api.ts.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BarState {
    pub phase: &'static str,
    pub level: f32,
    pub elapsed_ms: u64,
    pub hands_free: bool,
    pub kind: &'static str,
    pub message: Option<String>,
    pub detail: Option<String>,
    pub text: Option<String>,
    pub dictation_id: Option<String>,
    pub actions: Vec<&'static str>,
    pub mode_name: Option<String>,
    pub modes: Vec<ModeRef>,
    pub hotkey_label: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModeRef {
    pub id: String,
    pub name: String,
}

impl BarState {
    pub fn new(phase: &'static str, hotkey_label: &str) -> Self {
        Self {
            phase,
            level: 0.0,
            elapsed_ms: 0,
            hands_free: false,
            kind: "dictation",
            message: None,
            detail: None,
            text: None,
            dictation_id: None,
            actions: Vec::new(),
            mode_name: None,
            modes: Vec::new(),
            hotkey_label: hotkey_label.to_string(),
        }
    }

    pub fn msg(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }

    pub fn actions(mut self, a: &[&'static str]) -> Self {
        self.actions = a.to_vec();
        self
    }
}

/// Sends a state to the bar window and shows/hides it. `idle` with always-show keeps a
/// small pill visible; `hidden` hides the window entirely.
pub fn emit(app: &AppHandle, state: &BarState) {
    let _ = app.emit_to(LABEL, "bar-state", state);
    if state.phase == "hidden" {
        hide(app);
    } else if let Some(w) = app.get_webview_window(LABEL) {
        if !w.is_visible().unwrap_or(false) {
            show(app);
        }
    }
}

pub const LABEL: &str = "bar";
pub const WIDTH: f64 = 420.0;
pub const HEIGHT: f64 = 150.0;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("bar.html".into()))
        .title("Wisper bar")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    make_non_activating(&window);
    position_bottom_center(app);
    Ok(())
}

/// Places the bar at the bottom centre of the monitor the mouse cursor is on, above the taskbar.
pub fn position_bottom_center(app: &AppHandle) {
    position(app, false)
}

/// Places the bar at the bottom (or top) centre of the monitor the mouse cursor is on.
pub fn position(app: &AppHandle, top: bool) {
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let monitor = w
        .cursor_position()
        .ok()
        .and_then(|p| w.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };
    let work = m.work_area();
    let size = match w.outer_size() {
        Ok(s) => s,
        Err(_) => return,
    };
    let x = work.position.x + (work.size.width as i32 - size.width as i32) / 2;
    let y = if top {
        work.position.y + 8
    } else {
        work.position.y + work.size.height as i32 - size.height as i32 - 8
    };
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

static TOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_top(top: bool) {
    TOP.store(top, std::sync::atomic::Ordering::SeqCst);
}

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        position(app, TOP.load(std::sync::atomic::Ordering::SeqCst));
        let _ = w.show();
    }
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

#[cfg(windows)]
fn make_non_activating(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    if let Ok(hwnd) = window.hwnd() {
        let hwnd = HWND(hwnd.0 as _);
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize,
            );
        }
    }
}

#[cfg(not(windows))]
fn make_non_activating(_window: &tauri::WebviewWindow) {}
