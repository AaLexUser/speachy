//! Audio output: a gapless queue of synthesized chunks.

use crate::engine::Audio;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Playback progress of a queued chunk, reported from the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayEvent {
    /// The first sample of chunk `index` reached the output.
    Started(usize),
    /// Chunk `index` and the pause after it have been played.
    Finished(usize),
}

pub type PlayCallback = Arc<dyn Fn(PlayEvent) + Send + Sync>;

/// Opens output streams. One stream is opened per utterance, so the
/// current default output device is picked up each time.
pub trait AudioBackend: Send + Sync {
    fn open(&self, on_event: PlayCallback) -> Result<Box<dyn AudioStream>>;
}

/// A playing queue. Dropping it stops playback immediately.
///
/// Streams are not `Send`: on some platforms the OS audio handle must stay
/// on the thread that created it.
pub trait AudioStream {
    /// Queues `audio` followed by `pause_after` of silence. Chunks play in
    /// the order pushed, back to back.
    fn push(&mut self, index: usize, audio: Audio, pause_after: Duration);
    fn set_paused(&mut self, paused: bool);
    fn set_volume(&mut self, volume: f32);
}

/// A chunk as an audio source that reports when it starts and ends.
struct ChunkSource {
    index: usize,
    samples: std::vec::IntoIter<f32>,
    silence_left: usize,
    sample_rate: u32,
    started: bool,
    finished: bool,
    /// Report `Finished` as soon as the last sample is pulled. The capture
    /// backend turns this off to record the chunk first.
    auto_finish: bool,
    on_event: PlayCallback,
}

impl ChunkSource {
    fn new(index: usize, audio: Audio, pause_after: Duration, on_event: PlayCallback) -> Self {
        let silence_left = (pause_after.as_secs_f64() * audio.sample_rate as f64) as usize;
        Self {
            index,
            sample_rate: audio.sample_rate,
            samples: audio.samples.into_iter(),
            silence_left,
            started: false,
            finished: false,
            auto_finish: true,
            on_event,
        }
    }

    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            (self.on_event)(PlayEvent::Finished(self.index));
        }
    }
}

impl Iterator for ChunkSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if !self.started {
            self.started = true;
            (self.on_event)(PlayEvent::Started(self.index));
        }
        if let Some(sample) = self.samples.next() {
            return Some(sample);
        }
        if self.silence_left > 0 {
            self.silence_left -= 1;
            return Some(0.0);
        }
        if self.auto_finish {
            self.finish();
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.samples.len() + self.silence_left;
        (left, Some(left))
    }
}

impl rodio::Source for ChunkSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::MIN
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(self.sample_rate.max(1)).expect("non-zero sample rate")
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Plays through the system's default output device.
#[derive(Default)]
pub struct SystemAudio;

struct SystemStream {
    player: rodio::Player,
    // Declared after `player` so the device outlives the queue feeding it.
    _device: rodio::MixerDeviceSink,
    on_event: PlayCallback,
}

impl AudioBackend for SystemAudio {
    fn open(&self, on_event: PlayCallback) -> Result<Box<dyn AudioStream>> {
        let mut device = crate::output::open_default()?;
        device.log_on_drop(false);
        let player = rodio::Player::connect_new(device.mixer());
        Ok(Box::new(SystemStream {
            player,
            _device: device,
            on_event,
        }))
    }
}

impl AudioStream for SystemStream {
    fn push(&mut self, index: usize, audio: Audio, pause_after: Duration) {
        self.player.append(ChunkSource::new(
            index,
            audio,
            pause_after,
            self.on_event.clone(),
        ));
    }

    fn set_paused(&mut self, paused: bool) {
        if paused {
            self.player.pause();
        } else {
            self.player.play();
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.player.set_volume(volume.clamp(0.0, 2.0));
    }
}

/// A backend without a sound card: it consumes the queue on a timer and
/// keeps everything that was "played". Used by tests and by the CLI to
/// render speech into a file.
#[derive(Clone)]
pub struct CaptureAudio {
    /// How much faster than real time the queue is consumed. `f32::INFINITY`
    /// consumes instantly.
    pub time_scale: f32,
    /// Everything played so far, in order, with its sample rate.
    pub played: Arc<Mutex<Vec<Audio>>>,
    /// Whether played audio is kept in `played`.
    pub keep: bool,
}

impl CaptureAudio {
    pub fn new(time_scale: f32) -> Self {
        Self {
            time_scale,
            played: Arc::default(),
            keep: true,
        }
    }

    /// A sound card that plays at `time_scale` and remembers nothing: for
    /// long-running headless sessions.
    pub fn discarding(time_scale: f32) -> Self {
        Self {
            keep: false,
            ..Self::new(time_scale)
        }
    }

    /// All captured audio joined together. Chunks are expected to share a
    /// sample rate; the first chunk's rate is reported.
    pub fn take(&self) -> Audio {
        let chunks = std::mem::take(&mut *self.played.lock().unwrap());
        let sample_rate = chunks.first().map_or(24_000, |a| a.sample_rate);
        Audio {
            samples: chunks.into_iter().flat_map(|a| a.samples).collect(),
            sample_rate,
        }
    }
}

/// The clock of the virtual sound card: the time at which the audio
/// consumed so far has finished "playing".
///
/// Waiting until that time, rather than for the length of each block, is
/// what keeps playback at the intended pace: a thread never sleeps exactly
/// as long as asked, and on a busy machine it oversleeps by more than a
/// block. Time lost that way is made up on the following blocks.
struct Pacer {
    played_until: Instant,
}

impl Pacer {
    fn new(now: Instant) -> Self {
        Self { played_until: now }
    }

