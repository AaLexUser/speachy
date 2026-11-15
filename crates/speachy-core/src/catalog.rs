//! The models Speachy can speak with, and their voices.
//!
//! A model is data: what it is called, which languages and voices it has,
//! and its [`Runtime`] — where it runs. The models that come with Speachy
//! are built in; a user can add more, and add voices of their own to any
//! model that imitates a recording. A [`Catalog`] holds both.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

const RELEASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models";

/// Architecture of a model that runs through sherpa-onnx. Decides how
/// [`crate::engine`] wires the files up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Supertonic,
    Kokoro,
    Kitten,
    Pocket,
}

/// Where a model runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    /// Inside Speachy, through sherpa-onnx, from files Speachy downloads.
    Sherpa {
        family: Family,
        archive_url: String,
        /// Name of the single top-level directory inside the archive.
        archive_dir: String,
        #[serde(default)]
        extra_files: Vec<ExtraFile>,
    },
    /// In a speech server, on this computer or one it can reach.
    Server(Server),
}

/// A speech server that answers the OpenAI speech request
/// (`POST /v1/audio/speech`), as mlx-audio and many other local servers do.
/// The server holds the model; Speachy sends it one chunk of text at a
/// time and plays what comes back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    /// Address of the server, such as `http://127.0.0.1:8000`.
    pub endpoint: String,
    /// The model as the server names it.
    pub model: String,
    /// Name of the request field that carries the language, for servers
    /// that take one (`lang_code` for mlx-audio). Not sent when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_param: Option<String>,
    /// What to send for a language, where the server does not use ISO 639-1
    /// codes (mlx-audio's Kokoro wants `a` for American English). Languages
    /// not listed are sent as their code.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub language_values: BTreeMap<String, String>,
    /// Further fields to send with every request, as the server documents
    /// them.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    Female,
    Male,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceInfo {
    /// Stable id, unique within the model (e.g. `af_heart`). A server is
    /// sent this as the voice.
    pub id: String,
    /// Display name (e.g. `Heart`). The id, when not given.
    #[serde(default)]
    pub name: String,
    /// Speaker id passed to a sherpa-onnx model.
    #[serde(default)]
    pub sid: i32,
    #[serde(default)]
    pub gender: Option<Gender>,
    /// ISO 639-1 languages this voice can speak. A voice of a multilingual
    /// model lists every language of the model, which is also what an
    /// empty list means.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Accent / flavour shown next to the name (e.g. `US`, `British`).
    #[serde(default)]
    pub accent: Option<String>,
    /// How the engine must be loaded for this voice (Kokoro only: `us`,
    /// `gb` or `zh` pick the lexicons and text rules). Two voices with
    /// different variants need separately loaded engines.
    #[serde(default)]
    pub variant: Option<String>,
    /// For a model that speaks in the voice of a recording it is given:
    /// the recording of this voice. A file in the model's directory, or
    /// anywhere when the path is absolute.
    #[serde(default)]
    pub reference: Option<String>,
    /// What is said in the recording, for the models that need it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_text: Option<String>,
    /// Added by the user, not part of the model.
    #[serde(default, skip_deserializing)]
    pub custom: bool,
}

/// An extra file that lives outside the model archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub runtime: Runtime,
    #[serde(default)]
    pub description: String,
    /// Where the weights come from, shown in the UI.
    #[serde(default)]
    pub license: String,
    /// What Speachy downloads to install the model. Nothing for a model a
    /// server holds.
    #[serde(default)]
    pub size_bytes: u64,
    /// 0 when not known in advance.
    #[serde(default)]
    pub sample_rate: u32,
    /// ISO 639-1 codes.
    pub languages: Vec<String>,
    pub voices: Vec<VoiceInfo>,
    /// Voice used when the user has not picked one. The first voice, when
    /// not given.
    #[serde(default)]
    pub default_voice: String,
    #[serde(default)]
    pub recommended: bool,
    /// Rough relative synthesis speed, 1 (slow) to 3 (fast), for the UI.
    /// 0 when unknown.
    #[serde(default)]
    pub speed_tier: u8,
    /// Whether voices can be added from recordings.
    #[serde(default)]
    pub imitates_recordings: bool,
    /// Added by the user, not one of the models that come with Speachy.
    #[serde(default, skip_deserializing)]
    pub custom: bool,
}

