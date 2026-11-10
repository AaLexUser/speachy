//! The session as front ends use it: commands in, events out. Runs with a
//! fake synthesizer and a virtual sound card, so no model files are needed.

use serde_json::{json, Value};
use speachy_core::audio::CaptureAudio;
use speachy_core::engine::{Audio, EngineKey, EngineProvider, SynthRequest, Synthesizer};
use speachy_core::pipeline::{Outcome, SpeechEvent};
use speachy_core::session::{Command, Event, Paths, Session};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Speaks every request as a short tone and remembers what it was asked.
#[derive(Default)]
struct FakeEngines {
    requests: Mutex<Vec<(String, String, i32)>>, // (text, lang, sid)
    unloaded: Mutex<Vec<String>>,
}

impl Synthesizer for FakeEngines {
    fn synthesize(&self, req: &SynthRequest, _: &Arc<AtomicBool>) -> anyhow::Result<Option<Audio>> {
        let request = (req.text.to_string(), req.lang.to_string(), req.sid);
        self.requests.lock().unwrap().push(request);
        Ok(Some(Audio {
            samples: vec![0.1; 100],
            sample_rate: 1000,
        }))
    }
}

struct Provider(Arc<FakeEngines>);

impl EngineProvider for Provider {
    fn engine(&self, _: &EngineKey) -> anyhow::Result<Arc<dyn Synthesizer>> {
        Ok(self.0.clone())
    }

    fn is_loaded(&self, _: &EngineKey) -> bool {
        true
    }

    fn unload_model(&self, model_id: &str) {
        self.0.unloaded.lock().unwrap().push(model_id.to_string());
    }
}

struct Rig {
    session: Session,
    engines: Arc<FakeEngines>,
    events: Receiver<Event>,
    _home: tempfile::TempDir,
}

/// A session in a fresh directory where `installed` models appear installed.
fn rig(installed: &[&str]) -> Rig {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    for id in installed {
        let dir = paths.models_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".speachy-model.json"), "{}").unwrap();
    }
    let engines = Arc::new(FakeEngines::default());
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let session = Session::with_engines(
        paths,
        Arc::new(Provider(engines.clone())),
        Arc::new(CaptureAudio::discarding(f32::INFINITY)),
        move |event| {
            let _ = tx.lock().unwrap().send(event.clone());
        },
    );
    Rig {
        session,
        engines,
        events,
        _home: home,
    }
}

impl Rig {
    fn run(&self, command: Value) -> Result<Value, String> {
        self.session.dispatch_json(command)
    }

    fn events_until(&self, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let event = self
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("no event");
            let hit = pred(&event);
            seen.push(event);
            if hit {
                return seen;
            }
        }
    }
}

const RU: &str =
    "Привет! Это проверка синтеза речи на русском языке, сегодня очень хорошая погода.";

#[test]
fn snapshot_reports_models_settings_and_paths() {
    let rig = rig(&["supertonic-3"]);
    let snapshot = rig.run(json!({ "cmd": "snapshot" })).unwrap();
    let models = snapshot["models"].as_array().unwrap();
    assert!(models.len() >= 5);
    let installed: Vec<&str> = models
        .iter()
        .filter(|m| m["installed"] == true)
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(installed, ["supertonic-3"]);
    assert_eq!(snapshot["speech_state"], "idle");
    assert_eq!(snapshot["settings"]["speed"], 1.0);
    assert_eq!(snapshot["languages"]["ru"], "Russian");
    assert!(snapshot["paths"]["models_dir"]
        .as_str()
        .unwrap()
        .ends_with("models"));
}

