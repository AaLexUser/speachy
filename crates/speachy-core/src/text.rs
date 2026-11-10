//! Turns arbitrary selected text into speakable chunks.
//!
//! The pipeline synthesizes one chunk at a time, so chunk sizes decide how
//! soon speech starts and whether playback can stay ahead of synthesis:
//!
//! * the first chunk is kept short, so audio starts quickly;
//! * every later chunk may be as long as everything before it put together
//!   (up to a cap), so as long as synthesis runs at 2x real time or better a
//!   chunk is ready by the time the ones before it have played;
//! * chunks end on sentence boundaries whenever they can, on clause
//!   boundaries when a sentence is too long, and never inside a word.

use serde::Serialize;

/// What follows a chunk. Decides the pause inserted after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Boundary {
    /// The chunk was cut inside a sentence.
    Clause,
    Sentence,
    Paragraph,
    /// Last chunk of the text.
    End,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chunk {
    pub text: String,
    pub boundary: Boundary,
}

#[derive(Debug, Clone)]
pub struct ChunkOptions {
    /// Size limit of the first chunk, in [`weight`] units (roughly Latin
    /// characters).
    pub first_max: usize,
    /// Size limit of any chunk.
    pub max: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self { first_max: 110, max: 280 }
    }
}

/// Spoken length of a string in rough "Latin character" units. CJK
/// characters carry a syllable each, so they count for more.
pub fn weight(s: &str) -> usize {
    s.chars().map(char_weight).sum()
}

fn char_weight(c: char) -> usize {
    if is_cjk(c) {
        3
    } else {
        1
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility
    )
}

/// Splits `text` into chunks ready for synthesis.
pub fn chunk(text: &str, opts: &ChunkOptions) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let paragraphs = paragraphs(text);
    for paragraph in &paragraphs {
        let mut current = String::new();
        for sentence in sentences(paragraph) {
            let limit = next_limit(&chunks, opts);
            if !current.is_empty() && weight(&current) + 1 + weight(sentence) <= limit {
                current.push(' ');
                current.push_str(sentence);
                continue;
            }
            if !current.is_empty() {
                push(&mut chunks, std::mem::take(&mut current), Boundary::Sentence);
            }
            // The sentence starts a chunk. Cut it into clauses if it is too
            // long; the last piece stays open so short followers can join it.
            let mut rest = sentence;
            loop {
                let limit = next_limit(&chunks, opts);
                if weight(rest) <= limit {
                    current.push_str(rest);
                    break;
                }
                let (head, tail) = split_long(rest, limit);
                push(&mut chunks, head.to_string(), Boundary::Clause);
                rest = tail;
            }
        }
        if !current.is_empty() {
            push(&mut chunks, current, Boundary::Paragraph);
        }
    }
    if let Some(last) = chunks.last_mut() {
        last.boundary = Boundary::End;
    }
    chunks
}

fn push(chunks: &mut Vec<Chunk>, text: String, boundary: Boundary) {
    let text = text.trim().to_string();
    // Nothing to pronounce (e.g. a line of dashes): skip rather than send
    // the model an input it produces noise or nothing for.
    if text.chars().any(char::is_alphanumeric) {
        chunks.push(Chunk { text, boundary });
    }
}

fn next_limit(chunks: &[Chunk], opts: &ChunkOptions) -> usize {
    let spoken: usize = chunks.iter().map(|c| weight(&c.text)).sum();
    spoken.clamp(opts.first_max, opts.max.max(opts.first_max))
}

/// Splits `s`, which is heavier than `limit`, into a head of at most `limit`
/// and the rest. Prefers the last clause separator, then the last space,
/// then a hard cut.
fn split_long(s: &str, limit: usize) -> (&str, &str) {
    let mut used = 0;
    let mut clause = None;
    let mut space = None;
    let mut hard = s.len();
    for (i, c) in s.char_indices() {
        used += char_weight(c);
        if used > limit {
            hard = i;
            break;
        }
        let after = i + c.len_utf8();
        if matches!(c, ',' | ';' | ':' | '—' | '–' | '，' | '、' | '；' | '：' | ')') {
            clause = Some(after);
        } else if c.is_whitespace() {
            space = Some(i);
        }
    }
    // A clause cut very near the start would make a uselessly small chunk.
    let min = s.len().min(hard) / 4;
    let cut = clause
        .filter(|&i| i >= min)
        .or(space.filter(|&i| i > 0))
        .unwrap_or(if hard == 0 { s.chars().next().map_or(0, char::len_utf8) } else { hard });
    (s[..cut].trim_end(), s[cut..].trim_start())
}