impl ModelInfo {
    pub fn voice(&self, id: &str) -> Option<&VoiceInfo> {
        self.voices.iter().find(|v| v.id == id)
    }

    pub fn default_voice(&self) -> &VoiceInfo {
        self.voice(&self.default_voice).unwrap_or(&self.voices[0])
    }

    /// First voice that can speak `lang`, preferring `preferred` if it can.
    pub fn voice_for_language(&self, lang: &str, preferred: Option<&str>) -> Option<&VoiceInfo> {
        let speaks = |v: &&VoiceInfo| v.languages.iter().any(|l| l == lang);
        preferred
            .and_then(|id| self.voice(id))
            .filter(speaks)
            .or_else(|| Some(self.default_voice()).filter(speaks))
            .or_else(|| self.voices.iter().find(speaks))
    }

    /// The sherpa-onnx architecture, for a model that runs inside Speachy.
    pub fn family(&self) -> Option<Family> {
        match &self.runtime {
            Runtime::Sherpa { family, .. } => Some(*family),
            Runtime::Server(_) => None,
        }
    }

    /// Whether Speachy has to download files before the model can speak.
    pub fn needs_download(&self) -> bool {
        matches!(self.runtime, Runtime::Sherpa { .. })
    }

    /// Fills in what a definition may leave out and checks the rest, so
    /// the engine can rely on every model it is handed.
    fn checked(mut self) -> Result<Self, String> {
        let plain = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && !s.starts_with('.')
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !plain(&self.id) {
            return Err(format!(
                "a model id may hold letters, digits, '-', '_' and '.', not {:?}",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            self.name = self.id.clone();
        }
        if self.languages.is_empty() {
            return Err(format!("{}: no languages are given", self.id));
        }
        if self.voices.is_empty() {
            return Err(format!("{}: no voices are given", self.id));
        }
        let mut seen = std::collections::HashSet::new();
        for voice in &mut self.voices {
            if voice.id.trim().is_empty() {
                return Err(format!("{}: a voice has no id", self.id));
            }
            if !seen.insert(voice.id.clone()) {
                return Err(format!(
                    "{}: the voice {} is given twice",
                    self.id, voice.id
                ));
            }
            if voice.name.trim().is_empty() {
                voice.name = voice.id.clone();
            }
            if voice.languages.is_empty() {
                voice.languages = self.languages.clone();
            }
            if let Some(lang) = voice.languages.iter().find(|l| !self.languages.contains(l)) {
                return Err(format!(
                    "{}: the voice {} speaks {lang}, which the model does not list",
                    self.id, voice.id
                ));
            }
        }
        if self.voice(&self.default_voice).is_none() {
            self.default_voice = self.voices[0].id.clone();
        }
        match &mut self.runtime {
            Runtime::Sherpa {
                archive_url,
                archive_dir,
                extra_files,
                ..
            } => {
                if !archive_url.starts_with("https://") && !archive_url.starts_with("http://") {
                    return Err(format!("{}: the archive address is not a URL", self.id));
                }
                // Both end up as paths inside the models directory.
                if !plain(archive_dir) || extra_files.iter().any(|f| !plain(&f.filename)) {
                    return Err(format!(
                        "{}: a file name leads outside the model's directory",
                        self.id
                    ));
                }
            }
            Runtime::Server(server) => {
                let endpoint = server.endpoint.trim().trim_end_matches('/').to_string();
                if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                    return Err(format!(
                        "{}: the server address must start with http:// or https://",
                        self.id
                    ));
                }
                server.endpoint = endpoint;
                if server.model.trim().is_empty() {
                    return Err(format!("{}: the server's model is not named", self.id));
                }
            }
        }
        Ok(self)
    }
}

/// What the user added to the built-in models, as kept in the catalog file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Additions {
    /// Models, most recently added last.
    models: Vec<ModelInfo>,
    /// Voices added to models, by model id.
    voices: BTreeMap<String, Vec<VoiceInfo>>,
}

