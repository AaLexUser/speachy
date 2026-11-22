//! Captions: the words of a chunk, when each starts to be heard, and the
//! line each is shown on.
//!
//! The start of each word is estimated from two things the engine already
//! has: the text the model read and the loudness of the chunk (see
//! [`crate::pipeline::loudness_over_time`]). Pauses in the loudness are put
//! at the word boundaries where the text expects them, and between two
//! pauses the time is shared out by letters.

use crate::pipeline::LEVEL_WINDOW;
use serde::Serialize;

/// A window with a lower level is silent: 3 % of the loudest window's
/// amplitude.
const QUIET: u8 = 12;
/// Windows. Shorter silences are the closures of "п", "т", "к", inside words.
const MIN_PAUSE: usize = 3;
/// What a word weighs on top of its letters, in letters.
const WORD: f64 = 1.5;
/// The cost of a pause at a boundary where the text has no punctuation.
const UNEXPECTED: f64 = 0.5;
/// Columns of a caption line: the usual length of a subtitle line.
const LINE: usize = 42;
/// Letters. A line should not end with a word this short.
const SHORT: usize = 3;
/// Characters at the end of a word after which the speech pauses.
const PAUSE_MARKS: &str = ",.;:!?…—–)»\"、。，！？；：";

/// A word of a chunk as captions show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    /// As written, with its punctuation: "млрд", "рублей.", "биологи —".
    pub text: String,
    /// When it starts: seconds into the chunk's audio at natural pace.
    pub start_secs: f32,
    /// The caption line it is on, counted from 0 within the chunk.
    pub line: usize,
}

/// The words of `shown`, timed against audio whose loudness over time is
/// `levels` (see [`crate::pipeline::loudness_over_time`]). `spoken` is the
/// text the model read. Starts never decrease and never exceed the length
/// that `levels` covers. Never panics: text and audio are untrusted.
pub fn words(shown: &str, spoken: &str, levels: &[u8]) -> Vec<Word> {
    let pieces = split(shown);
    let said = split(spoken);
    let keys = |pieces: &[Piece]| pieces.iter().map(|p| p.key.clone()).collect::<Vec<_>>();
    let matched = pair(&keys(&pieces), &keys(&said));
    let starts = spoken_starts(&said, levels);
    let total = levels.len() as f64 * LEVEL_WINDOW.as_secs_f64();

    // Every caption word: the pieces of `shown` it is made of, and the spoken
    // word it starts at. A run of pieces between two matched ones was
    // rewritten ("3,5 млрд" for "три с половиной миллиарда") and is one word.
    let mut groups: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        if let Some(j) = matched[i] {
            groups.push((i..i + 1, j));
            i += 1;
            continue;
        }
        let first = i;
        while i < pieces.len() && matched[i].is_none() {
            i += 1;
        }
        let after_previous = first
            .checked_sub(1)
            .and_then(|p| matched[p])
            .map_or(0, |j| j + 1);
        let next = matched.get(i).copied().flatten().unwrap_or(said.len());
        let start = if next > after_previous {
            after_previous
        } else {
            next
        };
        groups.push((first..i, start));
    }

    let texts: Vec<String> = groups
        .iter()
        .map(|(range, _)| {
            let (from, to) = (pieces[range.start].from, pieces[range.end - 1].to);
            shown[from..to]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    // A clause ends after a word if the spoken text has punctuation where the
    // next word starts.
    let ends: Vec<bool> = groups
        .iter()
        .enumerate()
        .map(|(g, _)| {
            groups
                .get(g + 1)
                .is_none_or(|(_, j)| *j > 0 && said.get(j - 1).is_some_and(|p| p.pause))
        })
        .collect();
    let line_of = lines(&texts, &ends);

    groups
        .iter()
        .zip(texts)
        .zip(line_of)
        .map(|(((_, j), text), line)| {
            let start = if starts.is_empty() {
                0.0
            } else {
                starts.get(*j).copied().unwrap_or(total)
            };
            Word {
                text,
                start_secs: start.clamp(0.0, total) as f32,
                line,
            }
        })
        .collect()
}

/// A word of a text before rewritten runs are joined: a byte range of the
/// text, with any tokens that have no letters or digits around it.
#[derive(Debug, Clone, PartialEq)]
struct Piece {
    from: usize,
    to: usize,
    key: String,
    /// The speech is expected to pause after it.
    pause: bool,
}

/// The words of `text`. A word is a run of characters between spaces; in a
/// run of Han or kana characters every character is a word. A token without
/// letters or digits ("—", "№") is not a word: it goes with the word before
/// it, or before the first word, with that word.
fn split(text: &str) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut lead: Option<usize> = None;
    for (from, token) in tokens(text) {
        let parts = segments(token);
        if parts.iter().all(|(a, b)| key(&token[*a..*b]).is_empty()) {
            match pieces.last_mut() {
                Some(last) => {
                    last.to = from + token.len();
                    last.pause = true;
                }
                None => {
                    lead.get_or_insert(from);
                }
            }
            continue;
        }
        for (a, b) in parts {
            let word = &token[a..b];
            pieces.push(Piece {
                from: lead.take().unwrap_or(from + a),
                to: from + b,
                key: key(word),
                pause: word
                    .chars()
                    .next_back()
                    .is_some_and(|c| PAUSE_MARKS.contains(c)),
            });
        }
    }
    pieces
}

