//! The shell's side of speaking: reading the selection and telling the user
//! what happened. The speaking itself is the engine's.

use crate::selection;
use crate::state::AppState;
use speachy_core::pipeline::State;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// A short message for the user, shown by the pill and the settings window.
pub fn notice(app: &AppHandle, message: &str) {
    const SHOWN_FOR: Duration = Duration::from_millis(2500);
    log::info!("notice: {message}");
    let _ = app.emit("notice", message);
    let state = app.state::<AppState>();
    if !state.session.settings().show_overlay {
        return;
    }
    // Surface the message on the pill for a moment, even when idle.
    *state.notice_until.lock().unwrap() = Some(Instant::now() + SHOWN_FOR);
    crate::windows::show_overlay(app);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOWN_FOR);
        hide_overlay_when_done(&app);
    });
}

/// Hides the pill unless something is being spoken or a notice is still
/// meant to be on screen.
pub fn hide_overlay_when_done(app: &AppHandle) {
    let state = app.state::<AppState>();
    let notice_showing = state
        .notice_until
        .lock()
        .unwrap()
        .is_some_and(|until| Instant::now() < until);
    if !notice_showing && state.session.speech_state() == State::Idle {
        crate::windows::hide_overlay(app);
    }
}

/// Speaks `text` with the user's voices, reporting a failure as a notice.
fn speak(app: &AppHandle, text: &str) {
    let session = &app.state::<AppState>().session;
    if let Err(message) = session.speak(text, None, None) {
        if session.installed().is_empty() {
            // Nothing to speak with yet: take the user to the downloads.
            crate::windows::show_main(app);
        }
        notice(app, &message);
    }
}

/// The shortcut action: speak the selection, or stop if there is nothing
/// new to say.
pub fn speak_selection(app: &AppHandle) {
    let state = app.state::<AppState>();
    let active = state.session.is_speaking();
    let selected = selection::selected_text();
    let mut last = state.last_selection.lock().unwrap();
    match selected {
        // Pressing the shortcut again on the same selection stops.
        Some(text) if active && *last == text => {
            state.session.stop();
        }
        Some(text) => {
            *last = text.clone();
            drop(last);
            speak(app, &text);
        }
        None if active => {
            state.session.stop();
        }
        None => {
            drop(last);
            if selection::accessibility_granted() {
                notice(app, "No text is selected");
            } else {
                notice(
                    app,
                    "Speachy needs Accessibility access to read the selection",
                );
                crate::windows::show_main(app);
            }
        }
    }
}

pub fn speak_clipboard(app: &AppHandle) {
    match selection::clipboard_text() {
        Some(text) => speak(app, &text),
        None => notice(app, "The clipboard has no text"),
    }
}