/// The models of a session: the built-in ones and the user's additions.
///
/// Models are handed out as snapshots: a model in hand does not change when
/// the catalog does.
pub struct Catalog {
    /// Where additions are kept; `None` keeps them in memory only.
    file: Option<PathBuf>,
    state: RwLock<State>,
}

struct State {
    additions: Additions,
    /// Built-in models, then the user's, each with the user's voices.
    models: Vec<Arc<ModelInfo>>,
}

impl State {
    fn new(additions: Additions) -> Self {
        let custom = additions.models.iter().cloned().map(|mut model| {
            model.custom = true;
            model
        });
        let models = builtin()
            .iter()
            .cloned()
            .chain(custom)
            .map(|mut model| {
                for voice in additions.voices.get(&model.id).into_iter().flatten() {
                    let mut voice = voice.clone();
                    voice.custom = true;
                    model.voices.push(voice);
                }
                Arc::new(model)
            })
            .collect();
        Self { additions, models }
    }
}

impl Catalog {
    /// The built-in models only; additions are not kept.
    pub fn builtin_only() -> Self {
        Self {
            file: None,
            state: RwLock::new(State::new(Additions::default())),
        }
    }

    /// The built-in models plus the additions kept in `file`. A file that
    /// is missing or cannot be read counts as no additions, and entries
    /// that no longer make sense are dropped: a bad file must not keep the
    /// engine from starting.
    pub fn open(file: &Path) -> Self {
        let mut additions: Additions = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| match serde_json::from_str(&text) {
                Ok(additions) => Some(additions),
                Err(err) => {
                    log::warn!("ignoring {}: {err}", file.display());
                    None
                }
            })
            .unwrap_or_default();
        let mut ids: Vec<String> = builtin().iter().map(|m| m.id.clone()).collect();
        additions.models = std::mem::take(&mut additions.models)
            .into_iter()
            .filter_map(|model| match model.checked() {
                Ok(model) if !ids.contains(&model.id) => {
                    ids.push(model.id.clone());
                    Some(model)
                }
                Ok(model) => {
                    log::warn!("ignoring a second model called {}", model.id);
                    None
                }
                Err(err) => {
                    log::warn!("ignoring a model in {}: {err}", file.display());
                    None
                }
            })
            .collect();
        additions.voices.retain(|id, _| ids.contains(id));
        Self {
            file: Some(file.to_path_buf()),
            state: RwLock::new(State::new(additions)),
        }
    }

    /// Every model, built-in ones first.
    pub fn models(&self) -> Vec<Arc<ModelInfo>> {
        self.state.read().unwrap().models.clone()
    }

    pub fn find(&self, id: &str) -> Option<Arc<ModelInfo>> {
        let state = self.state.read().unwrap();
        state.models.iter().find(|m| m.id == id).cloned()
    }

    /// Changes the additions, keeps them and rebuilds the models.
    fn change<T>(
        &self,
        change: impl FnOnce(&mut Additions) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.write().unwrap();
        let mut additions = state.additions.clone();
        let result = change(&mut additions)?;
        if let Some(file) = &self.file {
            let text = serde_json::to_string_pretty(&additions).map_err(|err| err.to_string())?;
            let saved = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(file, text));
            saved.map_err(|err| format!("could not save {}: {err}", file.display()))?;
        }
        *state = State::new(additions);
        Ok(result)
    }

    /// Adds a model, or replaces the user's model of the same id.
    pub fn add_model(&self, model: ModelInfo) -> Result<Arc<ModelInfo>, String> {
        let model = model.checked()?;
        if builtin().iter().any(|m| m.id == model.id) {
            return Err(format!(
                "{} is the name of a model that comes with Speachy",
                model.id
            ));
        }
        let id = model.id.clone();
        self.change(|additions| {
            additions.models.retain(|m| m.id != id);
            additions.models.push(model);
            Ok(())
        })?;
        self.find(&id).ok_or_else(|| format!("unknown model {id}"))
    }

    /// Forgets a model the user added, and the voices added to it. Returns
    /// whether there was one.
    pub fn remove_model(&self, id: &str) -> Result<bool, String> {
        self.change(|additions| {
            let before = additions.models.len();
            additions.models.retain(|m| m.id != id);
            let removed = additions.models.len() < before;
            if removed {
                additions.voices.remove(id);
            }
            Ok(removed)
        })
    }

    /// Adds a voice to a model, or replaces the user's voice of the same id.
    pub fn add_voice(&self, model_id: &str, mut voice: VoiceInfo) -> Result<(), String> {
        let model = self
            .find(model_id)
            .ok_or_else(|| format!("unknown model {model_id}"))?;
        if voice.id.trim().is_empty() {
            return Err("the voice has no id".into());
        }
        if model.voices.iter().any(|v| v.id == voice.id && !v.custom) {
            return Err(format!(
                "{} already has a voice called {}",
                model.name, voice.id
            ));
        }
        if voice.name.trim().is_empty() {
            voice.name = voice.id.clone();
        }
        if voice.languages.is_empty() {
            voice.languages = model.languages.clone();
        }
        if let Some(lang) = voice
            .languages
            .iter()
            .find(|l| !model.languages.contains(l))
        {
            return Err(format!("{} does not speak {lang}", model.name));
        }
        // Past every speaker the model itself has.
        voice.sid = model.voices.iter().map(|v| v.sid).max().unwrap_or(0) + 1;
        self.change(|additions| {
            let voices = additions.voices.entry(model_id.to_string()).or_default();
            voices.retain(|v| v.id != voice.id);
            voices.push(voice);
            Ok(())
        })
    }

    /// Removes a voice the user added. Returns the voice, if there was one.
    pub fn remove_voice(
        &self,
        model_id: &str,
        voice_id: &str,
    ) -> Result<Option<VoiceInfo>, String> {
        self.change(|additions| {
            let Some(voices) = additions.voices.get_mut(model_id) else {
                return Ok(None);
            };
            let found = voices.iter().position(|v| v.id == voice_id);
            Ok(found.map(|at| voices.remove(at)))
        })
    }
}

