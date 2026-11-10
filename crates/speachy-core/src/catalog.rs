//! The built-in catalog of downloadable TTS models and their voices.
//!
//! Every model here runs through sherpa-onnx (see [`crate::engine`]). Archives
//! are the ones published on the sherpa-onnx `tts-models` release.

use serde::Serialize;
use std::sync::OnceLock;

const RELEASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models";

/// Model architecture. Decides how [`crate::engine`] wires the files up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Supertonic,
    Kokoro,
    Kitten,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    Female,
    Male,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceInfo {
    /// Stable id, unique within the model (e.g. `af_heart`).
    pub id: String,
    /// Display name (e.g. `Heart`).
    pub name: String,
    /// Speaker id passed to the model.
    pub sid: i32,
    pub gender: Gender,
    /// ISO 639-1 languages this voice can speak. A voice of a multilingual
    /// model lists every language of the model.
    pub languages: Vec<String>,
    /// Accent / flavour shown next to the name (e.g. `US`, `British`).
    pub accent: Option<String>,
    /// Which lexicon set the engine must be loaded with (Kokoro only). Two
    /// voices with different variants need separately loaded engines.
    pub variant: Option<String>,
}

/// An extra file that lives outside the model archive.
#[derive(Debug, Clone, Serialize)]
pub struct ExtraFile {
    pub url: String,
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub family: Family,
    pub description: String,
    /// Where the weights come from, shown in the UI.
    pub license: String,
    pub archive_url: String,
    /// Name of the single top-level directory inside the archive.
    pub archive_dir: String,
    /// Download size of the archive plus extra files.
    pub size_bytes: u64,
    pub extra_files: Vec<ExtraFile>,
    pub sample_rate: u32,
    /// ISO 639-1 codes.
    pub languages: Vec<String>,
    pub voices: Vec<VoiceInfo>,
    /// Voice used when the user has not picked one.
    pub default_voice: String,
    pub recommended: bool,
    /// Rough relative synthesis speed, 1 (slow) to 3 (fast), for the UI.
    pub speed_tier: u8,
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
}

pub fn catalog() -> &'static [ModelInfo] {
    static CATALOG: OnceLock<Vec<ModelInfo>> = OnceLock::new();
    CATALOG.get_or_init(build)
}

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    catalog().iter().find(|m| m.id == id)
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
        kitten(
            "kitten-nano-v0.2",
            "KittenTTS Nano 0.2",
            "kitten-nano-en-v0_2-fp16",
            26_586_708,
            "Tiny 15M-parameter English model. Smallest download and very fast; quality is a \
             step below the larger models.",
        ),
        kitten(
            "kitten-mini-v0.1",
            "KittenTTS Mini 0.1",
            "kitten-mini-en-v0_1-fp16",
            156_630_103,
            "80M-parameter English model with the KittenTTS voice set.",
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
                gender,
                languages: languages.clone(),
                accent: None,
                variant: None,
            }
        })
        .collect();
    let dir = "sherpa-onnx-supertonic-3-tts-int8-2026-05-11";
    ModelInfo {
        id: "supertonic-3".into(),
        name: "Supertonic 3".into(),
        family: Family::Supertonic,
        description: "Fast multilingual model: 31 languages with the same ten voices, 44.1 kHz \
                      output. The best starting point, and the one to use with automatic \
                      language detection."
            .into(),
        license: "Supertone (OpenRAIL-M)".into(),
        archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
        archive_dir: dir.into(),
        size_bytes: 128_774_318,
        extra_files: vec![],
        sample_rate: 44_100,
        languages,
        voices,
        default_voice: "F1".into(),
        recommended: true,
        speed_tier: 3,
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
                // Chinese voices read embedded English with the US lexicon.
                _ => (strings(&["zh", "en"]), "Mandarin", "us"),
            };
            let mut name = short.to_string();
            name[..1].make_ascii_uppercase();
            VoiceInfo {
                id: vid.into(),
                name,
                sid,
                gender,
                languages,
                accent: Some(accent.into()),
                variant: Some(variant.into()),
            }
        })
        .collect();
    ModelInfo {
        id: id.into(),
        name: name.into(),
        family: Family::Kokoro,
        description: description.into(),
        license: "Apache-2.0".into(),
        archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
        archive_dir: dir.into(),
        size_bytes: size,
        extra_files: vec![],
        sample_rate: 24_000,
        languages: strings(&["en", "zh"]),
        voices,
        default_voice: "af_heart".into(),
        recommended: false,
        speed_tier: 1,
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
            gender,
            languages: strings(&["en"]),
            accent: Some("US".into()),
            variant: None,
        })
        .collect();
    let lexicon = ExtraFile {
        url: format!("{RELEASE}/lexicon-en-us.txt"),
        filename: "lexicon-en-us.txt".into(),
        size_bytes: 6_305_773,
    };
    ModelInfo {
        id: id.into(),
        name: name.into(),
        family: Family::Kitten,
        description: description.into(),
        license: "Apache-2.0".into(),
        archive_url: format!("{RELEASE}/{dir}.tar.bz2"),
        archive_dir: dir.into(),
        size_bytes: size + lexicon.size_bytes,
        extra_files: vec![lexicon],
        sample_rate: 24_000,
        languages: strings(&["en"]),
        voices,
        default_voice: "expr-voice-2-f".into(),
        recommended: false,
        speed_tier: 3,
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
        for m in catalog() {
            assert!(ids.insert(&m.id), "duplicate model id {}", m.id);
            assert!(m.voice(&m.default_voice).is_some(), "{}: bad default voice", m.id);
            let mut voice_ids = HashSet::new();
            let mut sids = HashSet::new();
            for v in &m.voices {
                assert!(voice_ids.insert(&v.id), "{}: duplicate voice {}", m.id, v.id);
                assert!(sids.insert(v.sid), "{}: duplicate sid {}", m.id, v.sid);
                assert!(!v.languages.is_empty());
                for l in &v.languages {
                    assert!(m.languages.contains(l), "{}: {} speaks unlisted {l}", m.id, v.id);
                    assert_ne!(language_name(l), "Unknown", "no display name for {l}");
                }
            }
        }
    }

    #[test]
    fn voice_for_language_prefers_the_requested_voice() {
        let kokoro = find("kokoro-v1.0").unwrap();
        assert_eq!(kokoro.voice_for_language("en", Some("bm_george")).unwrap().id, "bm_george");
        // A US voice cannot read Chinese, so a Chinese voice is substituted.
        assert_eq!(kokoro.voice_for_language("zh", Some("af_heart")).unwrap().id, "zf_xiaobei");
        assert!(kokoro.voice_for_language("ru", None).is_none());
        let supertonic = find("supertonic-3").unwrap();
        assert_eq!(supertonic.voice_for_language("ru", Some("M2")).unwrap().sid, 6);
    }
}
