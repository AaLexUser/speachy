//! Behaviour of the streaming pipeline, with a fake engine and a virtual
//! sound card.

use speachy_core::audio::CaptureAudio;
use speachy_core::engine::{Audio, EngineKey, EngineProvider, SynthRequest, Synthesizer};
use speachy_core::pipeline::{
    loudness_over_time, Outcome, SpeakOptions, Speaker, SpeechEvent, State, LEVEL_WINDOW,
};
use speachy_core::plan::PlannedChunk;
use speachy_core::text::Boundary;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RATE: u32 = 1000;

/// Produces `secs / speed` seconds of a constant level that encodes the chunk
/// number ("chunk-3" → 0.04), so played audio can be attributed.
struct FakeEngine {
    secs: f32,
    delay: Duration,
    /// For every call: (chunk number, chunks that had started playing).
    calls: Mutex<Vec<(usize, usize)>>,
    /// Calls that have returned.
    returned: AtomicUsize,
    started_playing: Arc<AtomicUsize>,
    loaded: AtomicBool,
}

fn level(n: usize) -> f32 {
    (n + 1) as f32 / 100.0
}

impl Synthesizer for FakeEngine {
    fn synthesize(
        &self,
        req: &SynthRequest,
        cancel: &Arc<AtomicBool>,
    ) -> anyhow::Result<Option<Audio>> {
        let n: usize = req.text.rsplit('-').next().unwrap().parse().unwrap_or(0);
        self.calls
            .lock()
            .unwrap()
            .push((n, self.started_playing.load(Ordering::SeqCst)));
        std::thread::sleep(self.delay);
        self.returned.fetch_add(1, Ordering::SeqCst);
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if req.text.starts_with("bad") {
            anyhow::bail!("cannot pronounce {}", req.text);
        }
        Ok(Some(Audio {
            samples: vec![level(n); (self.secs * RATE as f32 / req.speed) as usize],
            sample_rate: RATE,
        }))
    }
}

struct Engines(Arc<FakeEngine>);

impl EngineProvider for Engines {
    fn engine(&self, _: &EngineKey) -> anyhow::Result<Arc<dyn Synthesizer>> {
        self.0.loaded.store(true, Ordering::SeqCst);
        Ok(self.0.clone())
    }

    fn is_loaded(&self, _: &EngineKey) -> bool {
        self.0.loaded.load(Ordering::SeqCst)
    }
}

struct Rig {
    speaker: Speaker,
    engine: Arc<FakeEngine>,
    audio: CaptureAudio,
    events: Receiver<SpeechEvent>,
}

fn rig(secs: f32, delay_ms: u64, time_scale: f32) -> Rig {
    let started_playing = Arc::new(AtomicUsize::new(0));
    let engine = Arc::new(FakeEngine {
        secs,
        delay: Duration::from_millis(delay_ms),
        calls: Mutex::default(),
        returned: AtomicUsize::new(0),
        started_playing: started_playing.clone(),
        loaded: AtomicBool::new(false),
    });
    let audio = CaptureAudio::new(time_scale);
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let speaker = Speaker::new(
        Arc::new(Engines(engine.clone())),
        Arc::new(audio.clone()),
        move |event| {
            if matches!(event, SpeechEvent::ChunkPlaying { .. }) {
                started_playing.fetch_add(1, Ordering::SeqCst);
            }
            let _ = tx.lock().unwrap().send(event.clone());
        },
    );
    Rig {
        speaker,
        engine,
        audio,
        events,
    }
}

fn plan(prefix: &str, count: usize) -> Vec<PlannedChunk> {
    (0..count)
        .map(|n| PlannedChunk {
            text: format!("{prefix}-{n}"),
            boundary: if n + 1 == count {
                Boundary::End
            } else {
                Boundary::Sentence
            },
            lang: "en".into(),
            engine: EngineKey {
                model_id: "fake".into(),
                variant: None,
            },
            sid: 0,
            model_id: "fake".into(),
            voice_id: "v".into(),
        })
        .collect()
}

