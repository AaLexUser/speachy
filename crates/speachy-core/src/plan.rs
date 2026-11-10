//! Decides which voice reads each chunk of a text.

use crate::catalog::{ModelInfo, VoiceInfo};
use crate::engine::EngineKey;
use crate::settings::{Settings, VoiceRef};
use crate::text::{self, Boundary, ChunkOptions};
use serde::Serialize;

/// A chunk of text together with everything needed to synthesize it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedChunk {
    pub text: String,
    pub boundary: Boundary,
    /// ISO 639-1 language the chunk is read in.
    pub lang: String,
    #[serde(skip)]
    pub engine: EngineKey,
    #[serde(skip)]
    pub sid: i32,
    pub model_id: String,
    pub voice_id: String,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PlanError {
    #[error("no voice model is installed yet")]
    NoModel,
    #[error("there is no text to speak")]
    NothingToSay,
}

struct Choice<'a> {
    model: &'a ModelInfo,
    voice: &'a VoiceInfo,
}

/// Chunks shorter than this inherit the language of the whole text: a
/// handful of words is not enough for reliable identification.
const MIN_WEIGHT_FOR_OWN_LANGUAGE: usize = 40;

/// Plans how to speak `text` with the `installed` models.
pub fn plan(
    text: &str,
    settings: &Settings,
    installed: &[&ModelInfo],
) -> Result<Vec<PlannedChunk>, PlanError> {
    let chunks = text::chunk(text, &ChunkOptions::default());
    if chunks.is_empty() {
        return Err(PlanError::NothingToSay);
    }
    let default = default_choice(settings, installed).ok_or(PlanError::NoModel)?;

    let mut candidates: Vec<String> = Vec::new();
    for lang in installed.iter().flat_map(|m| m.languages.iter()) {
        if !candidates.contains(lang) {
            candidates.push(lang.clone());
        }
    }
    let fallback_lang = if default.voice.languages.contains(&settings.language) {
        settings.language.clone()
    } else {
        default.voice.languages[0].clone()
    };
    let detect = |s: &str| crate::lang::detect(s, &candidates);
    let text_lang = if settings.auto_detect_language {
        detect(text)
            .filter(|d| d.reliable || text::weight(text) >= MIN_WEIGHT_FOR_OWN_LANGUAGE)
            .map(|d| d.code.to_string())
    } else {
        None
    }
    .unwrap_or(fallback_lang);

    Ok(chunks
        .into_iter()
        .map(|chunk| {
            let lang = if settings.auto_detect_language
                && text::weight(&chunk.text) >= MIN_WEIGHT_FOR_OWN_LANGUAGE
            {
                detect(&chunk.text)
                    .filter(|d| d.reliable)
                    .map_or(text_lang.clone(), |d| d.code.to_string())
            } else {
                text_lang.clone()
            };
            let (choice, lang) = resolve(&lang, settings, installed, &default);
            PlannedChunk {
                text: chunk.text,
                boundary: chunk.boundary,
                lang,
                engine: EngineKey {
                    model_id: choice.model.id.clone(),
                    variant: choice.voice.variant.clone(),
                },
                sid: choice.voice.sid,
                model_id: choice.model.id.clone(),
                voice_id: choice.voice.id.clone(),
            }
        })
        .collect())
}

fn lookup<'a>(voice: &VoiceRef, installed: &[&'a ModelInfo]) -> Option<Choice<'a>> {
    let model = installed.iter().find(|m| m.id == voice.model_id)?;
    Some(Choice {
        model,
        voice: model.voice(&voice.voice_id)?,
    })
}

fn default_choice<'a>(settings: &Settings, installed: &[&'a ModelInfo]) -> Option<Choice<'a>> {
    settings
        .default_voice
        .as_ref()
        .and_then(|v| lookup(v, installed))
        .or_else(|| {
            installed.first().map(|model| Choice {
                model,
                voice: model.default_voice(),
            })
        })
}

/// The voice that would read `lang`, and the language it will actually be
/// asked to speak (which differs when nothing installed speaks `lang`).
fn resolve<'a>(
    lang: &str,
    settings: &Settings,
    installed: &[&'a ModelInfo],
    default: &Choice<'a>,
) -> (Choice<'a>, String) {
    let speaks = |c: &Choice| c.voice.languages.iter().any(|l| l == lang);
    // 1. The user's explicit choice for this language.
    if let Some(choice) = settings
        .language_voices
        .get(lang)
        .and_then(|v| lookup(v, installed))
        .filter(speaks)
    {
        return (choice, lang.to_string());
    }
    // 2. The default voice, or another voice of its model.
    if let Some(voice) = default
        .model
        .voice_for_language(lang, Some(&default.voice.id))
    {
        return (
            Choice {
                model: default.model,
                voice,
            },
            lang.to_string(),
        );
    }
    // 3. Any other installed model that covers the language.
    for model in installed {
        if let Some(voice) = model.voice_for_language(lang, None) {
            return (Choice { model, voice }, lang.to_string());
        }
    }
    // 4. Nothing speaks it: read with the default voice in its own language.
    let own = default.voice.languages[0].clone();
    (
        Choice {
            model: default.model,
            voice: default.voice,
        },
        own,
    )
}

