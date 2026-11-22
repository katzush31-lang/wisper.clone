//! Background auto-update from GitHub Releases. Updates download quietly and install only
//! when no dictation is in progress.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// Set while a dictation is recording or processing; updates never install then.
pub static BUSY: AtomicBool = AtomicBool::new(false);

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(45);
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub available: bool,
    pub version: Option<String>,
    pub message: String,
}

pub fn spawn_background_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if let Err(e) = check_and_prepare(&app, false).await {
                tracing::warn!("update check failed: {e}");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Checks for an update; if one exists, downloads it and installs once the app is idle.
pub async fn check_and_prepare(app: &AppHandle, user_initiated: bool) -> anyhow::Result<UpdateStatus> {
    let update = app.updater()?.check().await?;
    let Some(update) = update else {
        return Ok(UpdateStatus {
            available: false,
            version: None,
            message: "You're on the latest version.".into(),
        });
    };
    let version = update.version.clone();
    tracing::info!("update {version} available, downloading");
    let bytes = update.download(|_, _| {}, || {}).await?;
    if !user_initiated {
        let c = crate::controller::ControllerState::get(app);
        if c.engine.settings().notifications.update_ready {
            c.notify(
                "Wisper update ready",
                &format!("Version {version} will install in a moment, when you're not dictating. Wisper restarts by itself."),
            );
        }
    }
    // Wait until no dictation is running (checked every few seconds).
    while BUSY.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    if !user_initiated {
        // Give the user a quiet moment: wait until idle for a full minute.
        let mut idle_for = Duration::ZERO;
        while idle_for < Duration::from_secs(60) {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if BUSY.load(Ordering::SeqCst) {
                idle_for = Duration::ZERO;
            } else {
                idle_for += Duration::from_secs(5);
            }
        }
    }
    update.install(bytes)?;
    Ok(UpdateStatus {
        available: true,
        version: Some(version.clone()),
        message: format!("Installing version {version}…"),
    })
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    check_and_prepare(&app, true).await.map_err(|e| {
        format!("Couldn't check for updates right now. Check your internet connection and try again. ({e})")
    })
}
