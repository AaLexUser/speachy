//! How well captions follow real speech: word starts estimated from the
//! benchmark recordings, against the times a speech recognizer heard.
//!
//! The fixture is made by `scripts/make-caption-fixtures.py`.

use serde::Deserialize;
use speachy_core::captions;

#[derive(Deserialize)]
struct Recording {
    id: String,
    shown: String,
    spoken: String,
    levels: Vec<u8>,
    /// For every whitespace token of `shown`: when the recognizer heard it
    /// start, if it is scored.
    truth: Vec<Option<f32>>,
}

/// The estimated minus the heard start of every scored token: each token has
/// the start of the caption word it belongs to.
fn errors(recording: &Recording) -> Vec<f32> {
    let words = captions::words(&recording.shown, &recording.spoken, &recording.levels);
    let starts: Vec<f32> = words
        .iter()
        .flat_map(|word| std::iter::repeat_n(word.start_secs, word.text.split_whitespace().count()))
        .collect();
    assert_eq!(
        starts.len(),
        recording.truth.len(),
        "{}: the words do not cover the text",
        recording.id
    );
    starts
        .iter()
        .zip(&recording.truth)
        .filter_map(|(start, truth)| truth.map(|truth| start - truth))
        .collect()
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    let at = p * (sorted.len() - 1) as f32;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f32)
}

#[test]
fn benchmark_recordings_are_timed_within_bounds() {
    let fixture: Vec<Recording> =
        serde_json::from_str(include_str!("fixtures/captions.json")).unwrap();
    let errors: Vec<f32> = fixture.iter().flat_map(errors).collect();
    assert!(errors.len() > 500, "{} scored words", errors.len());
    // The recognizer hears a word some tens of milliseconds after it starts,
    // so the mean offset is taken out, as the design's tables do.
    let offset = errors.iter().sum::<f32>() / errors.len() as f32;
    let mut off: Vec<f32> = errors.iter().map(|e| (e - offset).abs()).collect();
    off.sort_by(f32::total_cmp);
    let typical = percentile(&off, 0.5) * 1000.0;
    let nine_of_ten = percentile(&off, 0.9) * 1000.0;
    println!(
        "{} words, mean offset {:.0} ms, typical {typical:.0} ms, 9 of 10 within {nine_of_ten:.0} ms",
        off.len(),
        offset * 1000.0
    );
    assert!(typical <= 60.0, "typical error {typical:.0} ms");
    assert!(
        nine_of_ten <= 160.0,
        "9 of 10 words within {nine_of_ten:.0} ms"
    );
}
