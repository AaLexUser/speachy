//! Playing audio faster or slower without changing its pitch.
//!
//! WSOLA (waveform similarity overlap-add): the output is built from
//! windowed frames laid down half a frame apart. Each frame is taken from
//! the input near the position the speed asks for, at the spot that best
//! continues the frame before it, so the overlapping halves agree and no
//! period of the voice is cut in two.

use std::ops::RangeInclusive;

/// The speeds audio can be played at.
pub const SPEEDS: RangeInclusive<f32> = 0.25..=4.0;

/// `speed` made playable: within [`SPEEDS`], and normal if it is no number.
pub fn playable(speed: f32) -> f32 {
    if speed.is_nan() {
        1.0
    } else {
        speed.clamp(*SPEEDS.start(), *SPEEDS.end())
    }
}

/// Plays samples back faster or slower without changing their pitch.
pub struct Stretcher {
    /// The input, followed by enough silence that every frame read is whole.
    samples: Vec<f32>,
    /// Length of the input without that silence.
    len: usize,
    /// Half a frame: the distance between frames in the output, and the
    /// length of one block.
    hop: usize,
    /// How far from its ideal position a frame may be taken.
    reach: usize,
    /// The rising half of a periodic Hann window; the falling half is
    /// `1 - rise`, so overlapping halves add up to exactly one.
    rise: Vec<f32>,
    /// Where in the input the next frame ideally starts.
    ideal: f64,
    /// Where the next frame starts if it continues the last one naturally.
    natural: usize,
    /// The second half of the last frame, faded out, still to be overlapped.
    tail: Vec<f32>,
}

impl Stretcher {
    pub fn new(mut samples: Vec<f32>, sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let hop = ((rate * 0.015) as usize).max(1);
        let reach = (rate * 0.012) as usize;
        let len = samples.len();
        samples.resize(len + 3 * hop + reach, 0.0);
        let rise: Vec<f32> = (0..hop)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / hop as f32).cos())
            .collect();
        // As if a frame ending where the input starts had been played: the
        // first block is then the input itself.
        let tail = samples[..hop]
            .iter()
            .zip(&rise)
            .map(|(s, r)| s * (1.0 - r))
            .collect();
        Self {
            samples,
            len,
            hop,
            reach,
            rise,
            ideal: 0.0,
            natural: 0,
            tail,
        }
    }

    /// How far into the input the output has got, in samples: the end of
    /// the last block's frame.
    pub fn position(&self) -> usize {
        self.natural
    }

    /// The most samples one block appends.
    pub fn block_len(&self) -> usize {
        self.hop
    }

    /// Appends the next block of output to `out`, played at `speed`. Returns
    /// false when the samples are used up. `speed` may differ on every call.
    pub fn next_block(&mut self, speed: f32, out: &mut Vec<f32>) -> bool {
        let speed = playable(speed) as f64;
        if self.ideal >= self.len as f64 {
            return false;
        }
        if speed == 1.0 {
            self.ideal = self.natural as f64;
        }
        let left = self.len as f64 - self.ideal;
        if left <= 0.0 {
            return false;
        }
        let at = if speed == 1.0 {
            self.natural
        } else {
            self.best_frame()
        };
        let hop = self.hop;
        let (first, second) = self.samples[at..at + 2 * hop].split_at(hop);
        let count = ((left / speed).ceil() as usize).min(hop);
        out.extend((0..count).map(|i| self.tail[i] + first[i] * self.rise[i]));
        for ((tail, s), r) in self.tail.iter_mut().zip(second).zip(&self.rise) {
            *tail = s * (1.0 - r);
        }
        self.natural = at + hop;
        // A short block plays the last of the input: nothing may follow it,
        // even if a later block at normal speed would start further back.
        self.ideal = if count < hop {
            self.len as f64
        } else {
            self.ideal + hop as f64 * speed
        };
        true
    }

    /// The start of the frame near the ideal position that is most like the
    /// natural continuation of the last frame, by normalized correlation:
    /// a loud candidate does not win for being loud. Ties go to the one
    /// nearest the natural continuation: in silence every candidate ties,
    /// and going back towards the ideal position would replay speech that
    /// was already heard.
    fn best_frame(&self) -> usize {
        let frame = 2 * self.hop;
        let ideal = self.ideal.round() as usize;
        let lo = ideal.saturating_sub(self.reach);
        let hi = ideal + self.reach;
        let want = &self.samples[self.natural..self.natural + frame];
        let score = |at: usize| {
            let candidate = &self.samples[at..at + frame];
            dot(candidate, want) / (dot(candidate, candidate).sqrt() + 1e-6)
        };
        let nearest = self.natural.clamp(lo, hi);
        let mut best = (nearest, score(nearest));
        for at in lo..=hi {
            let s = score(at);
            if s > best.1 {
                best = (at, s);
            }
        }
        best.0
    }
}