/// The models that come with Speachy.
pub fn builtin() -> &'static [ModelInfo] {
    static BUILTIN: OnceLock<Vec<ModelInfo>> = OnceLock::new();
    BUILTIN.get_or_init(build)
}

/// Languages Supertonic 3 accepts as its `lang` option.
pub const SUPERTONIC_LANGUAGES: &[&str] = &[
    "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu",
    "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi",
];

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn build() -> Vec<ModelInfo> {
    vec![
        supertonic(),
        kokoro(
            "kokoro-v1.0-int8",
            "Kokoro 82M v1.0 (compact)",
            "kokoro-int8-multi-lang-v1_0",
            132_353_410,
            "Expressive, natural English and Chinese voices. 8-bit quantized: a third of the \
             download, a little slower to synthesize.",
        ),
        kokoro(
            "kokoro-v1.0",
            "Kokoro 82M v1.0",
            "kokoro-multi-lang-v1_0",
            349_886_476,
            "Expressive, natural English and Chinese voices at full precision.",
        ),
        pocket(),
        kitten(
            "kitten-nano-v0.2",
            "KittenTTS Nano 0.2",
            "kitten-nano-en-v0_2-fp16",
            26_586_708,
            "Tiny 15M-parameter English model: the smallest download, and very fast. Quality \
             is a step below the larger models, and words missing from its dictionary \
             (unusual names, jargon) are skipped.",
        ),
        kitten(
            "kitten-mini-v0.1",
            "KittenTTS Mini 0.1",
            "kitten-mini-en-v0_1-fp16",
            156_630_103,
            "80M-parameter English model with the KittenTTS voice set. Words missing from \
             its dictionary are skipped.",
        ),
    ]
}

