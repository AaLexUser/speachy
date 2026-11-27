//! Speech synthesis through sherpa-onnx.

use crate::catalog::{Family, ModelInfo};
use crate::english::Phonemizer;
use anyhow::{anyhow, bail, Context, Result};
use sherpa_onnx::{
    GenerationConfig, LinearResampler, OfflineTts, OfflineTtsConfig, OfflineTtsKittenModelConfig,
    OfflineTtsKokoroModelConfig, OfflineTtsModelConfig, OfflineTtsPocketModelConfig,
    OfflineTtsSupertonicModelConfig, Wave,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Mono PCM audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Audio {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate.max(1) as f32
    }
}

#[derive(Debug, Clone)]
pub struct SynthRequest<'a> {
    pub text: &'a str,
    /// Speaker id within the model.
    pub sid: i32,
    /// ISO 639-1 language of `text`.
    pub lang: &'a str,
}

/// What a synthesizer makes of a chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Speech {
    pub audio: Audio,
    /// The chunk as it was read: numbers, units and abbreviations in words.
    /// `None` when the model read the text as written.
    pub spoken: Option<String>,
}

/// Something that can turn text into audio. Implemented by [`SherpaEngine`];
/// tests substitute a fake.
pub trait Synthesizer: Send + Sync {
    /// Returns `None` if `cancel` was raised before the audio was complete.
    fn synthesize(&self, req: &SynthRequest, cancel: &Arc<AtomicBool>) -> Result<Option<Speech>>;
}

/// Identifies a loaded engine: a model plus the voice variant it was loaded
/// for (see [`crate::catalog::VoiceInfo::variant`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct EngineKey {
    pub model_id: String,
    pub variant: Option<String>,
}

/// Hands out synthesizers for engine keys, loading them as needed.
pub trait EngineProvider: Send + Sync {
    fn engine(&self, key: &EngineKey) -> Result<Arc<dyn Synthesizer>>;
    /// Whether [`EngineProvider::engine`] would return without loading.
    fn is_loaded(&self, key: &EngineKey) -> bool;
    /// Lets go of everything loaded for a model that is being removed.
    fn unload_model(&self, _model_id: &str) {}
}

pub struct SherpaEngine {
    tts: OfflineTts,
    family: Family,
    /// English phonemizer, for the families that are given phonemes.
    phonemizer: Option<Phonemizer>,
    /// Recordings of the voices, by speaker id, for the families that
    /// imitate one.
    recordings: HashMap<i32, Recording>,
}

/// A recording of a voice for the model to imitate.
struct Recording {
    samples: Vec<f32>,
    sample_rate: i32,
}

impl Recording {
    /// Reads a WAV file, converted to `sample_rate` so the model does not
    /// have to convert it for every sentence.
    fn read(path: &str, sample_rate: i32) -> Result<Self> {
        let wave = Wave::read(path)
            .ok_or_else(|| anyhow!("{path} is not a WAV file this model can read"))?;
        if wave.samples().is_empty() || wave.sample_rate() <= 0 {
            bail!("{path} holds no audio");
        }
        if sample_rate <= 0 || wave.sample_rate() == sample_rate {
            return Ok(Self {
                samples: wave.samples().to_vec(),
                sample_rate: wave.sample_rate(),
            });
        }
        let resampler = LinearResampler::create(wave.sample_rate(), sample_rate)
            .ok_or_else(|| anyhow!("cannot convert {path} to {sample_rate} Hz"))?;
        Ok(Self {
            samples: resampler.resample(wave.samples(), true),
            sample_rate,
        })
    }
}

/// Steps of Pocket TTS's sampler for each frame of audio.
const POCKET_STEPS: i32 = 2;