/// The whitespace-separated tokens of `text`, with their byte offsets.
fn tokens(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split_whitespace()
        .map(move |token| (token.as_ptr() as usize - text.as_ptr() as usize, token))
}

/// Byte ranges of the words in one token: the token itself, unless it holds
/// Han or kana characters, each of which is a word of its own, followed by
/// any punctuation after it.
fn segments(token: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut has_word = false;
    let mut after_ideograph = false;
    for (at, c) in token.char_indices() {
        let ideograph = is_ideograph(c);
        let starts_word = c.is_alphanumeric() && (ideograph || after_ideograph);
        if starts_word && has_word {
            out.push((start, at));
            start = at;
        }
        if c.is_alphanumeric() {
            has_word = true;
            after_ideograph = ideograph;
        }
    }
    out.push((start, token.len()));
    out
}

/// Han and kana: written without spaces, one caption word per character.
fn is_ideograph(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF     // Hiragana, Katakana
        | 0x3400..=0x4DBF   // CJK extension A
        | 0x4E00..=0x9FFF   // CJK unified
        | 0xF900..=0xFAFF   // CJK compatibility
        | 0x20000..=0x3FFFF // CJK extensions B and later
    )
}

/// Letters and digits of a word, lower case, without stress marks, е for ё.
fn key(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

/// For every word of `shown`, the word of `spoken` it is, if any. Equal keys
/// are matched in order (longest common subsequence).
fn pair(shown: &[String], spoken: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (shown.len(), spoken.len());
    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if shown[i] == spoken[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let mut at = vec![None; n];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if shown[i] == spoken[j] && lcs[i * width + j] == lcs[(i + 1) * width + j + 1] + 1 {
            at[i] = Some(j);
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    at
}

/// (first window, window after the last) of every silence of at least
/// [`MIN_PAUSE`] windows with sound on both sides.
fn pauses(levels: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < levels.len() {
        if levels[i] >= QUIET {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < levels.len() && levels[j] < QUIET {
            j += 1;
        }
        if i > 0 && j < levels.len() && j - i >= MIN_PAUSE {
            out.push((i, j));
        }
        i = j;
    }
    out
}

/// Puts each pause at a word boundary. Returns, for every pause, the word
/// that starts after it.
///
/// * `before[b]`: the share of the text's weight before word `b`
///   (`before[0]` is 0, the last is 1);
/// * `expected[b]`: the text has punctuation before word `b`;
/// * `heard[g]`: the share of the sounding time before pause `g`.
///
/// A stretch of speech between two anchors should take the share of the
/// sounding time that its words have of the weight. The cost of a set of
/// choices is the sum over stretches of how far they are from that, plus
/// [`UNEXPECTED`] for each pause without punctuation. The cheapest set is
/// found in one pass over the pauses. There are never more pauses than
/// boundaries.
fn anchor(before: &[f64], expected: &[bool], heard: &[f64]) -> Vec<usize> {
    let n = before.len() - 1;
    let gaps = heard.len();
    if gaps == 0 || gaps >= n {
        return Vec::new();
    }
    let stretch = |b0: usize, s0: f64, b1: usize, s1: f64| {
        let share = before[b1] - before[b0];
        n as f64 * (s1 - s0 - share).powi(2) / share.max(1e-3)
    };
    let unexpected = |b: usize| if expected[b] { 0.0 } else { UNEXPECTED };
    // cost[g][b]: the cheapest way to put pauses 0..=g with pause g before
    // word b; from[g][b]: the word pause g - 1 was put before.
    let mut cost = vec![vec![f64::INFINITY; n + 1]; gaps];
    let mut from = vec![vec![0usize; n + 1]; gaps];
    for (b, slot) in cost[0].iter_mut().enumerate().take(n - (gaps - 1)).skip(1) {
        *slot = stretch(0, 0.0, b, heard[0]) + unexpected(b);
    }
    for g in 1..gaps {
        for b0 in 0..=n {
            let so_far = cost[g - 1][b0];
            if so_far.is_infinite() {
                continue;
            }
            for b in b0 + 1..n - (gaps - 1 - g) {
                let total = so_far + stretch(b0, heard[g - 1], b, heard[g]) + unexpected(b);
                if total < cost[g][b] {
                    cost[g][b] = total;
                    from[g][b] = b0;
                }
            }
        }
    }
    let mut last = 0;
    let mut cheapest = f64::INFINITY;
    for (b0, so_far) in cost[gaps - 1].iter().enumerate() {
        if so_far.is_infinite() {
            continue;
        }
        let total = so_far + stretch(b0, heard[gaps - 1], n, 1.0);
        if total < cheapest {
            cheapest = total;
            last = b0;
        }
    }
    let mut choice = vec![0; gaps];
    for g in (0..gaps).rev() {
        choice[g] = last;
        last = from[g][last];
    }
    choice
}

/// The start, in seconds, of every word of `spoken` with letters or digits.
/// Empty if there is none.
fn spoken_starts(spoken: &[Piece], levels: &[u8]) -> Vec<f64> {
    let n = spoken.len();
    if n == 0 {
        return Vec::new();
    }
    let window = LEVEL_WINDOW.as_secs_f64();
    let weights: Vec<f64> = spoken
        .iter()
        .map(|p| crate::text::weight(&p.key) as f64 + WORD)
        .collect();
    let sum: f64 = weights.iter().sum();
    let mut before = Vec::with_capacity(n + 1);
    before.push(0.0);
    let mut acc = 0.0;
    for w in &weights {
        acc += w;
        before.push(acc / sum);
    }
    let mut expected = vec![false; n + 1];
    for (b, piece) in spoken.iter().enumerate() {
        expected[b + 1] = piece.pause;
    }

    // Speech ends at the last sounding window: quiet after it is not speech.
    let speech = levels
        .iter()
        .rposition(|l| *l >= QUIET)
        .map_or(0, |last| last + 1);
    let levels = &levels[..speech];
    let mut gaps = pauses(levels);
    if gaps.len() > n - 1 {
        // More pauses than boundaries: the longest are the real ones.
        gaps.sort_by_key(|(start, end)| std::cmp::Reverse(end - start));
        gaps.truncate(n - 1);
        gaps.sort();
    }
    let silent: usize = gaps.iter().map(|(start, end)| end - start).sum();
    let sounding = (levels.len() - silent) as f64;
    let mut heard = Vec::with_capacity(gaps.len());
    let mut passed = 0;
    for (start, end) in &gaps {
        heard.push((start - passed) as f64 / sounding);
        passed += end - start;
    }
    let choice = anchor(&before, &expected, &heard);

    // Anchors: (word, window where the speech before it ends, window where
    // it starts).
    let mut anchors = vec![(0, 0, 0)];
    anchors.extend(
        choice
            .iter()
            .zip(&gaps)
            .map(|(b, (start, end))| (*b, *start, *end)),
    );
    anchors.push((n, levels.len(), levels.len()));
    let mut starts = vec![0.0; n];
    for pair in anchors.windows(2) {
        let ((b0, _, w0), (b1, w1, _)) = (pair[0], pair[1]);
        let span = before[b1] - before[b0];
        for (b, start) in starts.iter_mut().enumerate().take(b1).skip(b0) {
            let part = (before[b] - before[b0]) / span;
            *start = (w0 as f64 + part * (w1 as f64 - w0 as f64)) * window;
        }
    }
    starts
}

/// The line of every word. `ends[i]` is whether a clause ends after word `i`.
///
/// A clause is kept whole if it fits in [`LINE`] columns, and clauses share
/// a line while they fit together. A clause that is too long is cut into the
/// fewest lines that fit, as equal as its words allow, and not after a word
/// of [`SHORT`] letters or fewer when that can be avoided.
fn lines(words: &[String], ends: &[bool]) -> Vec<usize> {
    let width = |idx: &[usize]| {
        idx.iter()
            .enumerate()
            .map(|(k, &i)| {
                let gap = k > 0 && spaced(&words[idx[k - 1]], &words[i]);
                columns(&words[i]) + usize::from(gap)
            })
            .sum::<usize>()
    };
    let cut = |clause: &[usize]| -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        let mut rest = clause;
        loop {
            let total = width(rest);
            if total <= LINE || rest.len() == 1 {
                out.push(rest.to_vec());
                return out;
            }
            let parts = total.div_ceil(LINE);
            let target = total as f64 / parts as f64;
            let cost = |k: usize| {
                let head = &rest[..k];
                let short = key(&words[head[k - 1]]).chars().count() <= SHORT;
                let over = width(head) > LINE && k > 1;
                (width(head) as f64 - target).abs()
                    + if short { 8.0 } else { 0.0 }
                    + if over { 1000.0 } else { 0.0 }
            };
            let mut best = 1;
            for k in 2..rest.len() {
                if cost(k) < cost(best) {
                    best = k;
                }
            }
            out.push(rest[..best].to_vec());
            rest = &rest[best..];
        }
    };

    let mut clauses: Vec<Vec<usize>> = Vec::new();
    let mut current = Vec::new();
    for (i, end) in ends.iter().enumerate().take(words.len()) {
        current.push(i);
        if *end {
            clauses.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        clauses.push(current);
    }
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut line: Vec<usize> = Vec::new();
    for clause in clauses {
        let joined: Vec<usize> = line.iter().chain(&clause).copied().collect();
        if width(&joined) <= LINE {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            out.push(line);
        }
        let mut pieces = cut(&clause);
        line = pieces.pop().unwrap_or_default();
        out.extend(pieces);
    }
    if !line.is_empty() {
        out.push(line);
    }
    let mut number = vec![0; words.len()];
    for (n, idx) in out.iter().enumerate() {
        for &i in idx {
            number[i] = n;
        }
    }
    number
}

/// Whether two words are shown with a space between them: not where Han or
/// kana meet, which are written without spaces. `ui/captions.js` joins words
/// by the same rule.
fn spaced(a: &str, b: &str) -> bool {
    let tight = |c: Option<char>| c.is_some_and(|c| is_ideograph(c) || is_wide_punctuation(c));
    !(tight(a.chars().next_back()) || tight(b.chars().next()))
}

fn is_wide_punctuation(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFF60)
}

/// Width of `text` on screen in columns: a wide East Asian character takes
/// two.
fn columns(text: &str) -> usize {
    text.chars()
        .map(|c| {
            let wide = matches!(c as u32,
                0x1100..=0x115F
                | 0x2E80..=0x303E
                | 0x3041..=0x33FF
                | 0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xA000..=0xA4CF
                | 0xAC00..=0xD7A3
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE4F
                | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6
                | 0x1F300..=0x1F64F
                | 0x20000..=0x3FFFD);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Levels of speech in which every word of `sounds` is heard for that
    /// many windows, with `silence` quiet windows after each but the last.
    fn speech(sounds: &[usize], silence: &[usize]) -> Vec<u8> {
        let mut levels = Vec::new();
        for (i, &sound) in sounds.iter().enumerate() {
            levels.extend(std::iter::repeat_n(80, sound));
            if let Some(&quiet) = silence.get(i) {
                levels.extend(std::iter::repeat_n(2, quiet));
            }
        }
        levels
    }

    fn secs(windows: usize) -> f32 {
        windows as f32 * LEVEL_WINDOW.as_secs_f32()
    }

    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_word_after_a_pause_starts_where_the_pause_ends() {
        let text = "Он долго молчал, потом сказал.";
        // Sound for 40 windows, a pause of 10, sound for 30.
        let levels = speech(&[40, 30], &[10]);
        let words = words(text, text, &levels);
        assert_eq!(
            texts(&words),
            ["Он", "долго", "молчал,", "потом", "сказал."]
        );
        assert!(near(words[3].start_secs, secs(50)), "{words:?}");
        assert_eq!(words[0].start_secs, 0.0);
    }

    #[test]
    fn quiet_after_the_last_sound_is_not_speech() {
        let text = "Он долго молчал, потом сказал.";
        let levels = speech(&[40, 30], &[10]);
        let mut tail = levels.clone();
        tail.extend([3; 10]);
        assert_eq!(words(text, text, &levels), words(text, text, &tail));
    }

    #[test]
    fn time_between_two_pauses_is_shared_by_letters() {
        // Weights 1 + 1.5, 4 + 1.5 and 9 + 1.5, out of 18.5, over 74 windows.
        let text = "a bbbb ccccccccc";
        let levels = speech(&[74], &[]);
        let words = words(text, text, &levels);
        assert!(near(words[1].start_secs, secs(10)), "{words:?}");
        assert!(near(words[2].start_secs, secs(32)), "{words:?}");
    }

    #[test]
    fn a_pause_goes_to_the_boundary_with_punctuation() {
        // Five words of equal weight and a pause in the middle of the sound:
        // it fits the boundary after the second word and the one after the
        // third equally well.
        let levels = speech(&[30, 30], &[8]);
        let after_pause = |text: &str| {
            words(text, text, &levels)
                .iter()
                .position(|w| near(w.start_secs, secs(38)))
        };
        assert_eq!(after_pause("мир дом, сад лес кот"), Some(2));
        assert_eq!(after_pause("мир дом сад, лес кот"), Some(3));
    }

    #[test]
    fn a_number_is_shown_as_written_and_timed_as_read() {
        let shown = "и достигла 3,5 млрд рублей.";
        let spoken = crate::stress::mark(&crate::russian::spell_out(shown));
        let said = split(&spoken);
        // A pause after every spoken word, so each starts at a known window.
        let sounds: Vec<usize> = said.iter().map(|_| 10).collect();
        let levels = speech(&sounds, &vec![5; said.len() - 1]);
        let start_of = |key: &str| {
            let j = said.iter().position(|p| p.key == key).unwrap();
            secs(j * 15)
        };
        let words = words(shown, &spoken, &levels);
        assert_eq!(texts(&words), ["и", "достигла", "3,5 млрд", "рублей."]);
        assert!(near(words[2].start_secs, start_of("три")), "{words:?}");
        assert!(near(words[3].start_secs, start_of("рублей")), "{words:?}");
    }

    #[test]
    fn a_dash_is_drawn_with_the_word_before_it() {
        let text = "— Нет, — сказал он — и ушёл.";
        let words = words(text, text, &speech(&[50], &[]));
        assert_eq!(texts(&words), ["— Нет, —", "сказал", "он —", "и", "ушёл."]);
    }

    #[test]
    fn cjk_text_is_one_word_per_character() {
        let text = "東京に行きました。iPhone用";
        let words = words(text, text, &speech(&[60], &[]));
        assert_eq!(
            texts(&words),
            ["東", "京", "に", "行", "き", "ま", "し", "た。", "iPhone", "用"]
        );
        assert!(words.windows(2).all(|w| w[0].start_secs < w[1].start_secs));
        assert!(!spaced("た。", "iPhone"));
        assert!(spaced("안녕하세요", "세계"));
    }

    #[derive(serde::Deserialize)]
    struct Recording {
        shown: String,
        spoken: String,
    }

    #[test]
    fn a_line_holds_whole_clauses_and_fits() {
        let fixture: Vec<Recording> =
            serde_json::from_str(include_str!("../tests/fixtures/captions.json")).unwrap();
        let long = "Он долго молчал; потом сказал тихо: всё будет хорошо, только не торопись.";
        let sentences = fixture
            .iter()
            .map(|r| (r.shown.as_str(), r.spoken.as_str()))
            .chain([(long, long)]);
        for (shown, spoken) in sentences {
            let words = words(shown, spoken, &speech(&[100], &[]));
            let last = words.last().unwrap().line;
            for n in 0..=last {
                let on: Vec<&str> = words
                    .iter()
                    .filter(|w| w.line == n)
                    .map(|w| w.text.as_str())
                    .collect();
                assert!(!on.is_empty(), "{shown}: line {n} is empty");
                let line = on.join(" ");
                assert!(
                    columns(&line) <= LINE || on.len() == 1,
                    "{shown}: {line:?} is too long"
                );
                assert!(!line.ends_with(" т."), "{shown}: {line:?}");
            }
            assert!(words.windows(2).all(|w| w[0].line <= w[1].line));
        }
        let words = words(long, long, &[]);
        let line = |n| {
            words
                .iter()
                .filter(|w| w.line == n)
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(line(0), "Он долго молчал; потом сказал тихо:");
        assert_eq!(line(1), "всё будет хорошо, только не торопись.");
        let number = "Выручка компании выросла на 15% и достигла 3,5 млрд рублей.";
        let spoken = crate::stress::mark(&crate::russian::spell_out(number));
        let words = super::words(number, &spoken, &[]);
        let second: Vec<&str> = words
            .iter()
            .filter(|w| w.line == 1)
            .map(|w| w.text.as_str())
            .collect();
        assert_eq!(
            second,
            ["на", "15%", "и", "достигла", "3,5 млрд", "рублей."]
        );
    }

    /// A tiny generator, so the test needs no crate and repeats exactly.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }

        fn text(&mut self) -> String {
            const PARTS: &[&str] = &[
                " ", " ", " ", "а", "б", "ё", "\u{301}", "x", "Z", "7", ",", ".", "—", "-", "東",
                "に", "。", "한", "😀", "№", "\n", "e\u{301}", "ٱ", "स्", "\t", "!",
            ];
            (0..self.below(80))
                .map(|_| PARTS[self.below(PARTS.len())])
                .collect()
        }
    }

    #[test]
    fn starts_never_decrease_and_stay_inside_the_audio() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for round in 0..3000 {
            let shown = random.text();
            let spoken = if round % 3 == 0 {
                shown.clone()
            } else {
                random.text()
            };
            let levels: Vec<u8> = (0..random.below(300))
                .map(|_| match random.below(3) {
                    0 => 0,
                    1 => random.below(101) as u8,
                    _ => 255,
                })
                .collect();
            let words = words(&shown, &spoken, &levels);
            let total = secs(levels.len());
            for word in &words {
                assert!(!word.text.is_empty(), "{shown:?}");
                assert!(
                    (0.0..=total + 1e-4).contains(&word.start_secs),
                    "{shown:?} / {spoken:?}: {words:?}"
                );
            }
            assert!(
                words.windows(2).all(|w| w[0].start_secs <= w[1].start_secs),
                "{shown:?} / {spoken:?}: {words:?}"
            );
            assert!(words.windows(2).all(|w| w[0].line <= w[1].line));
        }
        assert!(words("", "", &[]).is_empty());
        assert!(words("— …", "", &[50, 50]).is_empty());
        let none = words("Привет мир", "", &[]);
        assert!(none.iter().all(|w| w.start_secs == 0.0));
    }
}