/// Cleans the raw text and splits it into paragraphs.
///
/// Hard-wrapped lines (PDFs, emails, terminals) are joined back together;
/// headings and list items become their own paragraphs so they get a pause.
fn paragraphs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut break_before_next = false;
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let (line, standalone) = clean_line(raw);
        if line.is_empty() {
            flush(&mut out, &mut current);
            continue;
        }
        let ended = current.trim_end().ends_with(is_terminator);
        if !current.is_empty() && (standalone || break_before_next || ended) {
            flush(&mut out, &mut current);
        }
        if !current.is_empty() {
            if let Some(stripped) = current.strip_suffix('-').filter(|s| ends_with_letter(s)) {
                // "exam-\nple": a word hyphenated across a line break.
                current = stripped.to_string();
            } else {
                current.push(' ');
            }
        }
        current.push_str(&line);
        break_before_next = standalone;
    }
    flush(&mut out, &mut current);
    out
}

fn ends_with_letter(s: &str) -> bool {
    s.chars().next_back().is_some_and(char::is_alphabetic)
}

fn flush(out: &mut Vec<String>, current: &mut String) {
    let text = current.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
    current.clear();
}

/// Cleans one line. Returns the text and whether the line stands alone
/// (a heading or a list item) rather than flowing into its neighbours.
fn clean_line(raw: &str) -> (String, bool) {
    let mut line = raw.trim();
    let mut standalone = false;

    // Markdown heading / quote markers.
    let stripped = line.trim_start_matches(['#', '>']);
    if stripped.len() != line.len() {
        standalone = line.starts_with('#');
        line = stripped.trim_start();
    }
    // List bullets: "- ", "* ", "• ", "1. ", "2) ".
    if let Some(rest) = ["- ", "* ", "• ", "– ", "+ "].iter().find_map(|b| line.strip_prefix(b)) {
        line = rest;
        standalone = true;
    } else {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        if (1..=3).contains(&digits) {
            if let Some(rest) = line[digits..].strip_prefix(". ").or(line[digits..].strip_prefix(") ")) {
                // Keep the number: "1. Install" reads better as "1, Install".
                let text = format!("{}, {}", &line[..digits], rest.trim_start());
                return (clean_inline(&text), true);
            }
        }
    }
    // A rule such as "-----" or "=====" carries nothing to read.
    if !line.is_empty() && line.chars().all(|c| matches!(c, '-' | '=' | '_' | '*' | '~')) {
        return (String::new(), true);
    }
    (clean_inline(line), standalone)
}

/// Removes markup that would be read out as noise and collapses whitespace.
fn clean_inline(line: &str) -> String {
    let line = strip_links(line);
    let mut out = String::with_capacity(line.len());
    for word in line.split_whitespace() {
        let word = clean_word(word);
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word);
    }
    out
}

/// Replaces Markdown links, `[label](target)`, with their label.
fn strip_links(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(mid) = rest.find("](") {
        let (Some(open), Some(close)) = (rest[..mid].rfind('['), rest[mid..].find(')')) else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..mid]);
        rest = &rest[mid + close + 1..];
    }
    out.push_str(rest);
    out
}

