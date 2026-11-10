//! Command-line front end for the speech pipeline. Useful for trying models
//! and for timing the pipeline without the desktop app.
//!
//! ```text
//! cargo run --release --example speak -- --install supertonic-3
//! cargo run --release --example speak -- "Hello from Speachy."
//! cargo run --release --example speak -- --voice M2 --out hello.wav "Hello."
//! echo "Text from a pipe" | cargo run --release --example speak
//! ```

use anyhow::{bail, Context, Result};
use speachy_core::audio::{AudioBackend, CaptureAudio, SystemAudio};
use speachy_core::engine::EngineCache;
use speachy_core::models::{ModelManager, Phase};
use speachy_core::pipeline::{Outcome, SpeakOptions, Speaker, SpeechEvent};
use speachy_core::settings::{Settings, VoiceRef};
use speachy_core::{catalog, plan, wav};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let mut models_dir = std::env::var_os("SPEACHY_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models"));
    let mut settings = Settings::default();
    let (mut model_id, mut voice_id, mut out, mut install) = (None, None, None, None);
    // With --out: consume audio at playback pace instead of instantly, to
    // observe the pipeline's timing as a listener would.
    let mut realtime = false;
    let mut words = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--models" => models_dir = value("--models")?.into(),
            "--model" => model_id = Some(value("--model")?),
            "--voice" => voice_id = Some(value("--voice")?),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--install" => install = Some(value("--install")?),
            "--speed" => settings.speed = value("--speed")?.parse()?,
            "--lookahead" => settings.lookahead_secs = value("--lookahead")?.parse()?,
            "--lang" => {
                settings.language = value("--lang")?;
                settings.auto_detect_language = false;
            }
            "--realtime" => realtime = true,
            "--catalog-json" => {
                println!("{}", serde_json::to_string_pretty(catalog::catalog())?);
                return Ok(());
            }
            "--list" => {
                for m in catalog::catalog() {
                    println!(
                        "{:<20} {:>4} MB  {}  [{}]",
                        m.id,
                        m.size_bytes / 1_000_000,
                        m.name,
                        m.languages.join(",")
                    );
                    println!(
                        "{:<20} voices: {}",
                        "",
                        m.voices
                            .iter()
                            .map(|v| v.id.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
                return Ok(());
            }
            "-h" | "--help" => {
                println!("speak [--models DIR] [--install MODEL] [--list] [--model ID] [--voice ID] [--lang CODE] [--speed X] [--lookahead SECS] [--out FILE.wav [--realtime]] [TEXT]");
                return Ok(());
            }
            _ => words.push(arg),
        }
    }
    settings.sanitize();
    let manager = ModelManager::new(models_dir.clone());

    if let Some(id) = install {
        let model =
            catalog::find(&id).with_context(|| format!("unknown model {id}; try --list"))?;
        let last = Mutex::new(Instant::now() - Duration::from_secs(1));
        manager.install(model, &AtomicBool::new(false), &|p| {
            let mut last = last.lock().unwrap();
            if last.elapsed() > Duration::from_millis(500) || p.done_bytes == p.total_bytes {
                *last = Instant::now();
                let verb = if p.phase == Phase::Downloading {
                    "downloading"
                } else {
                    "extracting"
                };
                eprintln!(
                    "{verb} {:>4} / {} MB",
                    p.done_bytes / 1_000_000,
                    p.total_bytes / 1_000_000
                );
            }
        })?;
        eprintln!("installed {id} in {}", manager.model_dir(&id).display());
        if words.is_empty() {
            return Ok(());
        }
    }

    let mut text = words.join(" ");
    if text.is_empty() {
        std::io::stdin().read_to_string(&mut text)?;
    }
    let installed = manager.installed();
    if installed.is_empty() {
        bail!(
            "no model installed in {}; run with --install supertonic-3",
            models_dir.display()
        );
    }
    if model_id.is_some() || voice_id.is_some() {
        let model = match &model_id {
            Some(id) => *installed
                .iter()
                .find(|m| &m.id == id)
                .with_context(|| format!("model {id} is not installed"))?,
            None => installed[0],
        };
        let voice = match &voice_id {
            Some(id) => model
                .voice(id)
                .with_context(|| format!("{} has no voice {id}", model.id))?,
            None => model.default_voice(),
        };
        settings.default_voice = Some(VoiceRef {
            model_id: model.id.clone(),
            voice_id: voice.id.clone(),
        });
    }
    let chunks = plan::plan(&text, &settings, &installed)?;

    let capture = out
        .as_ref()
        .map(|_| CaptureAudio::new(if realtime { 1.0 } else { f32::INFINITY }));
    let backend: Arc<dyn AudioBackend> = match &capture {
        Some(capture) => Arc::new(capture.clone()),
        None => Arc::new(SystemAudio),
    };
    let engines = Arc::new(EngineCache::new(
        models_dir,
        settings.effective_threads(),
        2,
    ));
    let started = Instant::now();
    let failure = Arc::new(Mutex::new(None));
    let failed = failure.clone();
    let speaker = Speaker::new(engines, backend, move |event| {
        let t = started.elapsed().as_secs_f32();
        match event {
            SpeechEvent::Started { chunks, .. } => {
                for (i, c) in chunks.iter().enumerate() {
                    eprintln!(
                        "[{t:6.2}s] plan {i}: [{} {} {}] {:?}",
                        c.lang, c.model_id, c.voice_id, c.text
                    );
                }
            }
            SpeechEvent::State { state } => eprintln!("[{t:6.2}s] state: {state:?}"),
            SpeechEvent::ChunkReady {
                index,
                audio_secs,
                synth_secs,
                ..
            } => {
                eprintln!("[{t:6.2}s] chunk {index} ready: {audio_secs:.2}s of audio in {synth_secs:.2}s (rtf {:.2})", synth_secs / audio_secs)
            }
            SpeechEvent::ChunkPlaying { index, .. } => {
                eprintln!("[{t:6.2}s] chunk {index} playing")
            }
            SpeechEvent::Finished {
                outcome, message, ..
            } => {
                eprintln!(
                    "[{t:6.2}s] finished: {outcome:?} {}",
                    message.clone().unwrap_or_default()
                );
                if *outcome == Outcome::Failed {
                    *failed.lock().unwrap() = Some(message.clone().unwrap_or_default());
                }
            }
        }
    });
    speaker.speak(
        chunks,
        SpeakOptions {
            speed: settings.speed,
            volume: settings.volume,
            lookahead: Duration::from_secs_f32(settings.lookahead_secs),
        },
    );
    speaker.wait_idle();
    if let Some(message) = failure.lock().unwrap().take() {
        bail!("speech failed: {message}");
    }
    if let (Some(path), Some(capture)) = (out, capture) {
        let audio = capture.take();
        wav::write(&path, &audio)?;
        eprintln!(
            "wrote {} ({:.2}s at {} Hz)",
            path.display(),
            audio.seconds(),
            audio.sample_rate
        );
    }
    Ok(())
}
