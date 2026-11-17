//! Global keyboard shortcuts.

use crate::speech;
use crate::state::AppState;
use speachy_core::settings::Shortcuts;
use std::str::FromStr;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SpeakSelection,
    PauseResume,
    Stop,
    Faster,
    Slower,
    ResetSpeed,
}

/// The shortcuts currently registered with the system.
#[derive(Default)]
pub struct Bound(Mutex<Vec<(Shortcut, Action)>>);

/// Registers the configured shortcuts, replacing the previous set. Returns
/// a message for each shortcut that could not be registered.
pub fn register_all(app: &AppHandle, shortcuts: &Shortcuts) -> Vec<String> {
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();
    let mut bound = Vec::new();
    let mut errors = Vec::new();
    let wanted = [
        (
            &shortcuts.speak_selection,
            Action::SpeakSelection,
            "Speak selection",
        ),
        (
            &shortcuts.pause_resume,
            Action::PauseResume,
            "Pause / resume",
        ),
        (&shortcuts.stop, Action::Stop, "Stop"),
        (&shortcuts.faster, Action::Faster, "Faster"),
        (&shortcuts.slower, Action::Slower, "Slower"),
        (&shortcuts.reset_speed, Action::ResetSpeed, "Reset speed"),
    ];
    for (keys, action, label) in wanted {
        if keys.trim().is_empty() {
            continue;
        }
        let shortcut = match Shortcut::from_str(keys) {
            Ok(shortcut) => shortcut,
            Err(err) => {
                errors.push(format!(
                    "{label}: \"{keys}\" is not a valid shortcut ({err})"
                ));
                continue;
            }
        };
        if bound.iter().any(|(other, _)| *other == shortcut) {
            errors.push(format!("{label}: {keys} is already used by another action"));
            continue;
        }
        match manager.register(shortcut) {
            Ok(()) => bound.push((shortcut, action)),
            Err(err) => errors.push(format!("{label}: could not register {keys} ({err})")),
        }
    }
    for error in &errors {
        log::warn!("{error}");
    }
    *app.state::<Bound>().0.lock().unwrap() = bound;
    errors
}

/// Checks that `keys` parses as a shortcut.
pub fn validate(keys: &str) -> Result<(), String> {
    if keys.trim().is_empty() {
        return Ok(());
    }
    Shortcut::from_str(keys)
        .map(|_| ())
        .map_err(|err| format!("\"{keys}\" is not a valid shortcut ({err})"))
}

pub fn handle(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let action = app
        .state::<Bound>()
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|(s, _)| s == shortcut)
        .map(|(_, a)| *a);
    let Some(action) = action else { return };
    let app = app.clone();
    // Reading the selection can take a few hundred milliseconds: keep it
    // off the event loop.
    std::thread::spawn(move || run(&app, action));
}

pub fn run(app: &AppHandle, action: Action) {
    let session = &app.state::<AppState>().session;
    match action {
        Action::SpeakSelection => speech::speak_selection(app),
        Action::PauseResume => session.toggle_pause(),
        Action::Stop => {
            session.stop();
        }
        // The engine announces the new speed itself.
        Action::Faster => report(app, session.change_speed(0.1)),
        Action::Slower => report(app, session.change_speed(-0.1)),
        Action::ResetSpeed => report(app, session.reset_speed()),
    }
}

fn report<T>(app: &AppHandle, result: Result<T, String>) {
    if let Err(message) = result {
        speech::notice(app, &message);
    }
}
