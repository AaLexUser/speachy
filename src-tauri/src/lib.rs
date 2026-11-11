//! Speachy: select text anywhere, press a shortcut, hear it read aloud by a
//! local TTS model.
//!
//! This crate is the desktop shell: windows, the menu-bar icon, global
//! shortcuts and reading the selection. Everything else is the engine in
//! `speachy-core`, reached through a [`Session`].

mod commands;
mod selection;
mod shortcuts;
mod speech;
mod state;
mod tray;
mod updates;
mod windows;

use speachy_core::audio::SystemAudio;
use speachy_core::pipeline::{SpeechEvent, State};
use speachy_core::session::{Event, Paths, Session};
use state::AppState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

/// Passed by the login item so the app starts in the menu bar only.
const HIDDEN_FLAG: &str = "--hidden";
/// Check for a newer version, install it and exit, without opening the app.
const UPDATE_FLAG: &str = "--update";

/// Forwards engine events to the windows and keeps the pill in step.
///
/// Called on engine threads, which may be holding engine state: anything
/// that asks the engine a question runs on another thread.
fn on_engine_event(app: &AppHandle, event: &Event) {
    match event {
        Event::Speech(speech) => {
            let _ = app.emit("speech", speech);
            match speech {
                SpeechEvent::Started { .. } => {
                    let app = app.clone();
                    std::thread::spawn(move || {
                        if app.state::<AppState>().session.settings().show_overlay {
                            windows::show_overlay(&app);
                        }
                    });
                }
                SpeechEvent::State { state: State::Idle } => {
                    let app = app.clone();
                    std::thread::spawn(move || speech::hide_overlay_when_done(&app));
                }
                _ => {}
            }
        }
        Event::Download(download) => {
            let _ = app.emit("download", download);
        }
        Event::Settings(settings) => {
            let _ = app.emit("settings", settings);
        }
        Event::Notice(message) => {
            let (app, message) = (app.clone(), message.clone());
            std::thread::spawn(move || speech::notice(&app, &message));
        }
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let paths = Paths::resolve();
    std::fs::create_dir_all(&paths.models_dir)?;

    let listener_handle = handle.clone();
    let session = Session::open(paths, Arc::new(SystemAudio), move |event| {
        // Events can arrive before the state below is registered.
        if listener_handle.try_state::<AppState>().is_some() {
            on_engine_event(&listener_handle, event);
        }
    });
    let settings = session.settings();
    let hidden = settings.start_hidden || std::env::args().any(|arg| arg == HIDDEN_FLAG);

    app.manage(shortcuts::Bound::default());
    app.manage(updates::Updates::default());
    app.manage(AppState {
        session: session.clone(),
        last_selection: Mutex::default(),
        notice_until: Mutex::default(),
    });

    if std::env::args().any(|arg| arg == UPDATE_FLAG) {
        #[cfg(target_os = "macos")]
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        updates::run_and_exit(&handle);
        return Ok(());
    }

    tray::create(&handle)?;
    windows::create_overlay(&handle)?;
    windows::create_main(&handle, !hidden)?;
    #[cfg(target_os = "macos")]
    if hidden {
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    shortcuts::register_all(&handle, &settings.shortcuts);
    session.preload();
    updates::start_automatic_checks(&handle);
    Ok(())
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut builder = tauri::Builder::default();
    // A second launch only raises the window of the first. (Must be the
    // first plugin.) `--update` is a separate, short-lived run.
    if !std::env::args().any(|arg| arg == UPDATE_FLAG) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main(app)
        }));
    }
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    // The pill is a native panel on macOS.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    let app = builder
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(shortcuts::handle)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::engine,
            commands::shell_info,
            commands::set_shortcuts_suspended,
            commands::request_accessibility,
            updates::update_status,
            updates::check_for_updates,
            updates::install_update,
        ])
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing the settings window keeps the app alive in the menu bar.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == windows::MAIN {
                    api.prevent_close();
                    windows::hide_main(window.app_handle());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the Speachy application");

    app.run(|app, event| match event {
        // No window left is not a reason to quit: only "Quit" exits.
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::Exit => {
            // A model must not be torn down under a thread that is still
            // synthesizing with it.
            if let Some(state) = app.try_state::<AppState>() {
                if !state.session.shutdown(Duration::from_secs(10)) {
                    log::warn!("the engine did not shut down in time");
                }
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => windows::show_main(app),
        _ => {}
    });
}
