//! The streaming speech pipeline.
//!
//! A text is spoken as a sequence of chunks (see [`crate::text`]). Each
//! utterance runs two threads:
//!
//! * a **synthesis worker** that turns chunks into audio one after another,
//!   running ahead of playback until `lookahead` seconds of audio are
//!   waiting, then pausing so no work is wasted if the user stops;
//! * a **session thread** that owns the audio stream, queues finished
//!   chunks for gapless playback and reports progress.
//!
//! Speech starts as soon as the first (short) chunk is ready; later chunks
//! are already queued by the time they are needed.

use crate::audio::{AudioBackend, AudioStream, PlayEvent};
use crate::engine::{Audio, EngineProvider, SynthRequest};
use crate::plan::PlannedChunk;
use crate::text::Boundary;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SpeakOptions {
    pub speed: f32,
    pub volume: f32,
    /// How much audio to keep synthesized ahead of playback.
    pub lookahead: Duration,
}

impl Default for SpeakOptions {
    fn default() -> Self {
        Self { speed: 1.0, volume: 1.0, lookahead: Duration::from_secs(20) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    /// A model is being loaded into memory.
    Loading,
    /// Waiting for audio: before the first chunk, or if synthesis falls
    /// behind playback.
    Preparing,
    Speaking,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Completed,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpeechEvent {
    State { state: State },
    Started { session: u64, chunks: Vec<PlannedChunk> },
    /// Chunk `index` has been synthesized and queued.
    ChunkReady { session: u64, index: usize, audio_secs: f32, synth_secs: f32 },
    /// Chunk `index` started playing.
    ChunkPlaying { session: u64, index: usize },
    Finished { session: u64, outcome: Outcome, message: Option<String> },
}

type Listener = Box<dyn Fn(&SpeechEvent) + Send + Sync>;

enum Msg {
    Stop,
    Pause(bool),
    Volume(f32),
    /// The worker is about to load a model.
    Loading,
    Synth { index: usize, result: anyhow::Result<Option<Audio>>, synth_secs: f32 },
    Play(PlayEvent),
}

struct Current {
    id: u64,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
    text: String,
    paused: bool,
}

struct Inner {
    engines: Arc<dyn EngineProvider>,
    backend: Arc<dyn AudioBackend>,
    listener: Listener,
    current: Mutex<Option<Current>>,
    state: Mutex<State>,
    idle: Condvar,
    next_session: AtomicU64,
}

/// Speaks planned text. Cheap to clone; all clones control the same voice.
#[derive(Clone)]
pub struct Speaker {
    inner: Arc<Inner>,
}

impl Speaker {
    /// `listener` receives every event, in order. It is called from pipeline
    /// threads and must not call back into the `Speaker`.
    pub fn new(
        engines: Arc<dyn EngineProvider>,
        backend: Arc<dyn AudioBackend>,
        listener: impl Fn(&SpeechEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                engines,
                backend,
                listener: Box::new(listener),
                current: Mutex::new(None),
                state: Mutex::new(State::Idle),
                idle: Condvar::new(),
                next_session: AtomicU64::new(1),
            }),
        }
    }

    /// Starts speaking `plan`, replacing whatever is being spoken. Returns
    /// the session id used in events.
    pub fn speak(&self, plan: Vec<PlannedChunk>, opts: SpeakOptions) -> u64 {
        let inner = &self.inner;
        let id = inner.next_session.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let text = plan.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
        {
            let mut current = inner.current.lock().unwrap();
            if let Some(old) = current.take() {
                old.cancel.store(true, Ordering::Relaxed);
                let _ = old.tx.send(Msg::Stop);
                (inner.listener)(&SpeechEvent::Finished { session: old.id, outcome: Outcome::Stopped, message: None });
            }
            *current = Some(Current { id, cancel: cancel.clone(), tx: tx.clone(), text, paused: false });
            (inner.listener)(&SpeechEvent::Started { session: id, chunks: plan.clone() });
            inner.set_state(State::Preparing);
        }
        let inner = inner.clone();
        std::thread::Builder::new()
            .name("speachy-session".into())
            .spawn(move || run_session(inner, id, plan, opts, cancel, tx, rx))
            .expect("spawn session thread");
        id
    }

    /// Stops speaking. Returns whether anything was being spoken.
    pub fn stop(&self) -> bool {
        match &*self.inner.current.lock().unwrap() {
            Some(current) => {
                current.cancel.store(true, Ordering::Relaxed);
                let _ = current.tx.send(Msg::Stop);
                true
            }
            None => false,
        }
    }

    pub fn set_paused(&self, paused: bool) {
        if let Some(current) = self.inner.current.lock().unwrap().as_mut() {
            current.paused = paused;
            let _ = current.tx.send(Msg::Pause(paused));
        }
    }

    /// Pauses if speaking, resumes if paused.
    pub fn toggle_pause(&self) {
        let paused = self.inner.current.lock().unwrap().as_ref().map(|c| c.paused);
        if let Some(paused) = paused {
            self.set_paused(!paused);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(current) = &*self.inner.current.lock().unwrap() {
            let _ = current.tx.send(Msg::Volume(volume));
        }
    }

    pub fn state(&self) -> State {
        *self.inner.state.lock().unwrap()
    }

    pub fn is_active(&self) -> bool {
        self.inner.current.lock().unwrap().is_some()
    }

    /// The text being spoken, if any.
    pub fn current_text(&self) -> Option<String> {
        self.inner.current.lock().unwrap().as_ref().map(|c| c.text.clone())
    }

    /// Blocks until nothing is being spoken.
    pub fn wait_idle(&self) {
        let mut state = self.inner.state.lock().unwrap();
        while *state != State::Idle {
            state = self.inner.idle.wait(state).unwrap();
        }
    }
}

impl Inner {
    /// Must be called with `current` locked, so state changes and events of
    /// different sessions cannot interleave.
    fn set_state(&self, state: State) {
        let mut slot = self.state.lock().unwrap();
        if *slot != state {
            *slot = state;
            (self.listener)(&SpeechEvent::State { state });
            if state == State::Idle {
                self.idle.notify_all();
            }
        }
    }

    /// Runs `f` if session `id` is still the current one.
    fn if_current(&self, id: u64, f: impl FnOnce(&Self)) {
        let current = self.current.lock().unwrap();
        if current.as_ref().is_some_and(|c| c.id == id) {
            f(self);
        }
    }

    fn emit(&self, id: u64, event: SpeechEvent) {
        self.if_current(id, |inner| (inner.listener)(&event));
    }

    fn finish(&self, id: u64, outcome: Outcome, message: Option<String>) {
        let mut current = self.current.lock().unwrap();
        if current.as_ref().is_some_and(|c| c.id == id) {
            *current = None;
            (self.listener)(&SpeechEvent::Finished { session: id, outcome, message });
            self.set_state(State::Idle);
        }
    }
}

/// Lets the worker run ahead of playback by a bounded amount of audio.
#[derive(Default)]
struct Gate {
    /// Seconds of audio synthesized but not yet started playing.
    waiting_secs: Mutex<f32>,
    changed: Condvar,
}

impl Gate {
    fn add(&self, secs: f32) {
        *self.waiting_secs.lock().unwrap() += secs;
        self.changed.notify_all();
    }

    /// Blocks while at least `limit` seconds are waiting. Returns `false`
    /// if cancelled.
    fn wait_below(&self, limit: f32, cancel: &AtomicBool) -> bool {
        let mut waiting = self.waiting_secs.lock().unwrap();
        while *waiting >= limit && !cancel.load(Ordering::Relaxed) {
            (waiting, _) = self.changed.wait_timeout(waiting, Duration::from_millis(100)).unwrap();
        }
        !cancel.load(Ordering::Relaxed)
    }
}

fn synthesis_worker(
    engines: Arc<dyn EngineProvider>,
    plan: Vec<PlannedChunk>,
    speed: f32,
    lookahead: f32,
    gate: Arc<Gate>,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
) {
    for (index, chunk) in plan.iter().enumerate() {
        if !gate.wait_below(lookahead, &cancel) {
            return;
        }
        if !engines.is_loaded(&chunk.engine) {
            let _ = tx.send(Msg::Loading);
        }
        let started = Instant::now();
        let request = SynthRequest { text: &chunk.text, sid: chunk.sid, speed, lang: &chunk.lang };
        let result = engines.engine(&chunk.engine).and_then(|engine| engine.synthesize(&request, &cancel));
        let synth_secs = started.elapsed().as_secs_f32();
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // Count the audio as waiting before the session thread sees it, so
        // the next iteration cannot slip past the gate.
        if let Ok(Some(audio)) = &result {
            gate.add(audio.seconds());
        }
        if tx.send(Msg::Synth { index, result, synth_secs }).is_err() {
            return;
        }
    }
}

fn run_session(
    inner: Arc<Inner>,
    id: u64,
    plan: Vec<PlannedChunk>,
    opts: SpeakOptions,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
) {
    let started = Instant::now();
    let total = plan.len();
    let gate = Arc::new(Gate::default());
    let speed = opts.speed.clamp(0.25, 4.0);

    {
        let (engines, plan, gate, cancel, tx) = (inner.engines.clone(), plan.clone(), gate.clone(), cancel.clone(), tx.clone());
        let lookahead = opts.lookahead.as_secs_f32();
        std::thread::Builder::new()
            .name("speachy-synth".into())
            .spawn(move || synthesis_worker(engines, plan, speed, lookahead, gate, cancel, tx))
            .expect("spawn synthesis thread");
    }

    // The device is opened while the first chunk is being synthesized.
    let play_tx = tx.clone();
    let opened = inner.backend.open(Arc::new(move |event| {
        let _ = play_tx.send(Msg::Play(event));
    }));
    let mut stream: Box<dyn AudioStream> = match opened {
        Ok(stream) => stream,
        Err(err) => {
            cancel.store(true, Ordering::Relaxed);
            inner.finish(id, Outcome::Failed, Some(format!("{err:#}")));
            return;
        }
    };
    stream.set_volume(opts.volume);

    // Audio seconds of chunks that are queued but have not started playing.
    let mut waiting: HashMap<usize, f32> = HashMap::new();
    let mut done = 0;
    let mut playing = false;
    let mut ever_played = false;
    let mut paused = false;
    let mut failure: Option<String> = None;

    let outcome = loop {
        if done == total {
            break Outcome::Completed;
        }
        let Ok(msg) = rx.recv() else { break Outcome::Stopped };
        match msg {
            Msg::Stop => break Outcome::Stopped,
            Msg::Pause(pause) => {
                paused = pause;
                stream.set_paused(pause);
                let state = match (paused, playing) {
                    (true, _) => State::Paused,
                    (false, true) => State::Speaking,
                    (false, false) => State::Preparing,
                };
                inner.if_current(id, |inner| inner.set_state(state));
            }
            Msg::Volume(volume) => stream.set_volume(volume),
            Msg::Loading => {
                if !playing && !paused {
                    inner.if_current(id, |inner| inner.set_state(State::Loading));
                }
            }
            Msg::Synth { index, result, synth_secs } => {
                let raw_secs = match &result {
                    Ok(Some(audio)) => audio.seconds(),
                    _ => 0.0,
                };
                let audio = match result {
                    Ok(Some(audio)) => trim_silence(audio),
                    Ok(None) => continue,
                    Err(err) => {
                        log::warn!("chunk {index} failed: {err:#}");
                        failure.get_or_insert(format!("{err:#}"));
                        Audio { samples: Vec::new(), sample_rate: 1 }
                    }
                };
                if audio.samples.is_empty() {
                    // Nothing to play for this chunk: release its share of
                    // the look-ahead budget and move on.
                    gate.add(-raw_secs);
                    done += 1;
                    continue;
                }
                if index == 0 {
                    log::info!("first audio ready {:.0} ms after the request", started.elapsed().as_secs_f32() * 1000.0);
                }
                inner.emit(id, SpeechEvent::ChunkReady { session: id, index, audio_secs: audio.seconds(), synth_secs });
                waiting.insert(index, raw_secs);
                let pause = pause_after(&plan[index], speed);
                stream.push(index, audio, pause);
            }
            Msg::Play(PlayEvent::Started(index)) => {
                if let Some(secs) = waiting.remove(&index) {
                    gate.add(-secs);
                }
                playing = true;
                ever_played = true;
                inner.if_current(id, |inner| {
                    (inner.listener)(&SpeechEvent::ChunkPlaying { session: id, index });
                    if !paused {
                        inner.set_state(State::Speaking);
                    }
                });
            }
            Msg::Play(PlayEvent::Finished(_)) => {
                done += 1;
                // Playback ran dry before the next chunk was ready.
                if waiting.is_empty() && done < total {
                    playing = false;
                    if !paused {
                        inner.if_current(id, |inner| inner.set_state(State::Preparing));
                    }
                }
            }
        }
    };

    cancel.store(true, Ordering::Relaxed);
    gate.changed.notify_all();
    drop(stream);
    match (outcome, failure) {
        // Every chunk failed, or the text ended without any audio.
        (Outcome::Completed, Some(message)) if !ever_played => inner.finish(id, Outcome::Failed, Some(message)),
        (outcome, _) => inner.finish(id, outcome, None),
    }
}

/// Silence inserted after a chunk, on top of what the model produced.
fn pause_after(chunk: &PlannedChunk, speed: f32) -> Duration {
    let millis = match chunk.boundary {
        Boundary::Clause => {
            let ends_with_punctuation = chunk.text.chars().next_back().is_some_and(|c| !c.is_alphanumeric());
            if ends_with_punctuation {
                130.0
            } else {
                20.0
            }
        }
        Boundary::Sentence => 280.0,
        Boundary::Paragraph => 550.0,
        // A short tail, so the last sound is not cut when the stream closes.
        Boundary::End => 150.0,
    };
    Duration::from_secs_f32(millis / 1000.0 / speed.max(0.25))
}

/// Trims leading and trailing silence, which varies from chunk to chunk, so
/// the pauses between chunks are the deliberate ones from [`pause_after`].
/// Short fades keep the cut edges click-free.
pub fn trim_silence(mut audio: Audio) -> Audio {
    let rate = audio.sample_rate as usize;
    let peak = audio.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    let threshold = (peak * 0.02).max(1e-4);
    let Some(first) = audio.samples.iter().position(|s| s.abs() > threshold) else {
        audio.samples.clear();
        return audio;
    };
    let last = audio.samples.iter().rposition(|s| s.abs() > threshold).unwrap_or(first);
    let start = first.saturating_sub(rate * 15 / 1000);
    let end = (last + 1 + rate * 40 / 1000).min(audio.samples.len());
    audio.samples.truncate(end);
    audio.samples.drain(..start);

    let fade_in = (rate * 4 / 1000).min(audio.samples.len() / 2);
    for (i, sample) in audio.samples[..fade_in].iter_mut().enumerate() {
        *sample *= i as f32 / fade_in as f32;
    }
    let fade_out = (rate * 10 / 1000).min(audio.samples.len() / 2);
    let len = audio.samples.len();
    for (i, sample) in audio.samples[len - fade_out..].iter_mut().enumerate() {
        *sample *= 1.0 - (i + 1) as f32 / fade_out as f32;
    }
    audio
}
