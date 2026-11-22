//! System tray icon and menu.
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter,
};

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Wisper", true, None::<&str>)?;
    let paste = MenuItem::with_id(app, "paste_last", "Paste last dictation", true, None::<&str>)?;
    let scratch = MenuItem::with_id(app, "scratchpad", "Open Scratchpad", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause shortcuts", true, false, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Check for updates", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Wisper", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &paste, &scratch, &sep, &pause, &update, &sep2, &quit])?;
    let pause_item = pause.clone();

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("bundled window icon");

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Wisper")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => crate::show_main_window(app),
            "paste_last" => {
                let c = crate::controller::ControllerState::get(app);
                std::thread::spawn(move || {
                    // Give focus back to the app under the menu first.
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    c.paste_last();
                });
            }
            "scratchpad" => {
                crate::show_main_window(app);
                let _ = app.emit_to("main", "navigate", "scratchpad");
            }
            "pause" => {
                let c = crate::controller::ControllerState::get(app);
                let now_paused = !c.hook.paused.load(std::sync::atomic::Ordering::SeqCst);
                c.hook.paused.store(now_paused, std::sync::atomic::Ordering::SeqCst);
                let _ = pause_item.set_checked(now_paused);
                if let Some(tray) = app.tray_by_id("main") {
                    let _ = tray.set_tooltip(Some(if now_paused { "Wisper (shortcuts paused)" } else { "Wisper" }));
                }
            }
            "update" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::updater::check_and_prepare(&app, true).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