fn opts(lookahead_secs: f32) -> SpeakOptions {
    SpeakOptions {
        speed: 1.0,
        volume: 1.0,
        lookahead: Duration::from_secs_f32(lookahead_secs),
    }
}

impl Rig {
    /// Collects events until the given session finishes.
    fn until_finished(&self, session: u64) -> Vec<SpeechEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self
                .events
                .recv_timeout(Duration::from_secs(20))
                .expect("pipeline stalled");
            let done = matches!(&event, SpeechEvent::Finished { session: s, .. } if *s == session);
            seen.push(event);
            if done {
                // The closing state change follows the Finished event.
                if let Ok(event) = self.events.recv_timeout(Duration::from_secs(2)) {
                    seen.push(event);
                }
                return seen;
            }
        }
    }

    fn wait_for(&self, pred: impl Fn(&SpeechEvent) -> bool) -> Vec<SpeechEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self
                .events
                .recv_timeout(Duration::from_secs(20))
                .expect("pipeline stalled");
            let hit = pred(&event);
            seen.push(event);
            if hit {
                return seen;
            }
        }
    }
}

fn playing_order(events: &[SpeechEvent]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            SpeechEvent::ChunkPlaying { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

fn states(events: &[SpeechEvent]) -> Vec<State> {
    events
        .iter()
        .filter_map(|e| match e {
            SpeechEvent::State { state } => Some(*state),
            _ => None,
        })
        .collect()
}

fn outcome(events: &[SpeechEvent], session: u64) -> Option<(Outcome, Option<String>)> {
    events.iter().find_map(|e| match e {
        SpeechEvent::Finished {
            session: s,
            outcome,
            message,
        } if *s == session => Some((*outcome, message.clone())),
        _ => None,
    })
}

#[test]
fn plays_every_chunk_in_order_with_only_the_intended_pauses() {
    let rig = rig(0.5, 0, f32::INFINITY);
    let session = rig.speaker.speak(plan("chunk", 5), opts(20.0));
    let events = rig.until_finished(session);

    assert!(matches!(&events[0], SpeechEvent::Started { chunks, .. } if chunks.len() == 5));
    assert_eq!(playing_order(&events), [0, 1, 2, 3, 4]);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(states(&events).last(), Some(&State::Idle));
    assert_eq!(rig.speaker.state(), State::Idle);
    assert!(!rig.speaker.is_active());

    // The played signal is chunk, pause, chunk, pause, … with nothing else:
    // 500 samples of the chunk's level, then exactly the planned silence.
    let played = rig.audio.take();
    let mut runs: Vec<(bool, usize, f32)> = Vec::new(); // (silent, length, peak)
    for &sample in &played.samples {
        let silent = sample == 0.0;
        match runs.last_mut() {
            Some((s, len, peak)) if *s == silent => {
                *len += 1;
                *peak = peak.max(sample);
            }
            _ => runs.push((silent, 1, sample)),
        }
    }
    // Fades zero the very first and last sample of a chunk, so those read
    // as silence: one leading sample, and one or two extra per pause.
    assert_eq!(runs.remove(0), (true, 1, 0.0));
    let voiced: Vec<_> = runs.iter().filter(|r| !r.0).collect();
    assert_eq!(voiced.len(), 5, "{runs:?}");
    for (n, run) in voiced.iter().enumerate() {
        assert!((498..=500).contains(&run.1), "chunk {n} length {}", run.1);
        assert!(
            (run.2 - level(n)).abs() < 1e-6,
            "chunk {n} carries the wrong audio"
        );
    }
    let pauses: Vec<usize> = runs.iter().filter(|r| r.0).map(|r| r.1).collect();
    // 280 ms after a sentence, 150 ms at the end (plus the faded samples).
    assert_eq!(pauses.len(), 5, "{runs:?}");
    assert!(
        pauses[..4].iter().all(|&p| (280..=282).contains(&p)),
        "{pauses:?}"
    );
    assert!((150..=152).contains(&pauses[4]), "{pauses:?}");
}

#[test]
fn synthesis_runs_ahead_of_playback_but_stops_at_the_lookahead() {
    // 2 s chunks played at 10x: one chunk per 200 ms. Synthesis is instant.
    let rig = rig(2.0, 0, 10.0);
    let session = rig.speaker.speak(plan("chunk", 12), opts(5.0));
    let events = rig.until_finished(session);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(playing_order(&events), (0..12).collect::<Vec<_>>());

    let calls = rig.engine.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 12);
    // With 5 s of look-ahead and 2 s chunks, at most 2 finished chunks may
    // be waiting when the next one is started.
    let ahead: Vec<usize> = calls
        .iter()
        .map(|&(n, started)| n.saturating_sub(started))
        .collect();
    assert!(
        ahead.iter().all(|&a| a <= 2),
        "ran too far ahead: {ahead:?}"
    );
    assert!(ahead.iter().any(|&a| a >= 1), "never ran ahead: {ahead:?}");

    // Playback never ran dry: once speaking, it stayed speaking.
    let states = states(&events);
    let first_speaking = states.iter().position(|s| *s == State::Speaking).unwrap();
    assert_eq!(
        &states[first_speaking..],
        [State::Speaking, State::Idle],
        "{states:?}"
    );
}

#[test]
fn slow_synthesis_is_reported_as_preparing_between_chunks() {
    // 0.2 s chunks at 4x take about 120 ms to play (with the pause after
    // them) but 500 ms to synthesize.
    let rig = rig(0.2, 500, 4.0);
    let session = rig.speaker.speak(plan("chunk", 3), opts(5.0));
    let events = rig.until_finished(session);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    let states = states(&events);
    assert!(
        states
            .windows(2)
            .any(|w| w == [State::Speaking, State::Preparing]),
        "{states:?}"
    );
}

#[test]
fn stop_is_prompt_and_abandons_the_rest() {
    let rig = rig(1.0, 5, 1.0); // real time: 50 s of speech
    let session = rig.speaker.speak(plan("chunk", 50), opts(4.0));
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 0, .. }));

    let stopped_at = Instant::now();
    assert!(rig.speaker.stop());
    let events = rig.until_finished(session);
    assert!(
        stopped_at.elapsed() < Duration::from_millis(500),
        "stop took {:?}",
        stopped_at.elapsed()
    );
    assert_eq!(outcome(&events, session), Some((Outcome::Stopped, None)));
    assert_eq!(rig.speaker.state(), State::Idle);
    assert!(!rig.speaker.stop(), "nothing left to stop");

    std::thread::sleep(Duration::from_millis(150));
    let synthesized = rig.engine.calls.lock().unwrap().len();
    assert!(
        synthesized <= 6,
        "kept synthesizing after stop: {synthesized} chunks"
    );
}