/// Written with independent sums so the compiler can vectorize it.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (a8, a_rest) = a.as_chunks::<8>();
    let (b8, b_rest) = b.as_chunks::<8>();
    let mut sums = [0f32; 8];
    for (x, y) in a8.iter().zip(b8) {
        for i in 0..8 {
            sums[i] += x[i] * y[i];
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    sums.iter().sum::<f32>() + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stretches `samples`, asking `speed` for the speed of every block
    /// with the output so far.
    fn stretch(samples: &[f32], rate: u32, mut speed: impl FnMut(&[f32]) -> f32) -> Vec<f32> {
        let mut stretcher = Stretcher::new(samples.to_vec(), rate);
        let mut out = Vec::new();
        let mut blocks = 0;
        while stretcher.next_block(speed(&out), &mut out) {
            blocks += 1;
            assert!(blocks < 10_000_000, "never ends");
        }
        out
    }

    fn sine(hz: f32, secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut state = 0x2545_f491u32;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    fn frame(rate: u32) -> usize {
        2 * (rate as f32 * 0.015) as usize
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Cycles per second, from the zero crossings of a sine.
    fn pitch(x: &[f32], rate: u32) -> f32 {
        let crossings = x
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / 2.0 / (x.len() as f32 / rate as f32)
    }

    fn largest_step(x: &[f32]) -> f32 {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn normal_speed_returns_the_input() {
        for rate in [24_000, 44_100] {
            let mut input = noise(rate as usize);
            input.truncate(input.len() - 7);
            let out = stretch(&input, rate, |_| 1.0);
            assert_eq!(out.len(), input.len(), "{rate} Hz");
            for (i, (a, b)) in out.iter().zip(&input).enumerate() {
                assert!((a - b).abs() < 1e-4, "{rate} Hz, sample {i}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn length_is_divided_by_the_speed() {
        let rate = 24_000;
        let input = noise(2 * rate as usize);
        for speed in [0.5, 0.75, 1.5, 2.0, 2.5] {
            let out = stretch(&input, rate, |_| speed);
            let want = input.len() as f32 / speed;
            assert!(
                (out.len() as f32 - want).abs() <= frame(rate) as f32,
                "speed {speed}: {} samples, expected {want}",
                out.len()
            );
        }
    }

    #[test]
    fn pitch_and_loudness_are_kept() {
        let rate = 24_000;
        let input = sine(200.0, 2.0, rate);
        for speed in [0.75, 1.5, 2.0] {
            let out = stretch(&input, rate, |_| speed);
            let hz = pitch(&out, rate);
            assert!((hz - 200.0).abs() <= 6.0, "speed {speed}: {hz} Hz");
            let ratio = rms(&out) / rms(&input);
            assert!(
                (0.85..=1.15).contains(&ratio),
                "speed {speed}: rms x{ratio}"
            );
        }
    }

    #[test]
    fn no_holes_or_clicks_at_fixed_or_changing_speeds() {
        let rate = 24_000;
        let input = sine(200.0, 2.0, rate);
        let limit = 2.0 * largest_step(&input);
        let changing = [1.0, 1.7, 0.6, 2.5];
        let mut block = 0;
        let runs: [(&str, Vec<f32>); 4] = [
            ("0.6", stretch(&input, rate, |_| 0.6)),
            ("1.7", stretch(&input, rate, |_| 1.7)),
            ("2.5", stretch(&input, rate, |_| 2.5)),
            (
                "changing every block",
                stretch(&input, rate, |_| {
                    block += 1;
                    changing[block % changing.len()]
                }),
            ),
        ];
        for (name, out) in runs {
            let step = largest_step(&out);
            assert!(step <= limit, "{name}: step {step}, limit {limit}");
        }
    }

    #[test]
    fn a_change_takes_effect_at_once() {
        let rate = 24_000;
        let input = noise(2 * rate as usize);
        let half = input.len() / 2;
        let out = stretch(&input, rate, |out| if out.len() < half { 1.0 } else { 2.0 });
        let want = 1.5 * rate as f32;
        assert!(
            (out.len() as f32 - want).abs() <= frame(rate) as f32,
            "{} samples, expected {want}",
            out.len()
        );
    }

    #[test]
    fn voice_followed_by_silence_is_not_heard_again_after_it() {
        // A steady voice that fades out, then a pause, as a chunk is played.
        let mut input: Vec<f32> = (0..1000)
            .map(|i| 0.02 * ((1000 - i) as f32 / 10.0).min(1.0))
            .collect();
        input.resize(1280, 0.0);
        let audible = |s: &f32| s.abs() > 0.001;
        for speed in [0.5, 0.6, 0.75, 1.5] {
            let out = stretch(&input, 1000, |_| speed);
            let last_voice = out.iter().rposition(audible).unwrap();
            let first_silence = out.iter().position(|s| !audible(s)).unwrap();
            assert!(
                first_silence > last_voice,
                "speed {speed}: voice at {last_voice}, after silence at {first_silence}"
            );
        }
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let short = noise(100);
        for input in [Vec::new(), vec![0.3], short] {
            for speed in [f32::NAN, 0.0, -1.0, 1e9, f32::INFINITY, 1.0] {
                let out = stretch(&input, 24_000, |_| speed);
                assert!(out.len() <= input.len() * 4 + 1, "speed {speed}");
                assert!(out.iter().all(|s| s.is_finite()));
            }
        }
        for rate in [0, 1, 7] {
            stretch(&noise(50), rate, |_| 1.5);
        }
    }

    /// Run with `cargo test -p speachy-core --release stretch_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stretch_cost() {
        let rate = 44_100;
        let input: Vec<f32> = noise(60 * rate as usize)
            .iter()
            .zip(sine(200.0, 60.0, rate))
            .map(|(n, s)| 0.3 * n + s)
            .collect();
        let started = std::time::Instant::now();
        let out = stretch(&input, rate, |_| 1.5);
        let took = started.elapsed();
        println!(
            "60 s of 44.1 kHz at 1.5: {took:?} ({:.2} % of real time), {} samples out",
            took.as_secs_f64() / 60.0 * 100.0,
            out.len()
        );
    }
}