fn supertonic() -> ModelInfo {
    let languages = strings(SUPERTONIC_LANGUAGES);
    // voice.bin is built from the upstream style files in sorted order:
    // F1..F5 then M1..M5.
    let voices = (0..10)
        .map(|sid| {
            let (gender, prefix, n) = if sid < 5 {
                (Gender::Female, "F", sid + 1)
            } else {
                (Gender::Male, "M", sid - 4)
            };
            VoiceInfo {
                id: format!("{prefix}{n}"),
                name: format!("{prefix}{n}"),
                sid,
                gender: Some(gender),
                languages: languages.clone(),
                accent: None,
                variant: None,
                reference: None,
                reference_text: None,
                custom: false,
            }
        })
        .collect();
    let dir = "sherpa-onnx-supertonic-3-tts-int8-2026-05-11";
    ModelInfo {
        id: "supertonic-3".into(),
        name: "Supertonic 3".into(),
        runtime: Runtime::Sherpa {
            family: Family::Supertonic,
            archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
            archive_dir: dir.into(),
            extra_files: vec![],
        },
        description: "Fast multilingual model: 31 languages with the same ten voices, 44.1 kHz \
                      output. The best starting point, and the one to use with automatic \
                      language detection."
            .into(),
        license: "Supertone (OpenRAIL-M)".into(),
        size_bytes: 128_774_318,
        sample_rate: 44_100,
        languages,
        voices,
        default_voice: "F1".into(),
        recommended: true,
        speed_tier: 3,
        imitates_recordings: false,
        custom: false,
    }
}

fn kokoro(id: &str, name: &str, dir: &str, size: u64, description: &str) -> ModelInfo {
    // Speaker ids follow the `speaker_names` metadata of the ONNX model.
    // The Spanish, French, Hindi, Italian, Japanese and Portuguese voices
    // (ids 28-44, 53) are left out: sherpa-onnx ships no lexicon for them.
    const VOICES: &[(&str, i32)] = &[
        ("af_alloy", 0),
        ("af_aoede", 1),
        ("af_bella", 2),
        ("af_heart", 3),
        ("af_jessica", 4),
        ("af_kore", 5),
        ("af_nicole", 6),
        ("af_nova", 7),
        ("af_river", 8),
        ("af_sarah", 9),
        ("af_sky", 10),
        ("am_adam", 11),
        ("am_echo", 12),
        ("am_eric", 13),
        ("am_fenrir", 14),
        ("am_liam", 15),
        ("am_michael", 16),
        ("am_onyx", 17),
        ("am_puck", 18),
        ("am_santa", 19),
        ("bf_alice", 20),
        ("bf_emma", 21),
        ("bf_isabella", 22),
        ("bf_lily", 23),
        ("bm_daniel", 24),
        ("bm_fable", 25),
        ("bm_george", 26),
        ("bm_lewis", 27),
        ("zf_xiaobei", 45),
        ("zf_xiaoni", 46),
        ("zf_xiaoxiao", 47),
        ("zf_xiaoyi", 48),
        ("zm_yunjian", 49),
        ("zm_yunxi", 50),
        ("zm_yunxia", 51),
        ("zm_yunyang", 52),
    ];
    let voices = VOICES
        .iter()
        .map(|&(vid, sid)| {
            let (prefix, short) = vid.split_once('_').expect("voice id has a prefix");
            let gender = if prefix.ends_with('f') {
                Gender::Female
            } else {
                Gender::Male
            };
            let (languages, accent, variant) = match &prefix[..1] {
                "a" => (strings(&["en"]), "US", "us"),
                "b" => (strings(&["en"]), "British", "gb"),
                _ => (strings(&["zh", "en"]), "Mandarin", "zh"),
            };
            let mut name = short.to_string();
            name[..1].make_ascii_uppercase();
            VoiceInfo {
                id: vid.into(),
                name,
                sid,
                gender: Some(gender),
                languages,
                accent: Some(accent.into()),
                variant: Some(variant.into()),
                reference: None,
                reference_text: None,
                custom: false,
            }
        })
        .collect();
    ModelInfo {
        id: id.into(),
        name: name.into(),
        runtime: Runtime::Sherpa {
            family: Family::Kokoro,
            archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
            archive_dir: dir.into(),
            extra_files: vec![],
        },
        description: description.into(),
        license: "Apache-2.0".into(),
        size_bytes: size,
        sample_rate: 24_000,
        languages: strings(&["en", "zh"]),
        voices,
        default_voice: "af_heart".into(),
        recommended: false,
        speed_tier: 1,
        imitates_recordings: false,
        custom: false,
    }
}