fn clean_word(word: &str) -> String {
    // A bare URL keeps only its host.
    if let Some(pos) = word.find("://") {
        let scheme_start = word[..pos].rfind(|c: char| !c.is_ascii_alphabetic()).map_or(0, |i| i + 1);
        let rest = &word[pos + 3..];
        let host_end = rest.find(['/', '?', '#', ')', ']', '>']).unwrap_or(rest.len());
        let host = rest[..host_end].trim_start_matches("www.");
        let tail_punct: String = word.chars().rev().take_while(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?')).collect();
        let host = host.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        return format!("{}{}{}", &word[..scheme_start], host, tail_punct.chars().rev().collect::<String>());
    }
    word.chars()
        .filter(|c| {
            !matches!(c,
                '*' | '`' | '~' | '|' | '\\'
                // Zero-width and directional marks, soft hyphen, BOM.
                | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{00AD}' | '\u{FEFF}')
        })
        .map(|c| match c {
            '_' => ' ',
            '\u{00A0}' | '\u{202F}' | '\u{2007}' => ' ',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？' | ':')
}

/// Abbreviations whose trailing period does not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "eg", "ie", "no",
    "fig", "inc", "ltd", "co", "corp", "approx", "dept", "est", "vol", "al", "cf", "ca", "resp",
    "gen", "col", "lt", "sgt", "capt", "rev", "hon", "mt", "ft", "jan", "feb", "mar", "apr", "jun",
    "jul", "aug", "sep", "sept", "oct", "nov", "dec", "т.е", "т.д", "т.п", "т.к", "др", "им",
    "ул", "стр", "рис", "см", "напр", "bzw", "z.b", "ca", "nr", "sog", "usw",
];

/// Splits a paragraph into sentences. Slices of `p`, trimmed.
fn sentences(p: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = p.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let wide = matches!(c, '。' | '！' | '？');
        if !wide && !matches!(c, '.' | '!' | '?' | '…') {
            continue;
        }
        // Swallow runs of terminators and any closing quotes or brackets.
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = chars.peek() {
            if matches!(n, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '"' | '\'' | '”' | '’' | '»' | ')' | ']' | '」' | '』') {
                end = j + n.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let next = chars.peek().map(|&(_, n)| n);
        let at_break = match next {
            None => true,
            Some(n) if n.is_whitespace() => true,
            Some(_) => wide,
        };
        if !at_break {
            continue;
        }
        if c == '.' && end == i + 1 && !period_ends_sentence(p, i, &p[end..]) {
            continue;
        }
        let sentence = p[start..end].trim();
        if !sentence.is_empty() {
            out.push(sentence);
        }
        start = end;
    }
    let tail = p[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

/// Decides whether the lone period at byte `i` of `p` ends a sentence.
fn period_ends_sentence(p: &str, i: usize, after: &str) -> bool {
    let before = &p[..i];
    let word_start = before.rfind(|c: char| c.is_whitespace() || matches!(c, '(' | '[' | '"' | '“')).map_or(0, |j| j + 1);
    let word = &before[word_start..];
    let lower = word.to_lowercase();
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return false;
    }
    // A single letter is an initial or an abbreviation: "J. K. Rowling",
    // "г. Москва".
    let mut letters = word.chars();
    if let (Some(first), None) = (letters.next(), letters.next()) {
        if first.is_alphabetic() {
            return false;
        }
    }
    // "… approx. three" — a lowercase continuation is the same sentence.
    match after.trim_start().chars().next() {
        Some(n) if n.is_lowercase() => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(chunks: &[Chunk]) -> Vec<&str> {
        chunks.iter().map(|c| c.text.as_str()).collect()
    }

    fn opts(first_max: usize, max: usize) -> ChunkOptions {
        ChunkOptions { first_max, max }
    }

    #[test]
    fn splits_sentences_and_marks_boundaries() {
        let chunks = chunk("First one. Second one! Third one?", &opts(12, 12));
        assert_eq!(texts(&chunks), ["First one.", "Second one!", "Third one?"]);
        assert_eq!(chunks[0].boundary, Boundary::Sentence);
        assert_eq!(chunks[2].boundary, Boundary::End);
    }

    #[test]
    fn first_chunk_is_short_then_sizes_ramp_up() {
        let sentence = "This sentence has exactly forty characters.";
        let text = vec![sentence; 12].join(" ");
        let chunks = chunk(&text, &opts(50, 200));
        let sizes: Vec<usize> = chunks.iter().map(|c| weight(&c.text)).collect();
        assert_eq!(sizes[0], 43, "first chunk is a single sentence");
        assert!(sizes.iter().all(|&s| s <= 200), "{sizes:?}");
        assert!(*sizes.last().unwrap() > 100 || sizes.iter().any(|&s| s > 150), "sizes grow: {sizes:?}");
        // No chunk is longer than everything spoken before it.
        for k in 1..sizes.len() {
            let before: usize = sizes[..k].iter().sum();
            assert!(sizes[k] <= before.max(50), "ramp violated at {k}: {sizes:?}");
        }
        // Nothing is lost or reordered.
        assert_eq!(texts(&chunks).join(" "), text);
    }

    #[test]
    fn long_sentence_is_cut_at_clauses_not_inside_words() {
        let text = "When the weather is fine, and the wind is low, we walk along the river, \
                    past the old mill, until the path ends at the bridge by the station.";
        let chunks = chunk(text, &opts(40, 60));
        assert!(chunks.len() >= 3);
        for c in &chunks[..chunks.len() - 1] {
            assert_eq!(c.boundary, Boundary::Clause);
            assert!(c.text.ends_with(','), "cut at a comma: {:?}", c.text);
        }
        assert_eq!(texts(&chunks).join(" "), text);
    }

    #[test]
    fn unbroken_text_is_hard_cut_without_losing_characters() {
        let text = "a".repeat(250);
        let chunks = chunk(&text, &opts(100, 100));
        assert_eq!(chunks.len(), 3);
        assert_eq!(texts(&chunks).concat(), text);

        let cjk = "今天天气很好我们一起去公园散步吧".repeat(4);
        let chunks = chunk(&cjk, &opts(30, 30));
        assert!(chunks.iter().all(|c| weight(&c.text) <= 30));
        assert_eq!(texts(&chunks).concat(), cjk);
    }

    #[test]
    fn abbreviations_initials_and_decimals_do_not_split() {
        let cases = [
            "Dr. Smith met Mr. Jones at 3.15 p.m. on the corner.",
            "The value is approx. three times larger, e.g. in winter.",
            "J. K. Rowling wrote it.",
            "Version 2.0.1 is out now.",
            "Он работает в г. Москве и т.д. уже давно.",
        ];
        for text in cases {
            assert_eq!(texts(&chunk(text, &opts(200, 200))), [text]);
            assert_eq!(sentences(text), [text], "{text}");
        }
        assert_eq!(sentences("It works. Really! Are you sure? Yes."), ["It works.", "Really!", "Are you sure?", "Yes."]);
        assert_eq!(sentences("He said \"stop.\" Then he left..."), ["He said \"stop.\"", "Then he left..."]);
        assert_eq!(sentences("今天天气很好。我们去公园吧！好吗？"), ["今天天气很好。", "我们去公园吧！", "好吗？"]);
    }

    #[test]
    fn paragraphs_are_kept_apart_and_wrapped_lines_joined() {
        let text = "A line that was wrapped by\nthe terminal at a fixed width.\n\nNext paragraph.";
        let chunks = chunk(text, &opts(200, 200));
        assert_eq!(texts(&chunks), ["A line that was wrapped by the terminal at a fixed width.", "Next paragraph."]);
        assert_eq!(chunks[0].boundary, Boundary::Paragraph);

        assert_eq!(paragraphs("a hyphen-\nated word"), ["a hyphenated word"]);
        assert_eq!(paragraphs("3 - 2 -\n1"), ["3 - 2 - 1"]);
    }

    #[test]
    fn markdown_and_urls_are_cleaned() {
        let text = "# Title\n\n- **Bold** item with `code`\n- See [the docs](https://example.com/a/b?c=d).\n\
                    2. Visit https://www.rust-lang.org/learn, then stop.\n-----\nsnake_case_name";
        assert_eq!(
            paragraphs(text),
            [
                "Title",
                "Bold item with code",
                "See the docs.",
                "2, Visit rust-lang.org, then stop.",
                "snake case name",
            ]
        );
    }

    #[test]
    fn empty_and_symbol_only_input_gives_no_chunks() {
        assert!(chunk("", &ChunkOptions::default()).is_empty());
        assert!(chunk("  \n\n  ", &ChunkOptions::default()).is_empty());
        assert!(chunk("--- *** ...", &ChunkOptions::default()).is_empty());
    }

    #[test]
    fn short_followers_join_the_last_piece_of_a_long_sentence() {
        let text = "One two three four five six seven eight nine ten eleven twelve. Ok.";
        let chunks = chunk(text, &opts(30, 60));
        assert!(chunks.last().unwrap().text.ends_with("twelve. Ok."), "{chunks:?}");
    }
}
