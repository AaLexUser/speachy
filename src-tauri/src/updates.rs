//! Checking for, downloading and installing new versions of the app.
//!
//! Releases are published on GitHub. Each carries a `latest.json` naming
//! the newest version and, per platform, a signed archive of the app. The
//! archive is only installed if its signature matches the public key built
//! into this app (`plugins.updater.pubkey` in `tauri.conf.json`).

use crate::state::AppState;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Overrides where updates are looked for. For testing the updater against
/// a local server; archives must still carry a valid signature.
const ENDPOINT_VAR: &str = "SPEACHY_UPDATE_ENDPOINT";
/// How long after launch the first automatic check runs, and how often
/// after that.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Self-updating replaces the app bundle, which exists only on macOS.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    /// Not checked yet.
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// Installed; the app is about to restart into it.
    Installed {
        version: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<Option<Status>>,
    /// The update found by the last check, ready to install.
    pending: Mutex<Option<Update>>,
}

impl Updates {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone().unwrap_or(Status::Idle)
    }
}

fn set_status(app: &AppHandle, status: Status) {
    *app.state::<Updates>().status.lock().unwrap() = Some(status.clone());
    let _ = app.emit("update", status);
}

/// Looks for a newer version. Returns it, or `None` if this is the newest.
async fn find_update(app: &AppHandle) -> Result<Option<Update>, String> {
    let mut builder = app.updater_builder();
    if let Some(endpoint) = std::env::var(ENDPOINT_VAR).ok().filter(|v| !v.is_empty()) {
        let url = endpoint
            .parse()
            .map_err(|err| format!("{ENDPOINT_VAR} is not a URL: {err}"))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|err| err.to_string())?;
    }
    let updater = builder.build().map_err(|err| err.to_string())?;
    updater.check().await.map_err(|err| err.to_string())
}

/// Checks for an update and records the result. `quiet` checks (the
/// automatic ones) do not report failures or disturb an update in progress.
pub async fn check(app: &AppHandle, quiet: bool) -> Status {
    let updates = app.state::<Updates>();
    let busy = matches!(
        updates.status(),
        Status::Checking | Status::Downloading { .. } | Status::Installed { .. }
    );
    if busy {
        return updates.status();
    }
    let previous = updates.status();
    if !quiet {
        set_status(app, Status::Checking);
    }
    let status = match find_update(app).await {
        Ok(Some(update)) => {
            let status = Status::Available {
                version: update.version.clone(),
                notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
            };
            *updates.pending.lock().unwrap() = Some(update);
            status
        }
        Ok(None) => Status::UpToDate,
        Err(message) if quiet => {
            log::warn!("could not check for updates: {message}");
            previous
        }
        Err(message) => Status::Failed {
            message: format!("Could not check for updates: {message}"),
        },
    };
    set_status(app, status.clone());
    status
}

/// Downloads and installs the update found by the last check.
pub async fn install(app: &AppHandle) -> Result<String, String> {
    let updates = app.state::<Updates>();
    let update = updates
        .pending
        .lock()
        .unwrap()
        .take()
        .ok_or("there is no update to install; check for updates first")?;
    let version = update.version.clone();
    set_status(
        app,
        Status::Downloading {
            version: version.clone(),
            downloaded: 0,
            total: None,
        },
    );
    let progress_app = app.clone();
    let progress_version = version.clone();
    let mut downloaded = 0u64;
    let mut last_report = Instant::now();
    let result = update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                // Chunks arrive far more often than the window needs to hear.
                if last_report.elapsed() >= Duration::from_millis(100) {
                    last_report = Instant::now();
                    set_status(
                        &progress_app,
                        Status::Downloading {
                            version: progress_version.clone(),
                            downloaded,
                            total,
                        },
                    );
                }
            },
            || {},
        )
        .await;
    match result {
        Ok(()) => {
            log::info!("installed version {version}");
            set_status(
                app,
                Status::Installed {
                    version: version.clone(),
                },
            );
            Ok(version)
        }
        Err(err) => {
            let message = format!("the update could not be installed: {err}");
            // The update can be tried again.
            *updates.pending.lock().unwrap() = Some(update);
            set_status(
                app,
                Status::Failed {
                    message: message.clone(),
                },
            );
            Err(message)
        }
    }
}

/// Restarts into the version that was just installed.
pub fn restart(app: &AppHandle) -> ! {
    // A model must not be torn down under a thread still using it.
    if let Some(state) = app.try_state::<AppState>() {
        state.session.shutdown(Duration::from_secs(10));
    }
    app.restart()
}

/// Checks shortly after launch and then every few hours, for as long as
/// the user leaves automatic checks on.
pub fn start_automatic_checks(app: &AppHandle) {
    if !SUPPORTED {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut wait = FIRST_CHECK_AFTER;
        loop {
            // The runtime's timer is not exposed; a blocking sleep on its
            // blocking pool does the same without tying up a worker.
            let pause = wait;
            let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(pause)).await;
            wait = CHECK_EVERY;
            let enabled = app
                .try_state::<AppState>()
                .is_some_and(|state| state.session.settings().check_for_updates);
            if enabled {
                check(&app, true).await;
            }
        }
    });
}

/// `speachy --update`: check, install if there is something newer, report
/// on standard output and exit. No windows are opened.
pub fn run_and_exit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let code = match check(&app, false).await {
            Status::Available { version, .. } => match install(&app).await {
                Ok(_) => {
                    println!("Updated to version {version}. It will be used the next time Speachy starts.");
                    0
                }
                Err(message) => {
                    eprintln!("error: {message}");
                    1
                }
            },
            Status::UpToDate => {
                println!(
                    "Speachy {} is the newest version.",
                    app.package_info().version
                );
                0
            }
            Status::Failed { message } => {
                eprintln!("error: {message}");
                1
            }
            other => {
                eprintln!("error: unexpected update state {other:?}");
                1
            }
        };
        app.exit(code);
    });
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> Status {
    app.state::<Updates>().status()
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Status {
    check(&app, false).await
}

/// Installs the pending update and restarts the app into it.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    install(&app).await?;
    // Leave the "installed" state on screen for a moment before the window
    // goes away.
    let _ = tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(800)))
        .await;
    restart(&app)
}