fn kitten(id: &str, name: &str, dir: &str, size: u64, description: &str) -> ModelInfo {
    const VOICES: &[(&str, &str, Gender)] = &[
        ("expr-voice-2-m", "Voice 2", Gender::Male),
        ("expr-voice-2-f", "Voice 2", Gender::Female),
        ("expr-voice-3-m", "Voice 3", Gender::Male),
        ("expr-voice-3-f", "Voice 3", Gender::Female),
        ("expr-voice-4-m", "Voice 4", Gender::Male),
        ("expr-voice-4-f", "Voice 4", Gender::Female),
        ("expr-voice-5-m", "Voice 5", Gender::Male),
        ("expr-voice-5-f", "Voice 5", Gender::Female),
    ];
    let voices = VOICES
        .iter()
        .enumerate()
        .map(|(sid, &(vid, vname, gender))| VoiceInfo {
            id: vid.into(),
            name: vname.into(),
            sid: sid as i32,
            gender: Some(gender),
            languages: strings(&["en"]),
            accent: Some("US".into()),
            variant: None,
            reference: None,
            reference_text: None,
            custom: false,
        })
        .collect();
    let lexicon = ExtraFile {
        url: format!("{RELEASE}/lexicon-en-us.txt"),
        filename: "lexicon-en-us.txt".into(),
        size_bytes: 6_305_773,
    };
    let size = size + lexicon.size_bytes;
    ModelInfo {
        id: id.into(),
        name: name.into(),
        runtime: Runtime::Sherpa {
            family: Family::Kitten,
            archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
            archive_dir: dir.into(),
            extra_files: vec![lexicon],
        },
        description: description.into(),
        license: "Apache-2.0".into(),
        size_bytes: size,
        sample_rate: 24_000,
        languages: strings(&["en"]),
        voices,
        default_voice: "expr-voice-2-f".into(),
        recommended: false,
        speed_tier: 3,
        imitates_recordings: false,
        custom: false,
    }
}

fn pocket() -> ModelInfo {
    // Pocket TTS speaks in the voice of a recording. These are the ones
    // Kyutai offers with it that anyone may use: a voice actor's (CC BY
    // 4.0), donated voices (CC0) and speakers of the VCTK corpus (CC BY 4.0).
    const VOICES: &[(&str, Gender, &str, u64)] = &[
        ("Alba", Gender::Female, "alba-mackenna/casual.wav", 958_542),
        (
            "Fantine",
            Gender::Female,
            "vctk/p244_023_enhanced.wav",
            674_852,
        ),
        (
            "Eponine",
            Gender::Female,
            "vctk/p262_023_enhanced.wav",
            716_330,
        ),
        (
            "Azelma",
            Gender::Female,
            "vctk/p303_023_enhanced.wav",
            823_852,
        ),
        (
            "Javert",
            Gender::Male,
            "voice-donations/Butter.wav",
            480_044,
        ),
        (
            "Charles",
            Gender::Male,
            "vctk/p254_023_enhanced.wav",
            639_272,
        ),
        ("Paul", Gender::Male, "vctk/p259_023_enhanced.wav", 717_182),
        (
            "George",
            Gender::Male,
            "vctk/p315_023_enhanced.wav",
            642_692,
        ),
    ];
    // A fixed revision, so the recordings cannot change under an install.
    const RECORDINGS: &str = "https://huggingface.co/kyutai/tts-voices/resolve/\
                              323332d33f997de8394f24a193e1a76df720e01a";
    let mut voices = Vec::new();
    let mut recordings = Vec::new();
    for (sid, &(name, gender, path, size)) in VOICES.iter().enumerate() {
        let id = name.to_lowercase();
        let filename = format!("voice-{id}.wav");
        voices.push(VoiceInfo {
            id,
            name: name.into(),
            sid: sid as i32,
            gender: Some(gender),
            languages: strings(&["en"]),
            accent: None,
            variant: None,
            reference: Some(filename.clone()),
            reference_text: None,
            custom: false,
        });
        recordings.push(ExtraFile {
            url: format!("{RECORDINGS}/{path}"),
            filename,
            size_bytes: size,
        });
    }
    let dir = "sherpa-onnx-pocket-tts-int8-2026-01-26";
    let size = 98_336_520 + recordings.iter().map(|f| f.size_bytes).sum::<u64>();
    ModelInfo {
        id: "pocket-tts".into(),
        name: "Pocket TTS".into(),
        runtime: Runtime::Sherpa {
            family: Family::Pocket,
            archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
            archive_dir: dir.into(),
            extra_files: recordings,
        },
        description: "Kyutai's 100M-parameter English model. It imitates a recorded voice, \
                      so its voices sound like particular people, with natural intonation."
            .into(),
        license: "Kyutai (CC BY 4.0); voices CC BY 4.0 and CC0".into(),
        size_bytes: size,
        sample_rate: 24_000,
        languages: strings(&["en"]),
        voices,
        default_voice: "alba".into(),
        recommended: false,
        speed_tier: 2,
        imitates_recordings: true,
        custom: false,
    }
}

