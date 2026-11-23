//! Wisper desktop app: window management, tray, updater and wiring to the core pipeline.

mod account;
mod audio_capture;
mod backup;
mod bar;
mod commands;
mod controller;
mod platform;
mod secrets;
mod tray;
mod updater;

use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};

use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

use wisper_core::db::Db;
use wisper_core::hotkey::{GestureEngine, Timing};
use wisper_core::pipeline::{Engine, RealProviders};
use wisper_core::settings::Settings;

use crate::controller::{Controller, ControllerState};

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            init_logging(&data_dir);
            tracing::info!("Wisper {} starting", app.package_info().version);

            let settings_file = data_dir.join("settings.json");
            let first_run = !settings_file.exists();
            let settings = Settings::load(&settings_file);
            if first_run {
                let _ = settings.save(&settings_file);
            }
            let db = Arc::new(Db::open(&data_dir.join("wisper.db")).map_err(|e| format!("database: {e}"))?);
            let engine = Engine::new(db, data_dir.clone(), settings.clone(), commands::load_keys(), Arc::new(RealProviders));
            platform::restore_after_crash(&data_dir.join("muted-sessions.txt"));

            let (tx, rx) = channel();
            let hook = platform::HookShared::new(GestureEngine::new(settings.hotkeys.clone(), Timing::default()), tx);
            let capture = audio_capture::Capture::spawn();
            let controller = Controller::new(handle.clone(), engine, hook.clone(), capture, data_dir.clone());
            app.manage(ControllerState(controller.clone()));
            app.manage(commands::Paths { data_dir: data_dir.clone(), settings_file });
            account::init(&handle, controller.engine.db.clone(), data_dir.clone());

            tray::create(&handle)?;
            bar::create(&handle)?;
            #[cfg(windows)]
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(h) = w.hwnd() {
                    platform::set_owner_hwnd(h.0 as isize);
                }
            }
            commands::apply_settings(&handle, &settings);
            let al = handle.autolaunch();
            if al.is_enabled().unwrap_or(false) != settings.launch_at_login {
                let _ = if settings.launch_at_login { al.enable() } else { al.disable() };
            }
            platform::start_hook(hook);
            controller.spawn_hotkey_pump(rx);
            controller.spawn_queue_worker();
            controller.rest_bar();

            let started_in_background = std::env::args().any(|a| a == "--background");
            if !started_in_background || first_run {
                show_main_window(&handle);
            }
            updater::spawn_background_checks(handle.clone());
            account::start(&handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the hub hides it; the app keeps running in the tray.
            if let WindowEvent::Focused(true) = event {
                if window.label() == "main" {
                    account::on_focus(window.app_handle());
                }
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            updater::check_for_updates,
            commands::get_settings,
            commands::save_settings,
            commands::app_info,
            commands::open_data_folder,
            commands::list_history,
            commands::get_history_item,
            commands::delete_history_item,
            commands::reinsert_history_item,
            commands::reprocess_history_item,
            commands::retry_history_item,
            commands::copy_text,
            commands::list_dictionary,
            commands::add_dictionary_entry,
            commands::update_dictionary_entry,
            commands::delete_dictionary_entry,
            commands::list_suggestions,
            commands::resolve_suggestion,
            commands::import_dictionary_csv,
            commands::list_snippets,
            commands::save_snippet,
            commands::delete_snippet,
            commands::import_snippets_json,
            commands::list_modes,
            commands::default_mode_instructions,
            commands::get_stats,
            commands::key_status,
            commands::set_api_key,
            commands::test_api_key,
            commands::list_models,
            commands::list_microphones,
            commands::start_mic_test,
            commands::stop_mic_test,
            commands::bar_hit_area,
            commands::start_hotkey_capture,
            commands::stop_hotkey_capture,
            commands::describe_combo,
            commands::list_notes,
            commands::save_note,
            commands::delete_note,
            commands::dictate_into_scratchpad,
            commands::run_transform,
            commands::bar_start,
            commands::bar_finish,
            commands::bar_cancel,
            commands::bar_copy,
            commands::bar_undo,
            commands::bar_rerun,
            commands::bar_retry,
            commands::bar_open_hub,
            commands::bar_dismiss,
            commands::export_backup,
            commands::import_backup,
            account::account_status,
            account::account_sign_in,
            account::account_sign_out,
            account::account_cancel_sign_in,
            account::account_sync_now,
            account::account_set_key_storage,
            account::account_remove_cloud_keys,
            account::account_devices,
            account::account_remove_device,
            account::account_delete,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Wisper");
}

pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let appender = tracing_appender::rolling::daily(data_dir.join("logs"), "wisper.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(guard);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false).with_target(false))
        .with(fmt::layer().with_target(false))
        .try_init();
}
