//! UI Automation: is the focused control a text field, what text is around the cursor, and
//! what is selected. UIA calls go into the target app's process and can hang, so every
//! query runs on a worker thread with a timeout.

use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_HyperlinkControlTypeId,
    UIA_ImageControlTypeId, UIA_ListItemControlTypeId, UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_TabItemControlTypeId, UIA_TextPatternId, UIA_TreeItemControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

use super::super::{FieldContext, FocusState};

fn run_with_timeout<T: Send + 'static>(ms: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_millis(ms)).ok()
}

fn automation() -> Option<IUIAutomation> {
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
}

fn focused() -> Option<IUIAutomationElement> {
    unsafe { automation()?.GetFocusedElement().ok() }
}

fn foreground_class() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn classify(el: &IUIAutomationElement) -> FocusState {
    unsafe {
        let ct = el.CurrentControlType().unwrap_or_default();
        if ct == UIA_EditControlTypeId || ct == UIA_DocumentControlTypeId || ct == UIA_ComboBoxControlTypeId {
            if let Ok(v) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if v.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(false) && ct == UIA_EditControlTypeId {
                    return FocusState::NotEditable;
                }
            }
            return FocusState::Editable;
        }
        if el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).is_ok() {
            return FocusState::Editable;
        }
        let clearly_not_text = [
            UIA_ButtonControlTypeId,
            UIA_CheckBoxControlTypeId,
            UIA_RadioButtonControlTypeId,
            UIA_HyperlinkControlTypeId,
            UIA_ImageControlTypeId,
            UIA_ListItemControlTypeId,
            UIA_MenuItemControlTypeId,
            UIA_TabItemControlTypeId,
            UIA_TreeItemControlTypeId,
        ];
        if clearly_not_text.contains(&ct) {
            FocusState::NotEditable
        } else {
            FocusState::Unknown
        }
    }
}

/// Is the focused control a place text can go?
pub fn focus_state(timeout_ms: u64) -> FocusState {
    let class = foreground_class();
    if class.is_empty() || matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return FocusState::NotEditable;
    }
    run_with_timeout(timeout_ms, || focused().map(|el| classify(&el)).unwrap_or(FocusState::Unknown))
        .unwrap_or(FocusState::Unknown)
}

fn bstr_to_string(b: windows::core::BSTR) -> String {
    b.to_string()
}

fn read(el: &IUIAutomationElement) -> FieldContext {
    let mut ctx = FieldContext::default();
    unsafe {
        ctx.is_password = el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
        if ctx.is_password {
            return ctx;
        }
        if let Ok(tp) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
            let sel = tp.GetSelection().ok().and_then(|a| {
                if a.Length().unwrap_or(0) > 0 {
                    a.GetElement(0).ok()
                } else {
                    None
                }
            });
            if let Some(sel) = sel {
                let selected = sel.GetText(20_000).map(bstr_to_string).unwrap_or_default();
                if !selected.is_empty() {
                    ctx.selection = Some(selected);
                }
                // Read only a window around the cursor (never the whole document, which can
                // be huge and slow in big files).
                if let Ok(before) = sel.Clone() {
                    let ok = before.MoveEndpointByRange(TextPatternRangeEndpoint_End, &sel, TextPatternRangeEndpoint_Start).is_ok()
                        && before.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1500).is_ok();
                    if ok {
                        let t = before.GetText(1500).map(bstr_to_string).unwrap_or_default();
                        ctx.before = Some(tail(&t, 1500));
                    }
                }
                if let Ok(after) = sel.Clone() {
                    let ok = after.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &sel, TextPatternRangeEndpoint_End).is_ok()
                        && after.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, 200).is_ok();
                    if ok {
                        let t = after.GetText(200).map(bstr_to_string).unwrap_or_default();
                        ctx.after = Some(t.chars().take(200).collect());
                    }
                }
                return ctx;
            }
        }
        if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            // No caret information: only useful to know the field is empty.
            let v = vp.CurrentValue().map(bstr_to_string).unwrap_or_default();
            if v.is_empty() {
                ctx.before = Some(String::new());
            }
        }
    }
    ctx
}

fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

pub fn read_context(timeout_ms: u64) -> FieldContext {
    run_with_timeout(timeout_ms, || focused().map(|el| read(&el)).unwrap_or_default()).unwrap_or_default()
}
