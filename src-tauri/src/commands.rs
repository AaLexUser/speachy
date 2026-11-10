//! Commands the windows can invoke.

use crate::state::{AppState, Download};
use crate::{selection, shortcuts, speech};
use serde::Serialize;
use speachy_core::catalog::{self, ModelInfo};
use speachy_core::models::{is_cancelled, Progress};
use speachy_core::pipeline::State as SpeechState;
use speachy_core::settings::{Settings, VoiceRef};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

#[derive(Serialize)]
pub struct ModelView {
    #[serde(flatten)]
    info: &'static ModelInfo,
    installed: bool,
    /// Bytes of an interrupted download that will be resumed.
    partial_bytes: u64,
    /// Set while the model is being installed.
    progress: Option<Progress>,
    installing: bool,
}

/// Everything the settings window needs to render.
#[derive(Serialize)]
pub struct Snapshot {
    models: Vec<ModelView>,
    settings: Settings,
    speech_state: SpeechState,
    /// Display names of language codes.
    languages: BTreeMap<String, &'static str>,
    accessibility: bool,
    platform: &'static str,
    version: &'static str,
}

fn snapshot(state: &AppState) -> Snapshot {
    let downloads = state.downloads.lock().unwrap();
    let models = catalog::catalog()
        .iter()
        .map(|info| ModelView {
            info,
            installed: state.models.is_installed(&info.id),
            partial_bytes: state.models.partial_bytes(info),
            progress: downloads.get(&info.id).and_then(|d| d.progress),
            installing: downloads.contains_key(&info.id),
        })
        .collect();
    let languages = catalog::catalog()
        .iter()
        .flat_map(|m| m.languages.iter())
        .map(|code| (code.clone(), catalog::language_name(code)))
        .collect();
    Snapshot {
        models,
        settings: state.settings.read().unwrap().clone(),
        speech_state: state.speaker.state(),
        languages,
        accessibility: selection::accessibility_granted(),
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
pub fn get_snapshot(state: State<AppState>) -> Snapshot {
    snapshot(&state)
}

#[derive(Clone, Serialize)]
struct DownloadEvent {
    id: String,
    progress: Option<Progress>,
    finished: bool,
    error: Option<String>,
}

#[tauri::command]
pub fn install_model(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    let model = catalog::find(&id).ok_or_else(|| format!("unknown model {id}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut downloads = state.downloads.lock().unwrap();
        if downloads.contains_key(&id) {
            return Ok(());
        }
        downloads.insert(
            id.clone(),
            Download {
                cancel: cancel.clone(),
                progress: None,
            },
        );
    }
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let last_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let result = state.models.install(model, &cancel, &|progress| {
            if let Some(download) = state.downloads.lock().unwrap().get_mut(&model.id) {
                download.progress = Some(progress);
            }
            let mut last = last_emit.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(100) {
                *last = Instant::now();
                let _ = app.emit(
                    "download",
                    DownloadEvent {
                        id: model.id.clone(),
                        progress: Some(progress),
                        finished: false,
                        error: None,
                    },
                );
            }
        });
        state.downloads.lock().unwrap().remove(&model.id);
        let error = match &result {
            Ok(()) => None,
            Err(err) if is_cancelled(err) => None,
            Err(err) => Some(format!("{err:#}")),
        };
        if result.is_ok() {
            // The first model installed becomes the default voice.
            let mut settings = state.settings.write().unwrap();
            let has_default = settings
                .default_voice
                .as_ref()
                .is_some_and(|v| state.models.is_installed(&v.model_id));
            if !has_default {
                settings.default_voice = Some(VoiceRef {
                    model_id: model.id.clone(),
                    voice_id: model.default_voice.clone(),
                });
                if let Err(err) = settings.save(&state.settings_path) {
                    log::warn!("could not save settings: {err}");
                }
            }
            drop(settings);
            speech::preload(&app);
        }
        let _ = app.emit(
            "download",
            DownloadEvent {
                id: model.id.clone(),
                progress: None,
                finished: true,
                error,
            },
        );
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_install(state: State<AppState>, id: String) {
    if let Some(download) = state.downloads.lock().unwrap().get(&id) {
        download.cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn remove_model(state: State<AppState>, id: String) -> Result<Snapshot, String> {
    if state.downloads.lock().unwrap().contains_key(&id) {
        return Err("this model is being installed".into());
    }
    // Whatever is being spoken may be using the model's files.
    state.speaker.stop();
    state.engines.unload_model(&id);
    state.models.remove(&id).map_err(|err| format!("{err:#}"))?;
    {
        let mut settings = state.settings.write().unwrap();
        if settings
            .default_voice
            .as_ref()
            .is_some_and(|v| v.model_id == id)
        {
            settings.default_voice = state.models.installed().first().map(|m| VoiceRef {
                model_id: m.id.clone(),
                voice_id: m.default_voice.clone(),
            });
        }
        settings.language_voices.retain(|_, v| v.model_id != id);
        settings
            .save(&state.settings_path)
            .map_err(|err| err.to_string())?;
    }
    Ok(snapshot(&state))
}

#[derive(Serialize)]
pub struct SettingsUpdate {
    settings: Settings,
    /// Problems registering shortcuts; the rest of the update still applied.
    shortcut_errors: Vec<String>,
}

/// Returns `old` with the fields of `patch` replaced. `shortcuts` merges
/// field by field; every other field is replaced whole.
fn merge_settings(old: &Settings, patch: serde_json::Value) -> Result<Settings, String> {
    let serde_json::Value::Object(patch) = patch else {
        return Err("settings update must be an object".into());
    };
    let mut merged = serde_json::to_value(old).map_err(|err| err.to_string())?;
    for (key, value) in patch {
        match (merged.get_mut(&key), value) {
            (Some(serde_json::Value::Object(target)), serde_json::Value::Object(fields))
                if key == "shortcuts" =>
            {
                target.extend(fields)
            }
            (Some(slot), value) => *slot = value,
            (None, _) => return Err(format!("unknown setting {key}")),
        }
    }
    let mut new: Settings =
        serde_json::from_value(merged).map_err(|err| format!("invalid settings: {err}"))?;
    new.sanitize();
    Ok(new)
}

/// Applies a partial update: `patch` holds only the fields that changed.
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<AppState>,
    patch: serde_json::Value,
) -> Result<SettingsUpdate, String> {
    let old = state.settings.read().unwrap().clone();
    let new = merge_settings(&old, patch)?;
    for keys in [
        &new.shortcuts.speak_selection,
        &new.shortcuts.pause_resume,
        &new.shortcuts.stop,
    ] {
        shortcuts::validate(keys)?;
    }
    new.save(&state.settings_path)
        .map_err(|err| format!("could not save settings: {err}"))?;
    *state.settings.write().unwrap() = new.clone();

    let mut shortcut_errors = Vec::new();
    if new.shortcuts != old.shortcuts {
        shortcut_errors = shortcuts::register_all(&app, &new.shortcuts);
    }
    if new.volume != old.volume {
        state.speaker.set_volume(new.volume);
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
    if new.default_voice != old.default_voice || (new.preload_model && !old.preload_model) {
        speech::preload(&app);
    }
    let _ = app.emit("settings", &new);
    Ok(SettingsUpdate {
        settings: new,
        shortcut_errors,
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
        let shortcuts = state.settings.read().unwrap().shortcuts.clone();
        shortcuts::register_all(&app, &shortcuts);
    }
}

#[tauri::command]
pub fn speak_text(
    app: AppHandle,
    text: String,
    voice: Option<VoiceRef>,
    lang: Option<String>,
) -> Result<(), String> {
    speech::speak(&app, &text, voice, lang)
}

#[tauri::command]
pub fn stop(state: State<AppState>) {
    state.speaker.stop();
}

#[tauri::command]
pub fn toggle_pause(state: State<AppState>) {
    state.speaker.toggle_pause();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_only_the_given_fields() {
        let old = Settings::default();
        let new = merge_settings(
            &old,
            json!({
                "speed": 1.5,
                "default_voice": { "model_id": "supertonic-3", "voice_id": "M2" },
                "shortcuts": { "stop": "F9" },
            }),
        )
        .unwrap();
        assert_eq!(new.speed, 1.5);
        assert_eq!(new.default_voice.as_ref().unwrap().voice_id, "M2");
        // Shortcuts not mentioned keep their value.
        assert_eq!(new.shortcuts.stop, "F9");
        assert_eq!(new.shortcuts.speak_selection, old.shortcuts.speak_selection);
        assert_eq!(new.volume, old.volume);

        let cleared = merge_settings(&new, json!({ "default_voice": null })).unwrap();
        assert_eq!(cleared.default_voice, None);
        assert_eq!(cleared.speed, 1.5);
    }

    #[test]
    fn rejects_bad_updates_and_clamps_values() {
        let old = Settings::default();
        assert!(merge_settings(&old, json!({ "no_such_setting": 1 })).is_err());
        assert!(merge_settings(&old, json!({ "speed": "fast" })).is_err());
        assert!(merge_settings(&old, json!([1, 2])).is_err());
        assert_eq!(
            merge_settings(&old, json!({ "speed": 99 })).unwrap().speed,
            2.5
        );
        assert_eq!(
            merge_settings(&old, json!({ "language_voices": {} })).unwrap(),
            old
        );
    }
}