    /// Records that `audio` more was consumed at time `now`. Returns how
    /// long to wait before consuming more; nothing when running late.
    fn consumed(&mut self, audio: Duration, now: Instant) -> Duration {
        self.played_until += audio;
        self.played_until.saturating_duration_since(now)
    }

    /// Playback stood still (paused, or nothing was queued): time that
    /// passed meanwhile is not to be made up.
    fn resume(&mut self, now: Instant) {
        self.played_until = self.played_until.max(now);
    }
}

struct CaptureStream {
    queue: std::sync::mpsc::Sender<ChunkSource>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    on_event: PlayCallback,
}

impl AudioBackend for CaptureAudio {
    fn open(&self, on_event: PlayCallback) -> Result<Box<dyn AudioStream>> {
        let (queue, incoming) = std::sync::mpsc::channel::<ChunkSource>();
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (paused_flag, stop_flag) = (paused.clone(), stop.clone());
        let played = self.played.clone();
        let time_scale = self.time_scale;
        let keep = self.keep;
        // Stand-in for the device callback thread.
        std::thread::spawn(move || {
            let mut pacer = Pacer::new(Instant::now());
            loop {
                let mut source = match incoming.try_recv() {
                    Ok(source) => source,
                    Err(TryRecvError::Disconnected) => return,
                    // Nothing queued: playback stands still until there is.
                    Err(TryRecvError::Empty) => match incoming.recv() {
                        Ok(source) => {
                            pacer.resume(Instant::now());
                            source
                        }
                        Err(_) => return,
                    },
                };
                let sample_rate = source.sample_rate;
                let mut samples = Vec::with_capacity(source.size_hint().0);
                // Consume in 10 ms blocks, like a device callback would.
                let block = (sample_rate as usize / 100).max(1);
                let mut was_paused = false;
                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        return;
                    }
                    if paused_flag.load(Ordering::Relaxed) {
                        was_paused = true;
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    if std::mem::take(&mut was_paused) {
                        pacer.resume(Instant::now());
                    }
                    let before = samples.len();
                    samples.extend(source.by_ref().take(block));
                    let got = samples.len() - before;
                    if time_scale.is_finite() && got > 0 {
                        let audio = got as f64 / sample_rate as f64 / time_scale as f64;
                        let wait = pacer.consumed(Duration::from_secs_f64(audio), Instant::now());
                        std::thread::sleep(wait);
                    }
                    if got < block {
                        break;
                    }
                }
                // Record before reporting the end, so a listener that reacts
                // to `Finished` already sees the audio.
                if keep {
                    played.lock().unwrap().push(Audio {
                        samples,
                        sample_rate,
                    });
                }
                source.finish();
            }
        });
        Ok(Box::new(CaptureStream {
            queue,
            paused,
            stop,
            on_event,
        }))
    }
}

impl AudioStream for CaptureStream {
    fn push(&mut self, index: usize, audio: Audio, pause_after: Duration) {
        let mut source = ChunkSource::new(index, audio, pause_after, self.on_event.clone());
        source.auto_finish = false;
        let _ = self.queue.send(source);
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    fn set_volume(&mut self, _volume: f32) {}
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: Duration = Duration::from_millis(10);

    /// Consumes `blocks` blocks on a machine where every sleep lasts
    /// `overshoot` longer than asked. Returns how long that took.
    fn play(blocks: u32, overshoot: Duration) -> Duration {
        let start = Instant::now();
        let mut now = start;
        let mut pacer = Pacer::new(start);
        for _ in 0..blocks {
            let wait = pacer.consumed(BLOCK, now);
            if !wait.is_zero() {
                now += wait + overshoot;
            }
        }
        now - start
    }

    #[test]
    fn late_wakeups_are_made_up() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        // On time: wait a whole block.
        assert_eq!(pacer.consumed(BLOCK, start), BLOCK);
        // Woken 7 ms late: the next wait is 7 ms shorter.
        let late = start + BLOCK + Duration::from_millis(7);
        assert_eq!(pacer.consumed(BLOCK, late), Duration::from_millis(3));
        // Woken later than a whole block: no wait at all.
        let very_late = start + Duration::from_millis(45);
        assert_eq!(pacer.consumed(BLOCK, very_late), Duration::ZERO);
    }

    #[test]
    fn one_second_of_audio_takes_one_second_however_badly_sleeps_overshoot() {
        for overshoot_ms in [0, 1, 7, 25, 60] {
            let overshoot = Duration::from_millis(overshoot_ms);
            let took = play(100, overshoot);
            // Never early, and late by one overshoot at most.
            assert!(
                took >= Duration::from_millis(990),
                "{overshoot_ms} ms: took {took:?}"
            );
            assert!(
                took <= Duration::from_secs(1) + overshoot,
                "{overshoot_ms} ms: took {took:?}"
            );
        }
    }

    #[test]
    fn time_spent_standing_still_is_not_made_up() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        pacer.consumed(BLOCK, start);
        // Paused for a second: playback continues from now, at normal pace.
        let later = start + Duration::from_secs(1);
        pacer.resume(later);
        assert_eq!(pacer.consumed(BLOCK, later), BLOCK);
        // Resuming while still ahead of the clock changes nothing.
        let mut pacer = Pacer::new(start);
        pacer.consumed(BLOCK, start);
        pacer.resume(start + Duration::from_millis(4));
        assert_eq!(
            pacer.consumed(BLOCK, start + Duration::from_millis(4)),
            Duration::from_millis(16)
        );
    }
}
