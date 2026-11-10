//! Glue between user actions and the speech pipeline.

use crate::selection;
use crate::state::AppState;
use speachy_core::pipeline::SpeakOptions;
use speachy_core::plan::{self, PlanError};
use speachy_core::settings::VoiceRef;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// A short message for the user, shown by the pill and the settings window.
pub fn notice(app: &AppHandle, message: &str) {
    log::info!("notice: {message}");
    let _ = app.emit("notice", message);
    let state = app.state::<AppState>();
    if !state.settings.read().unwrap().show_overlay {
        return;
    }
    // Surface the message on the pill for a moment, even when idle.
    crate::windows::show_overlay(app);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        if !app.state::<AppState>().speaker.is_active() {
            crate::windows::hide_overlay(&app);
        }
    });
}

/// Speaks `text`. With `voice`, that exact voice is used (for previews);
/// otherwise voices follow the user's settings and the detected language.
///
/// With `lang`, the text is read in that language instead of detecting it.
pub fn speak(app: &AppHandle, text: &str, voice: Option<VoiceRef>, lang: Option<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut settings = state.settings.read().unwrap().clone();
    if let Some(voice) = voice {
        settings.language_voices.clear();
        settings.default_voice = Some(voice);
    }
    if let Some(lang) = lang {
        settings.auto_detect_language = false;
        settings.language = lang;
    }
    let installed = state.models.installed();
    let chunks = plan::plan(text, &settings, &installed).map_err(|err| {
        if err == PlanError::NoModel {
            // Nothing to speak with yet: take the user to the downloads.
            crate::windows::show_main(app);
        }
        err.to_string()
    })?;
    state.speaker.speak(
        chunks,
        SpeakOptions {
            speed: settings.speed,
            volume: settings.volume,
            lookahead: Duration::from_secs_f32(settings.lookahead_secs),
        },
    );
    Ok(())
}

/// The shortcut action: speak the selection, or stop if there is nothing
/// new to say.
pub fn speak_selection(app: &AppHandle) {
    let state = app.state::<AppState>();
    let active = state.speaker.is_active();
    let selected = selection::selected_text();
    let mut last = state.last_selection.lock().unwrap();
    match selected {
        // Pressing the shortcut again on the same selection stops.
        Some(text) if active && *last == text => {
            state.speaker.stop();
        }
        Some(text) => {
            *last = text.clone();
            drop(last);
            if let Err(message) = speak(app, &text, None, None) {
                notice(app, &message);
            }
        }
        None if active => {
            state.speaker.stop();
        }
        None => {
            drop(last);
            if selection::accessibility_granted() {
                notice(app, "No text is selected");
            } else {
                notice(app, "Speachy needs Accessibility access to read the selection");
                crate::windows::show_main(app);
            }
        }
    }
}

pub fn speak_clipboard(app: &AppHandle) {
    match selection::clipboard_text() {
        Some(text) => {
            if let Err(message) = speak(app, &text, None, None) {
                notice(app, &message);
            }
        }
        None => notice(app, "The clipboard has no text"),
    }
}

/// Loads the model of the default voice in the background, so the first
/// request does not wait for it.
pub fn preload(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let settings = state.settings.read().unwrap().clone();
        if !settings.preload_model {
            return;
        }
        let installed = state.models.installed();
        // Plan a token text to find out which engine the default voice uses.
        let Ok(chunks) = plan::plan("preload", &settings, &installed) else { return };
        use speachy_core::engine::EngineProvider;
        if let Err(err) = state.engines.engine(&chunks[0].engine) {
            log::warn!("could not preload {}: {err:#}", chunks[0].model_id);
        }
    });
}