/// The voice a preview or a fixed-language request should use.
pub fn voice_for(settings: &Settings, installed: &[&ModelInfo], lang: &str) -> Option<VoiceRef> {
    let default = default_choice(settings, installed)?;
    let (choice, _) = resolve(lang, settings, installed, &default);
    Some(VoiceRef {
        model_id: choice.model.id.clone(),
        voice_id: choice.voice.id.clone(),
    })
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::catalog::find;

    const EN: &str =
        "The quick brown fox jumps over the lazy dog and keeps running through the forest.";
    const RU: &str =
        "Привет! Это проверка синтеза речи на русском языке, сегодня очень хорошая погода.";
    const ZH: &str = "今天天气很好，我们一起去公园散步吧，然后再去喝一杯咖啡。";

    fn voice(model: &str, voice: &str) -> VoiceRef {
        VoiceRef {
            model_id: model.into(),
            voice_id: voice.into(),
        }
    }

    #[test]
    fn errors_without_models_or_text() {
        let settings = Settings::default();
        assert_eq!(plan(EN, &settings, &[]), Err(PlanError::NoModel));
        let supertonic = find("supertonic-3").unwrap();
        assert_eq!(
            plan(" \n ", &settings, &[supertonic]),
            Err(PlanError::NothingToSay)
        );
    }

    #[test]
    fn multilingual_model_reads_each_language_with_the_default_voice() {
        let supertonic = find("supertonic-3").unwrap();
        let mut settings = Settings::default();
        settings.default_voice = Some(voice("supertonic-3", "M2"));
        let planned = plan(&format!("{EN}\n\n{RU}"), &settings, &[supertonic]).unwrap();
        assert_eq!(planned.first().unwrap().lang, "en");
        assert_eq!(planned.last().unwrap().lang, "ru");
        assert!(planned.iter().all(|c| c.voice_id == "M2" && c.sid == 6));
    }

    #[test]
    fn per_language_voice_overrides_the_default() {
        let supertonic = find("supertonic-3").unwrap();
        let kokoro = find("kokoro-v1.0").unwrap();
        let mut settings = Settings::default();
        settings.default_voice = Some(voice("kokoro-v1.0", "bm_george"));
        settings
            .language_voices
            .insert("ru".into(), voice("supertonic-3", "F3"));
        let installed = [kokoro, supertonic];

        let en = plan(EN, &settings, &installed).unwrap();
        assert_eq!(
            (en[0].model_id.as_str(), en[0].voice_id.as_str()),
            ("kokoro-v1.0", "bm_george")
        );
        assert_eq!(en[0].engine.variant.as_deref(), Some("gb"));

        let ru = plan(RU, &settings, &installed).unwrap();
        assert_eq!(
            (
                ru[0].model_id.as_str(),
                ru[0].voice_id.as_str(),
                ru[0].lang.as_str()
            ),
            ("supertonic-3", "F3", "ru")
        );

        // The default voice cannot read Chinese; a voice of its model can.
        let zh = plan(ZH, &settings, &installed).unwrap();
        assert_eq!(
            (zh[0].model_id.as_str(), zh[0].voice_id.as_str()),
            ("kokoro-v1.0", "zf_xiaobei")
        );

        // German: not covered by Kokoro, falls through to the other model.
        let de = plan(
            "Dies ist ein Test der Sprachsynthese, und das Wetter ist heute wirklich schön.",
            &settings,
            &installed,
        )
        .unwrap();
        assert_eq!(
            (de[0].model_id.as_str(), de[0].lang.as_str()),
            ("supertonic-3", "de")
        );
    }

    #[test]
    fn unsupported_language_falls_back_to_the_default_voice() {
        let kitten = find("kitten-nano-v0.2").unwrap();
        let planned = plan(RU, &Settings::default(), &[kitten]).unwrap();
        assert_eq!(planned[0].lang, "en");
        assert_eq!(planned[0].voice_id, kitten.default_voice);
    }

    #[test]
    fn detection_can_be_turned_off() {
        let supertonic = find("supertonic-3").unwrap();
        let mut settings = Settings::default();
        settings.auto_detect_language = false;
        settings.language = "de".into();
        let planned = plan(RU, &settings, &[supertonic]).unwrap();
        assert!(planned.iter().all(|c| c.lang == "de"));
    }

    #[test]
    fn uninstalled_default_voice_is_ignored() {
        let kitten = find("kitten-nano-v0.2").unwrap();
        let mut settings = Settings::default();
        settings.default_voice = Some(voice("supertonic-3", "M2"));
        let planned = plan(EN, &settings, &[kitten]).unwrap();
        assert_eq!(planned[0].model_id, "kitten-nano-v0.2");
    }

    #[test]
    fn short_chunks_inherit_the_language_of_the_text() {
        let supertonic = find("supertonic-3").unwrap();
        let planned = plan(
            &format!("{RU} Ок. Да."),
            &Settings::default(),
            &[supertonic],
        )
        .unwrap();
        assert!(planned.iter().all(|c| c.lang == "ru"), "{planned:?}");
    }
}
