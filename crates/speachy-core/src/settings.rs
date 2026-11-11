//! User settings, persisted as JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A voice of a specific model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceRef {
    pub model_id: String,
    pub voice_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    /// Speak the current selection; pressed again with nothing new selected,
    /// stops.
    pub speak_selection: String,
    pub pause_resume: String,
    pub stop: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            speak_selection: "Ctrl+Alt+S".into(),
            pause_resume: "Ctrl+Alt+P".into(),
            stop: "Ctrl+Alt+X".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Voice used when no per-language choice applies. `None` until the
    /// user picks one; the first installed model's default voice is used.
    pub default_voice: Option<VoiceRef>,
    /// Voice per ISO 639-1 language, for detected languages.
    pub language_voices: BTreeMap<String, VoiceRef>,
    /// Detect the language of the text and pick a matching voice.
    pub auto_detect_language: bool,
    /// Language assumed when detection is off or inconclusive.
    pub language: String,
    /// Speech rate, 1.0 = the voice's natural pace.
    pub speed: f32,
    pub volume: f32,
    /// How many seconds of audio to synthesize ahead of playback.
    pub lookahead_secs: f32,
    /// Threads used for synthesis. 0 picks a default for the machine.
    pub threads: u32,
    /// Load the default voice's model at launch so the first shortcut press
    /// starts speaking without a model-load delay.
    pub preload_model: bool,
    /// Show the floating status pill while speaking.
    pub show_overlay: bool,
    pub launch_at_login: bool,
    /// Keep the settings window closed at launch (menu bar only).
    pub start_hidden: bool,
    /// Look for new versions of the app from time to time.
    pub check_for_updates: bool,
    pub shortcuts: Shortcuts,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_voice: None,
            language_voices: BTreeMap::new(),
            auto_detect_language: true,
            language: "en".into(),
            speed: 1.0,
            volume: 1.0,
            lookahead_secs: 20.0,
            threads: 0,
            preload_model: true,
            show_overlay: true,
            launch_at_login: false,
            start_hidden: false,
            check_for_updates: true,
            shortcuts: Shortcuts::default(),
        }
    }
}

impl Settings {
    /// Loads settings, falling back to defaults for a missing or corrupt
    /// file. Out-of-range values are clamped.
    pub fn load(path: &Path) -> Self {
        let mut settings: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| match serde_json::from_str(&text) {
                Ok(settings) => Some(settings),
                Err(err) => {
                    log::warn!("ignoring unreadable settings {}: {err}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        settings.sanitize();
        settings
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).expect("settings serialize");
        // Write-then-rename, so a crash mid-write cannot truncate the file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// Returns these settings with the fields of `patch` replaced.
    /// `shortcuts` merges field by field; every other field is replaced
    /// whole. Unknown fields and values of the wrong type are errors.
    pub fn merged(&self, patch: serde_json::Value) -> Result<Settings, String> {
        let serde_json::Value::Object(patch) = patch else {
            return Err("a settings update must be a JSON object".into());
        };
        let mut merged = serde_json::to_value(self).map_err(|err| err.to_string())?;
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

    pub fn sanitize(&mut self) {
        let finite = |v: f32, default: f32| if v.is_finite() { v } else { default };
        self.speed = finite(self.speed, 1.0).clamp(0.5, 2.5);
        self.volume = finite(self.volume, 1.0).clamp(0.0, 1.5);
        self.lookahead_secs = finite(self.lookahead_secs, 20.0).clamp(3.0, 120.0);
        self.threads = self.threads.min(32);
        if self.language.is_empty() {
            self.language = "en".into();
        }
    }

    /// Synthesis thread count, resolving 0 to a machine-appropriate default.
    pub fn effective_threads(&self) -> i32 {
        if self.threads > 0 {
            return self.threads as i32;
        }
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        // Leave headroom: synthesis runs while the user keeps working.
        (cores / 2).clamp(2, 6) as i32
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_partial_or_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let mut settings = Settings::default();
        settings.speed = 1.4;
        settings.default_voice = Some(VoiceRef {
            model_id: "supertonic-3".into(),
            voice_id: "M2".into(),
        });
        settings.language_voices.insert(
            "ru".into(),
            VoiceRef {
                model_id: "supertonic-3".into(),
                voice_id: "F3".into(),
            },
        );
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);

        // Unknown and missing fields are fine; bad values are clamped.
        std::fs::write(
            &path,
            r#"{"speed": 99, "future_field": true, "shortcuts": {"stop": "F9"}}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.speed, 2.5);
        assert_eq!(loaded.shortcuts.stop, "F9");
        assert_eq!(
            loaded.shortcuts.speak_selection,
            Shortcuts::default().speak_selection
        );

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn merging_replaces_only_the_given_fields() {
        use serde_json::json;
        let old = Settings::default();
        let new = old
            .merged(json!({
                "speed": 1.5,
                "default_voice": { "model_id": "supertonic-3", "voice_id": "M2" },
                "shortcuts": { "stop": "F9" },
            }))
            .unwrap();
        assert_eq!(new.speed, 1.5);
        assert_eq!(new.default_voice.as_ref().unwrap().voice_id, "M2");
        // Shortcuts not mentioned keep their value.
        assert_eq!(new.shortcuts.stop, "F9");
        assert_eq!(new.shortcuts.speak_selection, old.shortcuts.speak_selection);
        assert_eq!(new.volume, old.volume);

        let cleared = new.merged(json!({ "default_voice": null })).unwrap();
        assert_eq!(cleared.default_voice, None);
        assert_eq!(cleared.speed, 1.5);
    }

    #[test]
    fn merging_rejects_bad_updates_and_clamps_values() {
        use serde_json::json;
        let old = Settings::default();
        assert!(old.merged(json!({ "no_such_setting": 1 })).is_err());
        assert!(old.merged(json!({ "speed": "fast" })).is_err());
        assert!(old.merged(json!([1, 2])).is_err());
        assert_eq!(old.merged(json!({ "speed": 99 })).unwrap().speed, 2.5);
        assert_eq!(old.merged(json!({ "language_voices": {} })).unwrap(), old);
    }
}