#[test]
fn pause_holds_playback_and_resume_continues() {
    let rig = rig(0.5, 0, 10.0); // 50 ms per chunk
    let session = rig.speaker.speak(plan("chunk", 6), opts(20.0));
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 0, .. }));
    rig.speaker.toggle_pause();
    rig.wait_for(|e| {
        matches!(
            e,
            SpeechEvent::State {
                state: State::Paused
            }
        )
    });
    assert_eq!(rig.speaker.state(), State::Paused);

    // While paused nothing new starts playing (at most the chunk that was
    // already crossing the boundary).
    std::thread::sleep(Duration::from_millis(300));
    let during: Vec<SpeechEvent> = rig.events.try_iter().collect();
    assert!(playing_order(&during).len() <= 1, "{during:?}");

    rig.speaker.toggle_pause();
    let events = rig.until_finished(session);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(playing_order(&events).last(), Some(&5));
}

#[test]
fn a_new_request_replaces_the_one_in_progress() {
    let rig = rig(1.0, 0, 1.0);
    let first = rig.speaker.speak(plan("first", 20), opts(3.0));
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 0, .. }));
    assert_eq!(
        rig.speaker.current_text().unwrap(),
        plan("first", 20)
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>()
            .join(" ")
    );

    rig.audio.take();
    let second = rig.speaker.speak(plan("second", 1), opts(3.0));
    let events = rig.until_finished(second);

    // The first session is reported stopped before the second one starts,
    // and nothing of it is heard or reported afterwards.
    let stopped = events.iter().position(|e| matches!(e, SpeechEvent::Finished { session, outcome: Outcome::Stopped, .. } if *session == first));
    let started = events
        .iter()
        .position(|e| matches!(e, SpeechEvent::Started { session, .. } if *session == second));
    assert!(stopped.unwrap() < started.unwrap());
    for event in &events[started.unwrap()..] {
        if let SpeechEvent::ChunkPlaying { session, .. } | SpeechEvent::ChunkReady { session, .. } =
            event
        {
            assert_eq!(*session, second);
        }
    }
    assert_eq!(outcome(&events, second), Some((Outcome::Completed, None)));
}

