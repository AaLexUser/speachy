//! Holds `fixtures/stress-spoken.txt` to the code: it is what the voice is
//! given for the sentences of `fixtures/stress-gold.txt`, read as one text.
//!
//! The stress eval (`scripts/stress-eval.py`) scores that file against the
//! gold. This test knows nothing about scoring. It fails when the engine would
//! give the voice something else than the file says, so a change to the
//! stress table, to the rewriting of numbers or to chunking cannot land
//! without the file changing with it, and the diff of the file shows every
//! word that is now read differently.
//!
//! To accept a change:
//!
//! ```sh
//! SPEACHY_BLESS=1 cargo test -p speachy-core --test stress
//! python3 scripts/stress-eval.py marks --since main
//! ```

use speachy_core::catalog;
use speachy_core::plan::{self, PlannedChunk};
use speachy_core::settings::Settings;
use speachy_core::text::Boundary;

const GOLD: &str = include_str!("fixtures/stress-gold.txt");
const SPOKEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/stress-spoken.txt"
);

const BLESS: &str = "SPEACHY_BLESS=1 cargo test -p speachy-core --test stress";
const SCORE: &str = "python3 scripts/stress-eval.py marks --since main";

const HEADER: &str = "\
# What the voice is given for the sentences of stress-gold.txt, read as one document: the gold with its
# comments and stress marks removed and ё printed as е (as most articles print it), every sentence a
# paragraph of its own. Planned with default settings, language detection on, Supertonic 3 installed.
#
# One chunk per line, as it goes to the voice: numbers, units and abbreviations written out, and a
# U+0301 after the stressed vowel of each word the stress table knows (ё restored). An empty line
# follows the last chunk of each sentence. A chunk planned in a language other than Russian starts
# with its language code in square brackets, as in: [bg] Text of the chunk.
#
# This file is the code's output and is never edited by hand. After a change to stress.rs, russian.rs,
# the stress table or chunking, remake it and look at what moved:
#   SPEACHY_BLESS=1 cargo test -p speachy-core --test stress
#   python3 scripts/stress-eval.py marks --since main
";

/// The gold's sentences: every line that is not a comment or empty.
fn gold_sentences() -> Vec<&'static str> {
    GOLD.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .collect()
}

/// The gold sentences as a reader would select them: comments and stress
/// marks (U+0301) removed, ё printed as е, every sentence a paragraph of its
/// own.
fn document() -> String {
    let sentences: Vec<String> = gold_sentences()
        .into_iter()
        .map(|line| {
            line.chars()
                .filter(|&c| c != '\u{301}')
                .map(|c| match c {
                    'ё' => 'е',
                    'Ё' => 'Е',
                    c => c,
                })
                .collect()
        })
        .collect();
    sentences.join("\n\n")
}

/// The plan of [`document`] as the app makes it with default settings and
/// Supertonic 3 as the only installed model. No model file is read.
fn planned() -> Vec<PlannedChunk> {
    let model = catalog::find("supertonic-3").expect("supertonic-3 is in the catalog");
    plan::plan(&document(), &Settings::default(), &[model]).expect("the gold document has a plan")
}

/// A chunk as one line of the file: what the voice is given, behind its
/// language in brackets when that is not Russian.
fn chunk_line(chunk: &PlannedChunk) -> String {
    let given = chunk.spoken.as_deref().unwrap_or(&chunk.text);
    assert!(
        !given.contains(['\n', '\r']),
        "a chunk spans lines, which the file cannot show: {given:?}"
    );
    match chunk.lang.as_str() {
        "ru" => given.to_string(),
        lang => format!("[{lang}] {given}"),
    }
}

/// The file's format: the header, then what the voice is given, one chunk per
/// line, and an empty line after the last chunk of each gold sentence (the
/// chunk whose boundary is `Paragraph` or `End`).
fn spoken_file(chunks: &[PlannedChunk]) -> String {
    let mut file = String::from(HEADER);
    file.push('\n');
    for chunk in chunks {
        file.push_str(&chunk_line(chunk));
        file.push('\n');
        if matches!(chunk.boundary, Boundary::Paragraph | Boundary::End) {
            file.push('\n');
        }
    }
    file
}

/// The chunk lines of a file: everything but comments and empty lines.
fn chunk_lines(file: &str) -> Vec<&str> {
    file.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

#[test]
fn the_gold_sentences_are_given_to_the_voice_as_the_fixture_says() {
    let chunks = planned();
    let blocks = chunks
        .iter()
        .filter(|c| matches!(c.boundary, Boundary::Paragraph | Boundary::End))
        .count();
    assert_eq!(
        blocks,
        gold_sentences().len(),
        "the document must plan as one block per gold sentence: a sentence was merged with its \
         neighbour or lost"
    );

    let now = spoken_file(&chunks);
    if std::env::var("SPEACHY_BLESS").as_deref() == Ok("1") {
        std::fs::write(SPOKEN, &now).expect("write stress-spoken.txt");
        return;
    }
    let file = std::fs::read_to_string(SPOKEN).unwrap_or_default();
    if file == now {
        return;
    }

    let (old, new) = (chunk_lines(&file), chunk_lines(&now));
    let differing = (0..old.len().max(new.len()))
        .filter(|&i| old.get(i) != new.get(i))
        .count();
    let first = (0..old.len().max(new.len())).find(|&i| old.get(i) != new.get(i));
    match first {
        Some(i) => panic!(
            "stress-spoken.txt is not what the engine gives the voice. {differing} of {} chunks \
             differ; the first is chunk {}:\n  the file  {}\n  the code  {}\n\
             To accept the change, remake the file and look at what moved:\n  {BLESS}\n  {SCORE}",
            old.len().max(new.len()),
            i + 1,
            old.get(i).copied().unwrap_or("(no such chunk)"),
            new.get(i).copied().unwrap_or("(no such chunk)"),
        ),
        None => panic!(
            "stress-spoken.txt has the chunks the engine gives the voice, but its header or its \
             paragraph breaks differ. Remake it:\n  {BLESS}"
        ),
    }
}
