//! Commands the windows can invoke.
//!
//! Engine work goes through one command, [`engine`], which forwards the
//! same JSON commands the CLI and the control channel use. The others are
//! about the shell itself: permissions and shortcut recording.

use crate::state::AppState;
use crate::{selection, shortcuts};
use serde_json::{json, Value};
use speachy_core::session::Command;
use speachy_core::settings::Settings;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// Runs an engine command (`{"cmd": "speak", "text": "…"}`) and returns its
/// result. Runs off the main thread: some commands take a while.
#[tauri::command(async)]
pub fn engine(app: AppHandle, state: State<AppState>, command: Value) -> Result<Value, String> {
    let command: Command =
        serde_json::from_value(command).map_err(|err| format!("invalid command: {err}"))?;
    let session = &state.session;
    let Command::UpdateSettings { patch } = &command else {
        return session.dispatch(command);
    };
    // Settings reach beyond the engine: shortcuts and the login item belong
    // to the shell. Refuse shortcuts that cannot be parsed before saving.
    let old = session.settings();
    let new = old.merged(patch.clone())?;
    for keys in [
        &new.shortcuts.speak_selection,
        &new.shortcuts.pause_resume,
        &new.shortcuts.stop,
    ] {
        shortcuts::validate(keys)?;
    }
    let mut result = session.dispatch(command)?;
    let shortcut_errors = apply_settings(&app, &old, &session.settings());
    result["shortcut_errors"] = json!(shortcut_errors);
    Ok(result)
}

/// Brings the shell in line with changed settings. Returns problems
/// registering shortcuts.
fn apply_settings(app: &AppHandle, old: &Settings, new: &Settings) -> Vec<String> {
    let mut shortcut_errors = Vec::new();
    if new.shortcuts != old.shortcuts {
        shortcut_errors = shortcuts::register_all(app, &new.shortcuts);
    }
    if new.launch_at_login != old.launch_at_login {
        let autostart = app.autolaunch();
        let result = if new.launch_at_login {
            autostart.enable()
        } else {
            autostart.disable()
        };
        if let Err(err) = result {
            log::warn!("could not change launch at login: {err}");
        }
    }
    shortcut_errors
}

/// What the window needs to know about the shell it runs in.
#[tauri::command]
pub fn shell_info() -> Value {
    json!({
        "platform": std::env::consts::OS,
        "accessibility": selection::accessibility_granted(),
    })
}

/// While the settings window records a new shortcut, the global ones are
/// released so the key press reaches the window.
#[tauri::command]
pub fn set_shortcuts_suspended(app: AppHandle, state: State<AppState>, suspended: bool) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if suspended {
        let _ = app.global_shortcut().unregister_all();
    } else {
        shortcuts::register_all(&app, &state.session.settings().shortcuts);
    }
}

/// Shows the system's Accessibility prompt (macOS). Returns whether access
/// is already granted.
#[tauri::command]
pub fn request_accessibility(app: AppHandle) -> bool {
    let granted = selection::request_accessibility();
    if !granted && cfg!(target_os = "macos") {
        use tauri_plugin_opener::OpenerExt;
        let pane = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
        if let Err(err) = app.opener().open_url(pane, None::<&str>) {
            log::warn!("could not open System Settings: {err}");
        }
    }
    granted
}