/// Brings speech to a common loudness without exceeding full scale. Pocket
/// TTS speaks as loudly as the recording it imitates, which differs several
/// times over between voices and can clip.
fn level(samples: &mut [f32]) {
    const LOUDNESS: f32 = 0.1;
    const CEILING: f32 = 0.95;
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    if samples.is_empty() || peak < 1e-4 {
        return;
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let gain = (LOUDNESS / power.sqrt())
        .clamp(0.25, 8.0)
        .min(CEILING / peak);
    for sample in samples {
        *sample *= gain;
    }
}

/// KittenTTS ends every output with noise, often a murmur, and a click. Its
/// own code drops them (`audio = outputs[0][..., :-5000]`, 5000 samples at
/// 24 kHz, in KittenML/KittenTTS `kittenml/kittentts_legacy/onnx_model.py`
/// line 159); sherpa-onnx returns them.
const KITTEN_TAIL_SECONDS: f64 = 5000.0 / 24000.0;

fn without_kitten_tail(samples: &[f32], sample_rate: u32) -> &[f32] {
    let tail = (KITTEN_TAIL_SECONDS * f64::from(sample_rate)).round() as usize;
    &samples[..samples.len().saturating_sub(tail)]
}

fn existing(dir: &Path, names: &[&str]) -> Result<String> {
    names
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| {
            anyhow!(
                "model file missing in {}: {}",
                dir.display(),
                names.join(" or ")
            )
        })
}

impl SherpaEngine {
    /// Loads `model` from its installed directory `dir`.
    pub fn load(
        model: &ModelInfo,
        dir: &Path,
        variant: Option<&str>,
        threads: i32,
    ) -> Result<Self> {
        let file = |names: &[&str]| existing(dir, names);
        let mut config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                num_threads: threads.max(1),
                ..Default::default()
            },
            // One sentence per callback, so a stop request is honoured quickly.
            max_num_sentences: 1,
            ..Default::default()
        };
        match model.family {
            Family::Supertonic => {
                config.model.supertonic = OfflineTtsSupertonicModelConfig {
                    duration_predictor: Some(file(&[
                        "duration_predictor.int8.onnx",
                        "duration_predictor.onnx",
                    ])?),
                    text_encoder: Some(file(&["text_encoder.int8.onnx", "text_encoder.onnx"])?),
                    vector_estimator: Some(file(&[
                        "vector_estimator.int8.onnx",
                        "vector_estimator.onnx",
                    ])?),
                    vocoder: Some(file(&["vocoder.int8.onnx", "vocoder.onnx"])?),
                    tts_json: Some(file(&["tts.json"])?),
                    unicode_indexer: Some(file(&["unicode_indexer.bin"])?),
                    voice_style: Some(file(&["voice.bin"])?),
                };
            }
            Family::Kokoro => {
                let english = match variant {
                    Some("gb") => "lexicon-gb-en.txt",
                    _ => "lexicon-us-en.txt",
                };
                if variant == Some("zh") {
                    // Rules that read numbers, dates and phone numbers out
                    // in Chinese.
                    let rules: Vec<String> = ["date-zh.fst", "phone-zh.fst", "number-zh.fst"]
                        .iter()
                        .map(|name| dir.join(name))
                        .filter(|path| path.is_file())
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect();
                    if !rules.is_empty() {
                        config.rule_fsts = Some(rules.join(","));
                    }
                }
                config.model.kokoro = OfflineTtsKokoroModelConfig {
                    model: Some(file(&["model.onnx", "model.int8.onnx"])?),
                    voices: Some(file(&["voices.bin"])?),
                    tokens: Some(file(&["tokens.txt"])?),
                    lexicon: Some(format!(
                        "{},{}",
                        file(&[english])?,
                        file(&["lexicon-zh.txt"])?
                    )),
                    ..Default::default()
                };
            }
            Family::Kitten => {
                config.model.kitten = OfflineTtsKittenModelConfig {
                    model: Some(file(&["model.fp16.onnx", "model.onnx"])?),
                    voices: Some(file(&["voices.bin"])?),
                    tokens: Some(file(&["tokens.txt"])?),
                    lexicon: Some(file(&["lexicon-en-us.txt"])?),
                    ..Default::default()
                };
            }
            Family::Pocket => {
                config.model.pocket = OfflineTtsPocketModelConfig {
                    lm_flow: Some(file(&["lm_flow.int8.onnx", "lm_flow.onnx"])?),
                    lm_main: Some(file(&["lm_main.int8.onnx", "lm_main.onnx"])?),
                    encoder: Some(file(&["encoder.onnx"])?),
                    decoder: Some(file(&["decoder.int8.onnx", "decoder.onnx"])?),
                    text_conditioner: Some(file(&["text_conditioner.onnx"])?),
                    vocab_json: Some(file(&["vocab.json"])?),
                    token_scores_json: Some(file(&["token_scores.json"])?),
                    // What the model makes of a recording is kept, so each
                    // voice is analysed once.
                    voice_embedding_cache_capacity: model.voices.len() as i32,
                };
            }
        }
        let tts = OfflineTts::create(&config).with_context(|| {
            format!(
                "sherpa-onnx could not load {} from {}",
                model.name,
                dir.display()
            )
        })?;
        let mut recordings = HashMap::new();
        for voice in &model.voices {
            let Some(name) = &voice.reference else {
                continue;
            };
            let recording = Recording::read(&file(&[name])?, tts.sample_rate())?;
            recordings.insert(voice.sid, recording);
        }
        let phonemizer =
            (model.family == Family::Kokoro).then(|| Phonemizer::new(variant == Some("gb")));
        Ok(Self {
            tts,
            family: model.family,
            phonemizer,
            recordings,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.tts.sample_rate().max(0) as u32
    }
}

