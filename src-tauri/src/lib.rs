//! Speachy: select text anywhere, press a shortcut, hear it read aloud by a
//! local TTS model.

mod commands;
mod selection;
mod shortcuts;
mod speech;
mod state;
mod tray;
mod windows;

use speachy_core::audio::SystemAudio;
use speachy_core::engine::EngineCache;
use speachy_core::models::ModelManager;
use speachy_core::pipeline::{Outcome, Speaker, SpeechEvent, State};
use speachy_core::settings::Settings;
use state::AppState;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

/// Passed by the login item so the app starts in the menu bar only.
const HIDDEN_FLAG: &str = "--hidden";

fn on_speech_event(app: &AppHandle, event: &SpeechEvent) {
    let _ = app.emit("speech", event);
    match event {
        SpeechEvent::Started { .. } => {
            let show = app
                .try_state::<AppState>()
                .is_some_and(|s| s.settings.read().unwrap().show_overlay);
            if show {
                windows::show_overlay(app);
            }
        }
        // The pipeline reports this while holding its own state, which the
        // check reads: run it on another thread.
        SpeechEvent::State { state: State::Idle } => {
            let app = app.clone();
            std::thread::spawn(move || speech::hide_overlay_when_done(&app));
        }
        SpeechEvent::Finished {
            outcome: Outcome::Failed,
            message,
            ..
        } => {
            speech::notice(app, message.as_deref().unwrap_or("Speech failed"));
        }
        _ => {}
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let data_dir = app.path().app_data_dir()?;
    let settings_path = app.path().app_config_dir()?.join("settings.json");
    let models_dir = data_dir.join("models");
    std::fs::create_dir_all(&models_dir)?;

    let settings = Settings::load(&settings_path);
    let engines = Arc::new(EngineCache::new(
        models_dir.clone(),
        settings.effective_threads(),
        2,
    ));
    let listener_handle = handle.clone();
    let speaker = Speaker::new(engines.clone(), Arc::new(SystemAudio), move |event| {
        on_speech_event(&listener_handle, event)
    });

    let hidden = settings.start_hidden || std::env::args().any(|arg| arg == HIDDEN_FLAG);
    let shortcuts = settings.shortcuts.clone();
    app.manage(shortcuts::Bound::default());
    app.manage(AppState {
        settings: RwLock::new(settings),
        settings_path,
        models: ModelManager::new(models_dir),
        engines,
        speaker,
        downloads: Mutex::default(),
        last_selection: Mutex::default(),
        notice_until: Mutex::default(),
    });

    tray::create(&handle)?;
    windows::create_overlay(&handle)?;
    windows::create_main(&handle, !hidden)?;
    #[cfg(target_os = "macos")]
    if hidden {
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    shortcuts::register_all(&handle, &shortcuts);
    speech::preload(&handle);
    Ok(())
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let app = tauri::Builder::default()
        // Must be the first plugin: a second launch only raises the window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main(app)
        }))
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
            commands::get_snapshot,
            commands::install_model,
            commands::cancel_install,
            commands::remove_model,
            commands::update_settings,
            commands::set_shortcuts_suspended,
            commands::speak_text,
            commands::stop,
            commands::toggle_pause,
            commands::request_accessibility,
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
            if let Some(state) = app.try_state::<AppState>() {
                state.speaker.stop();
                // Let the audio thread wind down before the process ends.
                std::thread::sleep(Duration::from_millis(30));
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => windows::show_main(app),
        _ => {}
    });
}