#[test]
fn a_failing_chunk_is_skipped() {
    let rig = rig(0.2, 0, f32::INFINITY);
    let mut chunks = plan("chunk", 3);
    chunks[1].text = "bad-1".into();
    let session = rig.speaker.speak(chunks, opts(20.0));
    let events = rig.until_finished(session);
    assert_eq!(playing_order(&events), [0, 2]);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
}

#[test]
fn failing_everything_is_reported() {
    let rig = rig(0.2, 0, f32::INFINITY);
    let session = rig.speaker.speak(plan("bad", 2), opts(20.0));
    let events = rig.until_finished(session);
    let (result, message) = outcome(&events, session).unwrap();
    assert_eq!(result, Outcome::Failed);
    assert!(message.unwrap().contains("cannot pronounce"));
    assert_eq!(rig.speaker.state(), State::Idle);
}

#[test]
fn loading_a_model_is_reported_before_the_first_chunk() {
    let rig = rig(0.2, 0, f32::INFINITY);
    let session = rig.speaker.speak(plan("chunk", 1), opts(20.0));
    let events = rig.until_finished(session);
    let states = states(&events);
    assert_eq!(
        states,
        [
            State::Preparing,
            State::Loading,
            State::Speaking,
            State::Idle
        ]
    );
}

#[test]
fn shutdown_waits_for_synthesis_in_flight() {
    // Each synthesis call takes 300 ms and cannot be interrupted.
    let rig = rig(1.0, 300, 1.0);
    rig.speaker.speak(plan("chunk", 5), opts(60.0));
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 0, .. }));

    // A call is in flight now. After shutdown none is, and none starts.
    assert!(rig.speaker.shutdown(Duration::from_secs(5)));
    let started = rig.engine.calls.lock().unwrap().len();
    assert_eq!(rig.engine.returned.load(Ordering::SeqCst), started);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(rig.engine.calls.lock().unwrap().len(), started);
    assert_eq!(rig.speaker.state(), State::Idle);

    // With nothing running it returns at once.
    assert!(rig.speaker.shutdown(Duration::ZERO));
}

#[test]
fn loudness_follows_the_audio() {
    let rate = 24_000usize;
    let tone = |amplitude: f32| (0..rate / 2).map(move |i| amplitude * (i as f32 * 0.3).sin());
    // Half a second each of silence, quiet speech and loud speech.
    let mut samples = vec![0.0; rate / 2];
    samples.extend(tone(0.1));
    samples.extend(tone(0.5));
    let levels = loudness_over_time(&Audio {
        samples,
        sample_rate: rate as u32,
    });
    // One value for every 40 ms; the last one covers what is left over.
    assert_eq!(levels.len(), 38);
    assert!(levels[..12].iter().all(|l| *l == 0), "{levels:?}");
    // A fifth as strong reads as about two fifths as loud.
    assert!(
        levels[13..25].iter().all(|l| (33..=43).contains(l)),
        "{levels:?}"
    );
    assert!(levels[26..37].iter().all(|l| *l >= 97), "{levels:?}");

    let silence = Audio {
        samples: vec![0.0; rate],
        sample_rate: rate as u32,
    };
    assert_eq!(loudness_over_time(&silence), vec![0; 25]);
    let nothing = Audio {
        samples: Vec::new(),
        sample_rate: rate as u32,
    };
    assert!(loudness_over_time(&nothing).is_empty());
}