/// English display name of an ISO 639-1 code, for the UI.
pub fn language_name(code: &str) -> &'static str {
    match code {
        "ar" => "Arabic",
        "bg" => "Bulgarian",
        "cs" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "et" => "Estonian",
        "fi" => "Finnish",
        "fr" => "French",
        "hi" => "Hindi",
        "hr" => "Croatian",
        "hu" => "Hungarian",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "lt" => "Lithuanian",
        "lv" => "Latvian",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "sv" => "Swedish",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_defaults_exist() {
        let mut ids = HashSet::new();
        for m in builtin() {
            assert!(ids.insert(&m.id), "duplicate model id {}", m.id);
            // Every built-in model passes the checks a user's model must.
            assert_eq!(&m.clone().checked().unwrap(), m, "{}", m.id);
            assert!(
                m.voice(&m.default_voice).is_some(),
                "{}: bad default voice",
                m.id
            );
            let Runtime::Sherpa { extra_files, .. } = &m.runtime else {
                panic!("{} does not run inside Speachy", m.id);
            };
            let mut voice_ids = HashSet::new();
            let mut sids = HashSet::new();
            for v in &m.voices {
                assert!(
                    voice_ids.insert(&v.id),
                    "{}: duplicate voice {}",
                    m.id,
                    v.id
                );
                assert!(sids.insert(v.sid), "{}: duplicate sid {}", m.id, v.sid);
                // A voice that imitates a recording gets it with the model.
                assert_eq!(v.reference.is_some(), m.imitates_recordings);
                if let Some(recording) = &v.reference {
                    assert!(
                        extra_files.iter().any(|f| &f.filename == recording),
                        "{}: nothing downloads {recording}",
                        m.id
                    );
                }
                assert!(!v.languages.is_empty());
                for l in &v.languages {
                    assert!(
                        m.languages.contains(l),
                        "{}: {} speaks unlisted {l}",
                        m.id,
                        v.id
                    );
                    assert_ne!(language_name(l), "Unknown", "no display name for {l}");
                }
            }
        }
    }

    #[test]
    fn voice_for_language_prefers_the_requested_voice() {
        let catalog = Catalog::builtin_only();
        let kokoro = catalog.find("kokoro-v1.0").unwrap();
        assert_eq!(
            kokoro
                .voice_for_language("en", Some("bm_george"))
                .unwrap()
                .id,
            "bm_george"
        );
        // A US voice cannot read Chinese, so a Chinese voice is substituted.
        assert_eq!(
            kokoro
                .voice_for_language("zh", Some("af_heart"))
                .unwrap()
                .id,
            "zf_xiaobei"
        );
        assert!(kokoro.voice_for_language("ru", None).is_none());
        let supertonic = catalog.find("supertonic-3").unwrap();
        assert_eq!(
            supertonic.voice_for_language("ru", Some("M2")).unwrap().sid,
            6
        );
    }

    fn server_model(id: &str) -> ModelInfo {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "",
            "runtime": { "kind": "server", "endpoint": "http://127.0.0.1:8000/", "model": "org/model" },
            "languages": ["en", "ru"],
            "voices": [{ "id": "anna" }, { "id": "ivan", "name": "Ivan", "languages": ["ru"] }],
        }))
        .unwrap()
    }

    #[test]
    fn a_short_definition_is_filled_in() {
        let catalog = Catalog::builtin_only();
        let model = catalog.add_model(server_model("mine")).unwrap();
        assert!(model.custom);
        assert_eq!(model.name, "mine");
        assert_eq!(model.default_voice, "anna");
        assert_eq!(model.voices[0].name, "anna");
        assert_eq!(model.voices[0].languages, ["en", "ru"]);
        assert_eq!(model.voices[1].languages, ["ru"]);
        let Runtime::Server(server) = &model.runtime else {
            panic!("not a server model");
        };
        assert_eq!(server.endpoint, "http://127.0.0.1:8000");
        // It comes after the built-in models, and can be found.
        assert_eq!(catalog.models().last().unwrap().id, "mine");
        assert_eq!(catalog.models().len(), builtin().len() + 1);
    }

    #[test]
    fn definitions_that_make_no_sense_are_refused() {
        let catalog = Catalog::builtin_only();
        let refused = |change: fn(&mut ModelInfo)| {
            let mut model = server_model("mine");
            change(&mut model);
            catalog.add_model(model).unwrap_err()
        };
        assert!(refused(|m| m.id = "../escape".into()).contains("model id"));
        assert!(refused(|m| m.id = "supertonic-3".into()).contains("comes with Speachy"));
        assert!(refused(|m| m.languages.clear()).contains("languages"));
        assert!(refused(|m| m.voices.clear()).contains("voices"));
        assert!(refused(|m| m.voices[1].id = "anna".into()).contains("twice"));
        assert!(refused(|m| m.voices[1].languages = vec!["de".into()]).contains("de"));
        assert!(refused(|m| {
            m.runtime = Runtime::Server(Server {
                endpoint: "localhost:8000".into(),
                model: "m".into(),
                language_param: None,
                language_values: BTreeMap::new(),
                options: Map::new(),
            })
        })
        .contains("http://"));
        assert!(refused(|m| {
            m.runtime = Runtime::Sherpa {
                family: Family::Kokoro,
                archive_url: "https://example.com/a.tar.bz2".into(),
                archive_dir: "../../somewhere".into(),
                extra_files: vec![],
            }
        })
        .contains("outside"));
        assert_eq!(catalog.models().len(), builtin().len());
    }

    #[test]
    fn additions_are_kept_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join("models.json");
        let catalog = Catalog::open(&file);
        catalog.add_model(server_model("mine")).unwrap();
        let voice: VoiceInfo = serde_json::from_value(serde_json::json!({
            "id": "me", "name": "My voice", "reference": "/somewhere/me.wav"
        }))
        .unwrap();
        catalog.add_voice("pocket-tts", voice.clone()).unwrap();
        // A voice cannot take the place of one the model comes with.
        let mut taken = voice.clone();
        taken.id = "alba".into();
        assert!(catalog.add_voice("pocket-tts", taken).is_err());
        assert!(catalog.add_voice("nothing", voice).is_err());

        let reopened = Catalog::open(&file);
        assert_eq!(reopened.find("mine").unwrap().voices.len(), 2);
        let pocket = reopened.find("pocket-tts").unwrap();
        let mine = pocket.voice("me").unwrap();
        assert!(mine.custom && !pocket.custom);
        assert_eq!(mine.languages, ["en"]);
        // Its speaker id does not collide with the model's own voices.
        assert!(pocket.voices.iter().filter(|v| v.sid == mine.sid).count() == 1);

        assert_eq!(
            reopened
                .remove_voice("pocket-tts", "me")
                .unwrap()
                .unwrap()
                .id,
            "me"
        );
        assert!(reopened
            .remove_voice("pocket-tts", "alba")
            .unwrap()
            .is_none());
        assert!(reopened.remove_model("mine").unwrap());
        assert!(!reopened.remove_model("supertonic-3").unwrap());
        let last = Catalog::open(&file);
        assert_eq!(last.models().len(), builtin().len());
        assert!(last.find("pocket-tts").unwrap().voice("me").is_none());
    }

    #[test]
    fn a_damaged_file_does_not_stop_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("models.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(Catalog::open(&file).models().len(), builtin().len());
        // One good model, one that fails the checks, one that repeats.
        let good = serde_json::to_value(server_model("good")).unwrap();
        let mut bad = good.clone();
        bad["id"] = "bad id".into();
        let text = serde_json::json!({ "models": [good, bad, good] }).to_string();
        std::fs::write(&file, text).unwrap();
        let catalog = Catalog::open(&file);
        assert_eq!(catalog.models().len(), builtin().len() + 1);
        assert!(catalog.find("good").is_some());
    }
}
