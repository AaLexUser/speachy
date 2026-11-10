//! The engine's front door.
//!
//! A [`Session`] owns everything that is not user interface: settings, the
//! installed models, loaded engines and the speech pipeline. Every front
//! end drives it the same way — the desktop app, the command-line tool and
//! the JSON control channel all send [`Command`]s and receive [`Event`]s —
//! so anything the app can do can be scripted and tested without a window.

use crate::audio::AudioBackend;
use crate::catalog::{self, ModelInfo};
use crate::engine::{EngineCache, EngineProvider};
use crate::lang::{self, Detected};
use crate::models::{is_cancelled, ModelManager, Progress};
use crate::pipeline::{Outcome, SpeakOptions, Speaker, SpeechEvent, State};
use crate::plan::{self, PlannedChunk};
use crate::settings::{Settings, VoiceRef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Where a session keeps its files.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Paths {
    pub models_dir: PathBuf,
    pub settings_file: PathBuf,
}

impl Paths {
    /// Directory name under the platform's data and config directories.
    pub const APP_ID: &'static str = "com.speachy.app";
    /// Environment variable that points every front end at one directory
    /// instead of the per-user locations.
    pub const HOME_VAR: &'static str = "SPEACHY_HOME";

    /// The locations the desktop app uses, unless `SPEACHY_HOME` is set.
    pub fn resolve() -> Self {
        if let Some(home) = std::env::var_os(Self::HOME_VAR).filter(|v| !v.is_empty()) {
            return Self::under(Path::new(&home));
        }
        let here = || PathBuf::from(".");
        let data = dirs::data_dir().unwrap_or_else(here).join(Self::APP_ID);
        let config = dirs::config_dir().unwrap_or_else(here).join(Self::APP_ID);
        Self {
            models_dir: data.join("models"),
            settings_file: config.join("settings.json"),
        }
    }

    /// Everything inside one directory.
    pub fn under(root: &Path) -> Self {
        Self {
            models_dir: root.join("models"),
            settings_file: root.join("settings.json"),
        }
    }
}

/// A catalog model together with its state on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct ModelView {
    #[serde(flatten)]
    pub info: &'static ModelInfo,
    pub installed: bool,
    /// Bytes of an interrupted download that will be resumed.
    pub partial_bytes: u64,
    pub installing: bool,
    /// Set while the model is being installed.
    pub progress: Option<Progress>,
}

/// The whole observable state of a session.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub models: Vec<ModelView>,
    pub settings: Settings,
    pub speech_state: State,
    /// Display names of language codes.
    pub languages: BTreeMap<String, &'static str>,
    pub paths: Paths,
    pub version: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DownloadEvent {
    pub id: String,
    pub progress: Option<Progress>,
    pub finished: bool,
    pub error: Option<String>,
}

/// Something that happened in the session. Serialized as
/// `{"event": "<kind>", "data": …}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Speech(SpeechEvent),
    Download(DownloadEvent),
    /// The settings changed; carries the new settings.
    Settings(Settings),
    /// A message for the user (for example why speech failed).
    Notice(String),
}

/// Everything a front end can ask of the engine. Deserialized from
/// `{"cmd": "<name>", …arguments}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// The full state: models, settings, speech state.
    Snapshot,
    /// Download and install a model. Returns at once unless `wait` is set;
    /// progress arrives as download events.
    InstallModel {
        id: String,
        #[serde(default)]
        wait: bool,
    },
    CancelInstall {
        id: String,
    },
    RemoveModel {
        id: String,
    },
    /// Change settings; `patch` holds only the fields to change.
    UpdateSettings {
        patch: Value,
    },
    /// Speak `text`. `voice` forces one voice; `lang` forces the language
    /// instead of detecting it.
    Speak {
        text: String,
        #[serde(default)]
        voice: Option<VoiceRef>,
        #[serde(default)]
        lang: Option<String>,
    },
    /// What [`Command::Speak`] would do, without speaking: the chunks and
    /// the language and voice chosen for each.
    Plan {
        text: String,
        #[serde(default)]
        voice: Option<VoiceRef>,
        #[serde(default)]
        lang: Option<String>,
    },
    /// The language the detector sees in `text`.
    DetectLanguage {
        text: String,
    },
    Stop,
    Pause,
    Resume,
    TogglePause,
    /// Block until nothing is being spoken.
    Wait,
    /// Load the default voice's model now.
    Preload,
}