impl Synthesizer for SherpaEngine {
    fn synthesize(&self, req: &SynthRequest, cancel: &Arc<AtomicBool>) -> Result<Option<Speech>> {
        if req.text.contains('\0') {
            bail!("text contains a NUL character");
        }
        let mut extra = HashMap::new();
        if self.family == Family::Supertonic {
            extra.insert("lang".to_string(), serde_json::json!(req.lang));
        }
        let mut text = req.text;
        let mut phonemes = None;
        let mut spoken = None;
        match (self.family, &self.phonemizer) {
            // Kokoro reads English from phonemes (see `crate::english`).
            (Family::Kokoro, Some(phonemizer)) if req.lang == "en" => {
                let sentences = phonemizer.phonemize(req.text);
                if sentences.is_empty() {
                    return Ok(Some(Speech {
                        audio: Audio {
                            samples: Vec::new(),
                            sample_rate: self.sample_rate().max(1),
                        },
                        spoken: None,
                    }));
                }
                phonemes = Some(sentences);
                text = "";
                // The words behind the phonemes, as near as they can be told.
                spoken = Some(crate::numbers::spell_out_english(req.text));
            }
            // KittenTTS looks words up in a lexicon, which has no digits.
            (Family::Kitten, _) if req.lang == "en" => {
                spoken = Some(crate::numbers::spell_out_english(req.text));
            }
            // Supertonic misreads Russian numbers, units and abbreviations,
            // and sometimes stresses the wrong vowel.
            (Family::Supertonic, _) if req.lang == "ru" => {
                spoken = Some(crate::stress::mark(&crate::russian::spell_out(req.text)));
            }
            _ => {}
        }
        if let (Some(read), None) = (&spoken, &phonemes) {
            text = read.as_str();
        }
        let mut config = GenerationConfig {
            sid: req.sid,
            extra: Some(extra),
            phoneme_codepoints: phonemes,
            ..Default::default()
        };
        if self.family == Family::Kitten {
            // The tail is measured in the model's own output: sherpa-onnx
            // must not shorten pauses in it first.
            config.silence_scale = 1.0;
        }
        if self.family == Family::Pocket {
            let recording = self
                .recordings
                .get(&req.sid)
                .ok_or_else(|| anyhow!("this model has no voice {}", req.sid))?;
            config.reference_audio = Some(recording.samples.clone());
            config.reference_sample_rate = recording.sample_rate;
            config.num_steps = POCKET_STEPS;
        }
        let flag = cancel.clone();
        let keep_going = move |_: &[f32], _: f32| !flag.load(Ordering::Relaxed);
        let audio = self
            .tts
            .generate_with_config(text, &config, Some(keep_going))
            .ok_or_else(|| anyhow!("the model produced no audio"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let sample_rate = audio.sample_rate().max(1) as u32;
        let mut samples = audio.samples().to_vec();
        match self.family {
            Family::Pocket => level(&mut samples),
            Family::Kitten => {
                let speech = without_kitten_tail(&samples, sample_rate).len();
                samples.truncate(speech);
            }
            Family::Supertonic | Family::Kokoro => {}
        }
        Ok(Some(Speech {
            audio: Audio {
                samples,
                sample_rate,
            },
            spoken,
        }))
    }
}

/// Keeps the most recently used engines loaded, so switching between a
/// couple of voices does not reload weights every time.
pub struct EngineCache {
    models_dir: PathBuf,
    threads: i32,
    capacity: usize,
    /// Most recently used last.
    loaded: Mutex<Vec<(EngineKey, Arc<SherpaEngine>)>>,
    /// Serializes loading, so two callers never load the same model twice.
    loading: Mutex<()>,
}

impl EngineCache {
    pub fn new(models_dir: PathBuf, threads: i32, capacity: usize) -> Self {
        Self {
            models_dir,
            threads,
            capacity: capacity.max(1),
            loaded: Mutex::new(Vec::new()),
            loading: Mutex::new(()),
        }
    }

    fn cached(&self, key: &EngineKey) -> Option<Arc<SherpaEngine>> {
        let mut loaded = self.loaded.lock().unwrap();
        let pos = loaded.iter().position(|(k, _)| k == key)?;
        let entry = loaded.remove(pos);
        let engine = entry.1.clone();
        loaded.push(entry);
        Some(engine)
    }

    /// Drops every loaded engine, releasing its memory once in-flight
    /// synthesis finishes.
    pub fn unload_all(&self) {
        self.loaded.lock().unwrap().clear();
    }

    pub fn loaded_keys(&self) -> Vec<EngineKey> {
        self.loaded
            .lock()
            .unwrap()
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }
}

impl EngineProvider for EngineCache {
    fn engine(&self, key: &EngineKey) -> Result<Arc<dyn Synthesizer>> {
        if let Some(engine) = self.cached(key) {
            return Ok(engine);
        }
        let _loading = self.loading.lock().unwrap();
        if let Some(engine) = self.cached(key) {
            return Ok(engine);
        }
        let model = crate::catalog::find(&key.model_id)
            .ok_or_else(|| anyhow!("unknown model {}", key.model_id))?;
        let dir = self.models_dir.join(&model.id);
        let started = std::time::Instant::now();
        let engine = Arc::new(SherpaEngine::load(
            model,
            &dir,
            key.variant.as_deref(),
            self.threads,
        )?);
        log::info!(
            "loaded {} ({:?}) in {:.2}s",
            model.id,
            key.variant,
            started.elapsed().as_secs_f32()
        );
        let mut loaded = self.loaded.lock().unwrap();
        while loaded.len() >= self.capacity {
            loaded.remove(0);
        }
        loaded.push((key.clone(), engine.clone()));
        Ok(engine)
    }

    fn is_loaded(&self, key: &EngineKey) -> bool {
        self.loaded.lock().unwrap().iter().any(|(k, _)| k == key)
    }

    fn unload_model(&self, model_id: &str) {
        self.loaded
            .lock()
            .unwrap()
            .retain(|(k, _)| k.model_id != model_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{level, without_kitten_tail};

    fn loudness(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..2400)
            .map(|i| amplitude * (i as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn quiet_and_loud_speech_end_up_equally_loud() {
        let (mut quiet, mut loud) = (tone(0.02), tone(0.4));
        level(&mut quiet);
        level(&mut loud);
        assert!(
            (loudness(&quiet) - 0.1).abs() < 0.005,
            "{}",
            loudness(&quiet)
        );
        assert!((loudness(&loud) - 0.1).abs() < 0.005, "{}", loudness(&loud));
    }

    #[test]
    fn levelling_never_clips() {
        // Mostly silence with one loud burst: raising it to the common
        // loudness would push the burst past full scale.
        let mut samples = vec![0.0; 24_000];
        samples[100..200].fill(0.5);
        level(&mut samples);
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.95 + 1e-6, "{peak}");
        // Audio that already exceeds full scale is brought back under it.
        let mut hot = tone(1.3);
        level(&mut hot);
        assert!(hot.iter().all(|s| s.abs() <= 0.95 + 1e-6));
    }

    #[test]
    fn kitten_output_loses_the_tail_kitten_itself_drops() {
        let audio: Vec<f32> = (0..24_000).map(|i| i as f32).collect();
        assert_eq!(without_kitten_tail(&audio, 24_000), &audio[..19_000]);
        // The same 208 ms at another sample rate.
        assert_eq!(without_kitten_tail(&audio, 48_000), &audio[..14_000]);
        assert!(without_kitten_tail(&audio[..5_000], 24_000).is_empty());
        assert!(without_kitten_tail(&audio[..100], 24_000).is_empty());
        assert!(without_kitten_tail(&[], 24_000).is_empty());
    }

    #[test]
    fn silence_is_left_alone() {
        let mut silence = vec![0.0; 100];
        level(&mut silence);
        assert!(silence.iter().all(|s| *s == 0.0));
        level(&mut []);
    }
}