#[test]
fn every_chunk_reports_its_loudness_over_time() {
    let rig = rig(0.5, 0, f32::INFINITY);
    let session = rig.speaker.speak(plan("chunk", 3), opts(20.0));
    let events = rig.until_finished(session);
    let mut reported = 0;
    for event in &events {
        if let SpeechEvent::ChunkReady {
            audio_secs, levels, ..
        } = event
        {
            reported += 1;
            let windows = (audio_secs / LEVEL_WINDOW.as_secs_f32()).ceil() as usize;
            assert_eq!(levels.len(), windows, "{audio_secs} s of audio");
            // The fake engine's audio is equally loud throughout, up to
            // the fades at its edges.
            assert!(
                levels[1..windows - 1].iter().all(|l| *l >= 95),
                "{levels:?}"
            );
        }
    }
    assert_eq!(reported, 3);
}

/// What was heard: for each chunk, in order, its number, its length in
/// samples and the silence after it.
fn heard(rig: &Rig) -> Vec<(usize, usize, usize)> {
    let played = rig.audio.take();
    let mut runs: Vec<(bool, usize, f32)> = Vec::new();
    for &sample in &played.samples {
        let silent = sample == 0.0;
        match runs.last_mut() {
            Some((s, len, peak)) if *s == silent => {
                *len += 1;
                *peak = peak.max(sample);
            }
            _ => runs.push((silent, 1, sample)),
        }
    }
    // The fade-in zeroes a chunk's first sample.
    assert_eq!(runs.first().map(|r| r.0), Some(true), "{runs:?}");
    runs[1..]
        .chunks(2)
        .map(|pair| {
            let (_, len, peak) = pair[0];
            let pause = pair.get(1).map_or(0, |p| p.1);
            ((peak * 100.0).round() as usize - 1, len, pause)
        })
        .collect()
}

/// Asserts that chunks were heard exactly as `expected` says: (chunk,
/// speed, milliseconds of pause after it), for 1 s chunks at 1000 Hz.
fn assert_heard(heard: &[(usize, usize, usize)], expected: &[(usize, f32, f32)]) {
    let chunks: Vec<usize> = heard.iter().map(|h| h.0).collect();
    let wanted: Vec<usize> = expected.iter().map(|e| e.0).collect();
    assert_eq!(chunks, wanted, "{heard:?}");
    for (&(n, len, pause), &(_, speed, pause_ms)) in heard.iter().zip(expected) {
        let want = (RATE as f32 / speed) as usize;
        // Fades zero a sample or two at each edge of a chunk.
        assert!(
            (want - 2..=want).contains(&len),
            "chunk {n}: {len} samples, expected {want} at speed {speed}: {heard:?}"
        );
        let want = (pause_ms / speed) as usize;
        assert!(
            (want..=want + 2).contains(&pause),
            "pause after chunk {n}: {pause}, expected {want}: {heard:?}"
        );
    }
}

fn speed_opts() -> SpeakOptions {
    opts(20.0)
}

#[test]
fn a_speed_change_during_speech_is_heard_from_the_next_chunk() {
    // 1 s chunks in real time, all five synthesized and queued at once.
    let rig = rig(1.0, 0, 1.0);
    let session = rig.speaker.speak(plan("chunk", 5), speed_opts());
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 1, .. }));
    rig.speaker.set_speed(2.0);
    let events = rig.until_finished(session);

    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(playing_order(&events), [2, 3, 4]);
    assert_heard(
        &heard(&rig),
        &[
            (0, 1.0, 280.0),
            (1, 1.0, 280.0),
            (2, 2.0, 280.0),
            (3, 2.0, 280.0),
            (4, 2.0, 150.0),
        ],
    );
    // The three queued chunks were synthesized again, and reported again.
    assert_eq!(rig.engine.calls.lock().unwrap().len(), 8);
    let ready: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            SpeechEvent::ChunkReady { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(ready, [2, 3, 4]);
}