struct Download {
    cancel: Arc<AtomicBool>,
    progress: Option<Progress>,
}

type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

struct Inner {
    paths: Paths,
    settings: RwLock<Settings>,
    models: ModelManager,
    engines: Arc<dyn EngineProvider>,
    speaker: Speaker,
    downloads: Mutex<HashMap<String, Download>>,
    listener: Listener,
    /// Model loads running in the background.
    loading: Mutex<usize>,
    loading_done: std::sync::Condvar,
}

/// The engine. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    /// Opens a session on `paths`, speaking through `audio`. `listener`
    /// receives every [`Event`]; it is called from engine threads and must
    /// not call back into the session synchronously.
    pub fn open(
        paths: Paths,
        audio: Arc<dyn AudioBackend>,
        listener: impl Fn(&Event) + Send + Sync + 'static,
    ) -> Self {
        let threads = Settings::load(&paths.settings_file).effective_threads();
        let engines = Arc::new(EngineCache::new(paths.models_dir.clone(), threads, 2));
        Self::with_engines(paths, engines, audio, listener)
    }

    /// Like [`Session::open`], with the synthesizers supplied by the caller
    /// (tests use this to run without model files).
    pub fn with_engines(
        paths: Paths,
        engines: Arc<dyn EngineProvider>,
        audio: Arc<dyn AudioBackend>,
        listener: impl Fn(&Event) + Send + Sync + 'static,
    ) -> Self {
        let listener: Listener = Arc::new(listener);
        let speech_listener = listener.clone();
        let speaker = Speaker::new(engines.clone(), audio, move |event| {
            speech_listener(&Event::Speech(event.clone()));
            if let SpeechEvent::Finished {
                outcome: Outcome::Failed,
                message,
                ..
            } = event
            {
                let message = message.clone().unwrap_or_else(|| "Speech failed".into());
                speech_listener(&Event::Notice(message));
            }
        });
        Self {
            inner: Arc::new(Inner {
                settings: RwLock::new(Settings::load(&paths.settings_file)),
                models: ModelManager::new(paths.models_dir.clone()),
                paths,
                engines,
                speaker,
                downloads: Mutex::default(),
                listener,
                loading: Mutex::new(0),
                loading_done: std::sync::Condvar::new(),
            }),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.read().unwrap().clone()
    }

    pub fn installed(&self) -> Vec<&'static ModelInfo> {
        self.inner.models.installed()
    }

    pub fn speech_state(&self) -> State {
        self.inner.speaker.state()
    }

    pub fn is_speaking(&self) -> bool {
        self.inner.speaker.is_active()
    }

    fn emit(&self, event: Event) {
        (self.inner.listener)(&event);
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = &self.inner;
        let downloads = inner.downloads.lock().unwrap();
        let models = catalog::catalog()
            .iter()
            .map(|info| ModelView {
                info,
                installed: inner.models.is_installed(&info.id),
                partial_bytes: inner.models.partial_bytes(info),
                installing: downloads.contains_key(&info.id),
                progress: downloads.get(&info.id).and_then(|d| d.progress),
            })
            .collect();
        let languages = catalog::catalog()
            .iter()
            .flat_map(|m| m.languages.iter())
            .map(|code| (code.clone(), catalog::language_name(code)))
            .collect();
        Snapshot {
            models,
            settings: self.settings(),
            speech_state: inner.speaker.state(),
            languages,
            paths: inner.paths.clone(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// Installs a model, in the background unless `wait` is set. Progress
    /// and the outcome are reported as [`Event::Download`]; with `wait`
    /// the outcome is also returned.
    pub fn install_model(&self, id: &str, wait: bool) -> Result<(), String> {
        let model = catalog::find(id).ok_or_else(|| format!("unknown model {id}"))?;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut downloads = self.inner.downloads.lock().unwrap();
            if downloads.contains_key(id) {
                return if wait {
                    Err(format!("{} is already being installed", model.name))
                } else {
                    Ok(())
                };
            }
            downloads.insert(
                id.to_string(),
                Download {
                    cancel: cancel.clone(),
                    progress: None,
                },
            );
        }
        let session = self.clone();
        let run = move || session.run_install(model, &cancel);
        if wait {
            run()
        } else {
            std::thread::Builder::new()
                .name("speachy-install".into())
                .spawn(run)
                .map_err(|err| err.to_string())?;
            Ok(())
        }
    }

    fn run_install(&self, model: &'static ModelInfo, cancel: &AtomicBool) -> Result<(), String> {
        let inner = &self.inner;
        let event = |progress, finished, error| {
            Event::Download(DownloadEvent {
                id: model.id.clone(),
                progress,
                finished,
                error,
            })
        };
        let last_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let result = inner.models.install(model, cancel, &|progress| {
            if let Some(download) = inner.downloads.lock().unwrap().get_mut(&model.id) {
                download.progress = Some(progress);
            }
            let mut last = last_emit.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(100) {
                *last = Instant::now();
                self.emit(event(Some(progress), false, None));
            }
        });
        inner.downloads.lock().unwrap().remove(&model.id);
        let outcome = match result {
            Ok(()) => Ok(()),
            Err(err) if is_cancelled(&err) => Err("the download was cancelled".to_string()),
            Err(err) => Err(format!("{err:#}")),
        };
        if outcome.is_ok() {
            // The first model installed becomes the default voice.
            let changed = {
                let mut settings = inner.settings.write().unwrap();
                let has_default = settings
                    .default_voice
                    .as_ref()
                    .is_some_and(|v| inner.models.is_installed(&v.model_id));
                if has_default {
                    None
                } else {
                    settings.default_voice = Some(VoiceRef {
                        model_id: model.id.clone(),
                        voice_id: model.default_voice.clone(),
                    });
                    if let Err(err) = settings.save(&inner.paths.settings_file) {
                        log::warn!("could not save settings: {err}");
                    }
                    Some(settings.clone())
                }
            };
            if let Some(settings) = changed {
                self.emit(Event::Settings(settings));
            }
            self.preload();
        }
        // A cancellation is the user's doing, not an error to report.
        let error = outcome
            .clone()
            .err()
            .filter(|_| !cancel.load(Ordering::Relaxed));
        self.emit(event(None, true, error));
        outcome
    }

    /// Asks a running install to stop. Returns whether one was running.
    pub fn cancel_install(&self, id: &str) -> bool {
        match self.inner.downloads.lock().unwrap().get(id) {
            Some(download) => {
                download.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn remove_model(&self, id: &str) -> Result<(), String> {
        let inner = &self.inner;
        if inner.downloads.lock().unwrap().contains_key(id) {
            return Err("this model is being installed".into());
        }
        // Whatever is being spoken may be using the model's files.
        inner.speaker.stop();
        inner.engines.unload_model(id);
        inner.models.remove(id).map_err(|err| format!("{err:#}"))?;
        let settings = {
            let mut settings = inner.settings.write().unwrap();
            if settings
                .default_voice
                .as_ref()
                .is_some_and(|v| v.model_id == id)
            {
                settings.default_voice = inner.models.installed().first().map(|m| VoiceRef {
                    model_id: m.id.clone(),
                    voice_id: m.default_voice.clone(),
                });
            }
            settings.language_voices.retain(|_, v| v.model_id != id);
            settings
                .save(&inner.paths.settings_file)
                .map_err(|err| format!("could not save settings: {err}"))?;
            settings.clone()
        };
        self.emit(Event::Settings(settings));
        Ok(())
    }

    /// Applies a partial settings update (see [`Settings::merged`]), saves
    /// it and returns the new settings.
    pub fn update_settings(&self, patch: Value) -> Result<Settings, String> {
        let inner = &self.inner;
        let (old, new) = {
            let mut settings = inner.settings.write().unwrap();
            let new = settings.merged(patch)?;
            new.save(&inner.paths.settings_file)
                .map_err(|err| format!("could not save settings: {err}"))?;
            (std::mem::replace(&mut *settings, new.clone()), new)
        };
        if new.volume != old.volume {
            inner.speaker.set_volume(new.volume);
        }
        if new.default_voice != old.default_voice || (new.preload_model && !old.preload_model) {
            self.preload();
        }
        self.emit(Event::Settings(new.clone()));
        Ok(new)
    }

    /// The settings a request is planned with: the user's, with the
    /// request's voice and language forced.
    fn settings_for(&self, voice: Option<VoiceRef>, lang: Option<String>) -> Settings {
        let mut settings = self.settings();
        if let Some(voice) = voice {
            settings.language_voices.clear();
            settings.default_voice = Some(voice);
        }
        if let Some(lang) = lang {
            settings.auto_detect_language = false;
            settings.language = lang;
        }
        settings
    }

    /// How `text` would be spoken: its chunks with the language and voice
    /// of each.
    pub fn plan(
        &self,
        text: &str,
        voice: Option<VoiceRef>,
        lang: Option<String>,
    ) -> Result<Vec<PlannedChunk>, String> {
        let settings = self.settings_for(voice, lang);
        plan::plan(text, &settings, &self.installed()).map_err(|err| err.to_string())
    }

    /// Starts speaking `text`, replacing whatever is being spoken. Returns
    /// the speech session id used in events.
    pub fn speak(
        &self,
        text: &str,
        voice: Option<VoiceRef>,
        lang: Option<String>,
    ) -> Result<u64, String> {
        let settings = self.settings_for(voice, lang);
        let chunks =
            plan::plan(text, &settings, &self.installed()).map_err(|err| err.to_string())?;
        Ok(self.inner.speaker.speak(
            chunks,
            SpeakOptions {
                speed: settings.speed,
                volume: settings.volume,
                lookahead: Duration::from_secs_f32(settings.lookahead_secs),
            },
        ))
    }

    /// The language of `text`, among the languages the installed models
    /// speak (all supported languages if nothing is installed).
    pub fn detect_language(&self, text: &str) -> Option<Detected> {
        let installed = self.installed();
        let models: Vec<&ModelInfo> = if installed.is_empty() {
            catalog::catalog().iter().collect()
        } else {
            installed
        };
        let mut candidates: Vec<String> = Vec::new();
        for code in models.iter().flat_map(|m| m.languages.iter()) {
            if !candidates.contains(code) {
                candidates.push(code.clone());
            }
        }
        lang::detect(text, &candidates)
    }

    /// The text being spoken, if any.
    pub fn current_text(&self) -> Option<String> {
        self.inner.speaker.current_text()
    }

    /// Stops speaking. Returns whether anything was being spoken.
    pub fn stop(&self) -> bool {
        self.inner.speaker.stop()
    }

    pub fn set_paused(&self, paused: bool) {
        self.inner.speaker.set_paused(paused);
    }

    pub fn toggle_pause(&self) {
        self.inner.speaker.toggle_pause();
    }

    /// Blocks until nothing is being spoken.
    pub fn wait_idle(&self) {
        self.inner.speaker.wait_idle();
    }

    /// Loads the model of the default voice in the background, so the next
    /// request does not wait for it. Does nothing if the user turned
    /// preloading off or no model is installed.
    pub fn preload(&self) {
        let session = self.clone();
        *self.inner.loading.lock().unwrap() += 1;
        let spawned = std::thread::Builder::new()
            .name("speachy-preload".into())
            .spawn(move || {
                if session.settings().preload_model {
                    if let Err(err) = session.load_default_voice() {
                        log::warn!("could not preload the default voice: {err}");
                    }
                }
                session.loading_finished();
            });
        if let Err(err) = spawned {
            log::warn!("could not start the preload thread: {err}");
            self.loading_finished();
        }
    }

    fn loading_finished(&self) {
        *self.inner.loading.lock().unwrap() -= 1;
        self.inner.loading_done.notify_all();
    }

    /// Stops speaking, cancels installs and waits, up to `timeout`, for the
    /// engine's threads to finish. Front ends call this before the process
    /// exits; exiting while a thread is inside a model aborts the process.
    /// Returns whether everything finished in time.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        for download in self.inner.downloads.lock().unwrap().values() {
            download.cancel.store(true, Ordering::Relaxed);
        }
        let spoke = self.inner.speaker.shutdown(timeout);
        let mut loading = self.inner.loading.lock().unwrap();
        while *loading > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            (loading, _) = self.inner.loading_done.wait_timeout(loading, left).unwrap();
        }
        spoke
    }

    /// Loads the default voice's model now. Returns the model's id, or
    /// `None` if no model is installed.
    pub fn load_default_voice(&self) -> Result<Option<String>, String> {
        // Plan a token text to find out which engine the default voice uses.
        let Ok(chunks) = plan::plan("preload", &self.settings(), &self.installed()) else {
            return Ok(None);
        };
        let Some(first) = chunks.first() else {
            return Ok(None);
        };
        self.inner
            .engines
            .engine(&first.engine)
            .map(|_| Some(first.model_id.clone()))
            .map_err(|err| format!("{err:#}"))
    }

    /// Runs a command and returns its result as JSON. This is the one entry
    /// point shared by every front end.
    pub fn dispatch(&self, command: Command) -> Result<Value, String> {
        let done = || Ok(json!({}));
        match command {
            Command::Snapshot => to_json(&self.snapshot()),
            Command::InstallModel { id, wait } => {
                self.install_model(&id, wait).and_then(|()| done())
            }
            Command::CancelInstall { id } => Ok(json!({ "cancelled": self.cancel_install(&id) })),
            Command::RemoveModel { id } => self.remove_model(&id).and_then(|()| done()),
            Command::UpdateSettings { patch } => {
                let settings = self.update_settings(patch)?;
                Ok(json!({ "settings": to_json(&settings)? }))
            }
            Command::Speak { text, voice, lang } => {
                let session = self.speak(&text, voice, lang)?;
                Ok(json!({ "session": session }))
            }
            Command::Plan { text, voice, lang } => {
                let chunks = self.plan(&text, voice, lang)?;
                Ok(json!({ "chunks": to_json(&chunks)? }))
            }
            Command::DetectLanguage { text } => {
                let detected = self.detect_language(&text);
                Ok(json!({
                    "language": detected.as_ref().map(|d| d.code),
                    "name": detected.as_ref().map(|d| catalog::language_name(d.code)),
                    "reliable": detected.as_ref().map(|d| d.reliable),
                }))
            }
            Command::Stop => Ok(json!({ "stopped": self.stop() })),
            Command::Pause => {
                self.set_paused(true);
                done()
            }
            Command::Resume => {
                self.set_paused(false);
                done()
            }
            Command::TogglePause => {
                self.toggle_pause();
                done()
            }
            Command::Wait => {
                self.wait_idle();
                done()
            }
            Command::Preload => Ok(json!({ "model": self.load_default_voice()? })),
        }
    }

    /// Parses a command from JSON (`{"cmd": "speak", "text": "…"}`) and
    /// runs it.
    pub fn dispatch_json(&self, command: Value) -> Result<Value, String> {
        let command: Command =
            serde_json::from_value(command).map_err(|err| format!("invalid command: {err}"))?;
        self.dispatch(command)
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}
