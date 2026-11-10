//! Speech synthesis through sherpa-onnx.

use crate::catalog::{Family, ModelInfo};
use anyhow::{anyhow, bail, Context, Result};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKittenModelConfig,
    OfflineTtsKokoroModelConfig, OfflineTtsModelConfig, OfflineTtsSupertonicModelConfig,
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
    /// 1.0 is the model's natural pace.
    pub speed: f32,
    /// ISO 639-1 language of `text`.
    pub lang: &'a str,
}

/// Something that can turn text into audio. Implemented by [`SherpaEngine`];
/// tests substitute a fake.
pub trait Synthesizer: Send + Sync {
    /// Returns `None` if `cancel` was raised before the audio was complete.
    fn synthesize(&self, req: &SynthRequest, cancel: &Arc<AtomicBool>) -> Result<Option<Audio>>;
}

/// Identifies a loaded engine: a model plus the voice variant it was loaded
/// for (see [`crate::catalog::VoiceInfo::variant`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineKey {
    pub model_id: String,
    pub variant: Option<String>,
}

/// Hands out synthesizers for engine keys, loading them as needed.
pub trait EngineProvider: Send + Sync {
    fn engine(&self, key: &EngineKey) -> Result<Arc<dyn Synthesizer>>;
    /// Whether [`EngineProvider::engine`] would return without loading.
    fn is_loaded(&self, key: &EngineKey) -> bool;
}

pub struct SherpaEngine {
    tts: OfflineTts,
    family: Family,
}

fn existing(dir: &Path, names: &[&str]) -> Result<String> {
    names
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow!("model file missing in {}: {}", dir.display(), names.join(" or ")))
}

impl SherpaEngine {
    /// Loads `model` from its installed directory `dir`.
    pub fn load(model: &ModelInfo, dir: &Path, variant: Option<&str>, threads: i32) -> Result<Self> {
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
                    duration_predictor: Some(file(&["duration_predictor.int8.onnx", "duration_predictor.onnx"])?),
                    text_encoder: Some(file(&["text_encoder.int8.onnx", "text_encoder.onnx"])?),
                    vector_estimator: Some(file(&["vector_estimator.int8.onnx", "vector_estimator.onnx"])?),
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
                config.model.kokoro = OfflineTtsKokoroModelConfig {
                    model: Some(file(&["model.onnx", "model.int8.onnx"])?),
                    voices: Some(file(&["voices.bin"])?),
                    tokens: Some(file(&["tokens.txt"])?),
                    lexicon: Some(format!("{},{}", file(&[english])?, file(&["lexicon-zh.txt"])?)),
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
        }
        let tts = OfflineTts::create(&config)
            .with_context(|| format!("sherpa-onnx could not load {} from {}", model.name, dir.display()))?;
        Ok(Self { tts, family: model.family })
    }

    pub fn sample_rate(&self) -> u32 {
        self.tts.sample_rate().max(0) as u32
    }
}

impl Synthesizer for SherpaEngine {
    fn synthesize(&self, req: &SynthRequest, cancel: &Arc<AtomicBool>) -> Result<Option<Audio>> {
        if req.text.contains('\0') {
            bail!("text contains a NUL character");
        }
        let mut extra = HashMap::new();
        if self.family == Family::Supertonic {
            extra.insert("lang".to_string(), serde_json::json!(req.lang));
        }
        // Lexicon-driven models only know words, so spell numbers out.
        let normalized;
        let text = if self.family != Family::Supertonic && req.lang == "en" {
            normalized = crate::numbers::spell_out_english(req.text);
            normalized.as_str()
        } else {
            req.text
        };
        let config = GenerationConfig {
            sid: req.sid,
            speed: req.speed.clamp(0.25, 4.0),
            extra: Some(extra),
            ..Default::default()
        };
        let flag = cancel.clone();
        let keep_going = move |_: &[f32], _: f32| !flag.load(Ordering::Relaxed);
        let audio = self
            .tts
            .generate_with_config(text, &config, Some(keep_going))
            .ok_or_else(|| anyhow!("the model produced no audio"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(Audio {
            samples: audio.samples().to_vec(),
            sample_rate: audio.sample_rate().max(1) as u32,
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

    /// Drops the engines of one model (it is being deleted).
    pub fn unload_model(&self, model_id: &str) {
        self.loaded.lock().unwrap().retain(|(k, _)| k.model_id != model_id);
    }

    pub fn loaded_keys(&self) -> Vec<EngineKey> {
        self.loaded.lock().unwrap().iter().map(|(k, _)| k.clone()).collect()
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
        let engine = Arc::new(SherpaEngine::load(model, &dir, key.variant.as_deref(), self.threads)?);
        log::info!("loaded {} ({:?}) in {:.2}s", model.id, key.variant, started.elapsed().as_secs_f32());
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
}