#[test]
fn a_speed_change_before_anything_plays_starts_over_at_the_new_speed() {
    let rig = rig(1.0, 0, 1.0);
    let session = rig.speaker.speak(plan("chunk", 3), speed_opts());
    // Paused before the first chunk is ready: chunks are queued, none plays.
    rig.speaker.set_paused(true);
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkReady { index: 2, .. }));
    rig.speaker.set_speed(2.0);
    rig.speaker.set_paused(false);
    let events = rig.until_finished(session);

    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(playing_order(&events), [0, 1, 2]);
    assert_heard(
        &heard(&rig),
        &[(0, 2.0, 280.0), (1, 2.0, 280.0), (2, 2.0, 150.0)],
    );
}

#[test]
fn several_speed_changes_in_a_row_leave_the_last_one() {
    let rig = rig(1.0, 0, 1.0);
    let session = rig.speaker.speak(plan("chunk", 5), speed_opts());
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 1, .. }));
    rig.speaker.set_speed(1.5);
    rig.speaker.set_speed(0.5);
    rig.speaker.set_speed(2.0);
    let events = rig.until_finished(session);

    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_heard(
        &heard(&rig),
        &[
            (0, 1.0, 280.0),
            (1, 1.0, 280.0),
            (2, 2.0, 280.0),
            (3, 2.0, 280.0),
            (4, 2.0, 150.0),
        ],
    );
}

#[test]
fn a_speed_change_while_paused_is_heard_after_resuming() {
    let rig = rig(1.0, 0, 1.0);
    let session = rig.speaker.speak(plan("chunk", 4), speed_opts());
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 1, .. }));
    rig.speaker.set_paused(true);
    rig.wait_for(|e| {
        matches!(
            e,
            SpeechEvent::State {
                state: State::Paused
            }
        )
    });
    rig.speaker.set_speed(0.5);
    rig.speaker.set_paused(false);
    let events = rig.until_finished(session);

    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_heard(
        &heard(&rig),
        &[
            (0, 1.0, 280.0),
            (1, 1.0, 280.0),
            (2, 0.5, 280.0),
            (3, 0.5, 150.0),
        ],
    );
}

#[test]
fn a_speed_change_when_idle_or_to_the_same_speed_does_nothing() {
    let rig = rig(1.0, 0, 10.0);
    rig.speaker.set_speed(2.0);
    assert_eq!(rig.speaker.state(), State::Idle);

    let session = rig.speaker.speak(plan("chunk", 5), speed_opts());
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 0, .. }));
    rig.speaker.set_speed(1.0);
    let events = rig.until_finished(session);
    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(rig.engine.calls.lock().unwrap().len(), 5);
    assert_heard(
        &heard(&rig),
        &[
            (0, 1.0, 280.0),
            (1, 1.0, 280.0),
            (2, 1.0, 280.0),
            (3, 1.0, 280.0),
            (4, 1.0, 150.0),
        ],
    );
}

#[test]
fn a_chunk_skipped_before_and_after_a_speed_change_counts_once() {
    let rig = rig(1.0, 0, 1.0);
    let mut chunks = plan("chunk", 5);
    chunks[3].text = "bad-3".into();
    let session = rig.speaker.speak(chunks, speed_opts());
    rig.wait_for(|e| matches!(e, SpeechEvent::ChunkPlaying { index: 1, .. }));
    rig.speaker.set_speed(2.0);
    let events = rig.until_finished(session);

    assert_eq!(outcome(&events, session), Some((Outcome::Completed, None)));
    assert_eq!(playing_order(&events), [2, 4]);
    assert_heard(
        &heard(&rig),
        &[
            (0, 1.0, 280.0),
            (1, 1.0, 280.0),
            (2, 2.0, 280.0),
            (4, 2.0, 150.0),
        ],
    );
}