#[test]
fn speak_plans_synthesizes_and_reports_completion() {
    let rig = rig(&["supertonic-3"]);
    let result = rig
        .run(json!({ "cmd": "speak", "text": format!("Hello there, this is a test. {RU}") }))
        .unwrap();
    let session = result["session"].as_u64().unwrap();
    let events = rig.events_until(|e| matches!(e, Event::Speech(SpeechEvent::Finished { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Speech(SpeechEvent::Finished { session: s, outcome: Outcome::Completed, .. }) if *s == session
    )));
    rig.run(json!({ "cmd": "wait" })).unwrap();

    // Each language reached the synthesizer with its own language code.
    let requests = rig.engines.requests.lock().unwrap().clone();
    assert_eq!(requests.first().unwrap().1, "en");
    assert_eq!(requests.last().unwrap().1, "ru");
}

#[test]
fn plan_shows_chunks_languages_and_voices_without_speaking() {
    let rig = rig(&["supertonic-3", "kokoro-v1.0"]);
    let plan = rig.run(json!({ "cmd": "plan", "text": RU })).unwrap();
    let chunk = &plan["chunks"][0];
    assert_eq!(chunk["lang"], "ru");
    assert_eq!(chunk["model_id"], "supertonic-3");

    // A forced voice and language override detection.
    let forced = rig
        .run(json!({
            "cmd": "plan", "text": "Good morning to you all.", "lang": "en",
            "voice": { "model_id": "kokoro-v1.0", "voice_id": "bm_george" },
        }))
        .unwrap();
    assert_eq!(forced["chunks"][0]["voice_id"], "bm_george");
    assert_eq!(forced["chunks"][0]["engine"]["variant"], "gb");
    assert!(
        rig.engines.requests.lock().unwrap().is_empty(),
        "planning must not synthesize"
    );
}

#[test]
fn settings_updates_persist_and_are_announced() {
    let rig = rig(&["supertonic-3"]);
    let result = rig
        .run(json!({ "cmd": "update_settings", "patch": { "speed": 1.5, "shortcuts": { "stop": "F9" } } }))
        .unwrap();
    assert_eq!(result["settings"]["speed"], 1.5);
    rig.events_until(|e| matches!(e, Event::Settings(s) if s.speed == 1.5));

    // A second session on the same directory sees the saved settings.
    let reopened = Session::with_engines(
        rig.session.paths().clone(),
        Arc::new(Provider(rig.engines.clone())),
        Arc::new(CaptureAudio::discarding(f32::INFINITY)),
        |_| {},
    );
    assert_eq!(reopened.settings().speed, 1.5);
    assert_eq!(reopened.settings().shortcuts.stop, "F9");

    let err = rig
        .run(json!({ "cmd": "update_settings", "patch": { "nope": 1 } }))
        .unwrap_err();
    assert!(err.contains("unknown setting"), "{err}");
}

#[test]
fn removing_a_model_releases_it_and_repoints_the_default_voice() {
    let rig = rig(&["supertonic-3", "kitten-nano-v0.2"]);
    rig.run(json!({ "cmd": "update_settings", "patch": {
        "default_voice": { "model_id": "supertonic-3", "voice_id": "M2" },
        "language_voices": { "ru": { "model_id": "supertonic-3", "voice_id": "F3" } },
    } }))
    .unwrap();
    rig.run(json!({ "cmd": "remove_model", "id": "supertonic-3" }))
        .unwrap();

    let settings = rig.session.settings();
    assert_eq!(settings.default_voice.unwrap().model_id, "kitten-nano-v0.2");
    assert!(settings.language_voices.is_empty());
    assert_eq!(*rig.engines.unloaded.lock().unwrap(), ["supertonic-3"]);
    assert_eq!(rig.session.installed().len(), 1);
    assert!(rig
        .run(json!({ "cmd": "remove_model", "id": "no-such-model" }))
        .is_err());
}

#[test]
fn errors_are_messages_not_panics() {
    let empty = rig(&[]);
    let err = empty
        .run(json!({ "cmd": "speak", "text": "Hello." }))
        .unwrap_err();
    assert!(err.contains("no voice model"), "{err}");

    let rig = rig(&["supertonic-3"]);
    assert!(rig
        .run(json!({ "cmd": "speak", "text": "  " }))
        .unwrap_err()
        .contains("no text"));
    assert!(rig
        .run(json!({ "cmd": "fly" }))
        .unwrap_err()
        .contains("invalid command"));
    assert!(rig
        .run(json!({ "cmd": "speak" }))
        .unwrap_err()
        .contains("invalid command"));
    // A misspelt argument is rejected rather than silently ignored.
    assert!(rig
        .run(json!({ "cmd": "speak", "text": "Hi.", "langauge": "en" }))
        .unwrap_err()
        .contains("invalid command"));
    assert!(rig
        .run(json!({ "cmd": "install_model", "id": "no-such-model" }))
        .is_err());
    assert_eq!(
        rig.run(json!({ "cmd": "stop" })).unwrap(),
        json!({ "stopped": false })
    );
    assert_eq!(
        rig.run(json!({ "cmd": "cancel_install", "id": "supertonic-3" }))
            .unwrap(),
        json!({ "cancelled": false })
    );
}

#[test]
fn detect_language_answers_with_or_without_models() {
    let rig = rig(&[]);
    let detected = rig
        .run(json!({ "cmd": "detect_language", "text": RU }))
        .unwrap();
    assert_eq!(
        detected,
        json!({ "language": "ru", "name": "Russian", "reliable": true })
    );
    let unsure = rig
        .run(json!({ "cmd": "detect_language", "text": "OK" }))
        .unwrap();
    assert_eq!(unsure["language"], Value::Null);
}

#[test]
fn commands_and_events_have_a_stable_json_shape() {
    let parsed: Command =
        serde_json::from_value(json!({ "cmd": "install_model", "id": "supertonic-3" })).unwrap();
    assert_eq!(
        parsed,
        Command::InstallModel {
            id: "supertonic-3".into(),
            wait: false
        }
    );
    let event = serde_json::to_value(Event::Notice("hello".into())).unwrap();
    assert_eq!(event, json!({ "event": "notice", "data": "hello" }));
    let event = serde_json::to_value(Event::Speech(SpeechEvent::ChunkPlaying {
        session: 3,
        index: 1,
    }))
    .unwrap();
    assert_eq!(
        event,
        json!({ "event": "speech", "data": { "type": "chunk_playing", "session": 3, "index": 1 } })
    );
}
