//! Marks the stressed vowel of Russian words.
//!
//! Supertonic sometimes stresses the wrong syllable of a Russian word, but it
//! follows an acute accent (U+0301) placed after a vowel. The stress comes
//! from a dictionary table built by `scripts/make-stress-dict.py`: words in
//! byte order of Windows-1251, front-coded in blocks of 64 and compressed
//! with bzip2. Words with one vowel, with ё, and words whose stress depends on
//! the meaning are not in it, so they are left unmarked.

use std::io::Read;
use std::sync::OnceLock;

const ACCENT: char = '\u{301}';
const COMPRESSED: &[u8] = include_bytes!("../data/ru-stress.bz2");

/// Windows-1251 bytes of а е и о у ы э ю я.
const VOWELS: [u8; 9] = [0xE0, 0xE5, 0xE8, 0xEE, 0xF3, 0xFB, 0xFD, 0xFE, 0xFF];

/// The decompressed dictionary and where its blocks start. Each entry is
/// `shared`, `rest` and `vowel` bytes, then `rest` bytes of the word after the
/// `shared` bytes it has in common with the previous word.
struct Table {
    data: Vec<u8>,
    block_starts: Vec<usize>,
}

impl Table {
    fn load() -> Option<Table> {
        let mut data = Vec::new();
        bzip2::read::BzDecoder::new(COMPRESSED)
            .read_to_end(&mut data)
            .ok()?;
        let mut block_starts = Vec::new();
        let mut at = 0;
        while at < data.len() {
            let head = data.get(at..at + 3)?;
            let (shared, rest) = (head[0], head[1]);
            data.get(at + 3..at + 3 + usize::from(rest))?;
            if shared == 0 {
                block_starts.push(at);
            }
            at += 3 + usize::from(rest);
        }
        Some(Table { data, block_starts })
    }

    /// Calls `visit` with each word of the block starting at `from` and the
    /// number of its stressed vowel, until the block ends or `visit` returns
    /// false.
    fn walk_block(
        &self,
        from: usize,
        word: &mut Vec<u8>,
        mut visit: impl FnMut(&[u8], u8) -> bool,
    ) {
        word.clear();
        let mut at = from;
        let mut first = true;
        while let Some(head) = self.data.get(at..at + 3) {
            let (shared, rest, vowel) = (head[0], head[1], head[2]);
            if shared == 0 && !first {
                return;
            }
            first = false;
            let end = at + 3 + usize::from(rest);
            let Some(tail) = self.data.get(at + 3..end) else {
                return;
            };
            word.truncate(usize::from(shared));
            word.extend_from_slice(tail);
            if !visit(word, vowel) {
                return;
            }
            at = end;
        }
    }

    /// Which vowel of `word` (lowercase Windows-1251) is stressed, counting
    /// from 1, or `None` when the table does not have the word.
    fn stressed_vowel(&self, word: &[u8]) -> Option<u8> {
        let block = self
            .block_starts
            .partition_point(|&start| self.first_word(start) <= word)
            .checked_sub(1)?;
        let mut buffer = Vec::with_capacity(64);
        let mut found = None;
        self.walk_block(self.block_starts[block], &mut buffer, |entry, vowel| {
            if entry == word {
                found = Some(vowel);
            }
            entry < word
        });
        found
    }

    fn first_word(&self, block_start: usize) -> &[u8] {
        let rest = usize::from(self.data[block_start + 1]);
        &self.data[block_start + 3..block_start + 3 + rest]
    }
}

fn table() -> Option<&'static Table> {
    static TABLE: OnceLock<Option<Table>> = OnceLock::new();
    TABLE.get_or_init(Table::load).as_ref()
}

fn is_cyrillic(c: char) -> bool {
    ('\u{400}'..='\u{4FF}').contains(&c) && c.is_alphabetic()
}

/// The lowercase Windows-1251 byte of а to я in either case.
fn lowercase_byte(c: char) -> Option<u8> {
    match c {
        '\u{430}'..='\u{44F}' => Some(0xE0 + (c as u32 - 0x430) as u8),
        '\u{410}'..='\u{42F}' => Some(0xE0 + (c as u32 - 0x410) as u8),
        _ => None,
    }
}

/// Puts an accent after the stressed vowel of each Russian word whose stress
/// is known, so the voice stresses it there.
pub fn mark(text: &str) -> String {
    let Some(table) = table() else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut bytes = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_cyrillic(c) {
            out.push(c);
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !is_cyrillic(next) && next != ACCENT {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        mark_word(table, &text[start..end], &mut bytes, &mut out);
    }
    out
}

fn mark_word(table: &Table, word: &str, bytes: &mut Vec<u8>, out: &mut String) {
    bytes.clear();
    let mut letters = 0;
    let mut capitals = 0;
    for c in word.chars() {
        let Some(byte) = lowercase_byte(c) else {
            out.push_str(word);
            return;
        };
        letters += 1;
        capitals += usize::from(c.is_uppercase());
        bytes.push(byte);
    }
    let all_capitals = letters > 1 && capitals == letters;
    let stressed = if all_capitals || bytes.iter().filter(|b| VOWELS.contains(b)).count() < 2 {
        None
    } else {
        table.stressed_vowel(bytes)
    };
    let Some(stressed) = stressed else {
        out.push_str(word);
        return;
    };
    let mut vowels = 0;
    for (c, byte) in word.chars().zip(bytes.iter()) {
        out.push(c);
        if VOWELS.contains(byte) {
            vowels += 1;
            if vowels == stressed {
                out.push(ACCENT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp1251(word: &str) -> Vec<u8> {
        word.chars()
            .map(|c| lowercase_byte(c).unwrap_or(b'?'))
            .collect()
    }

    fn lookup(word: &str) -> Option<u8> {
        table().unwrap().stressed_vowel(&cp1251(word))
    }

    #[test]
    fn looks_words_up() {
        assert_eq!(lookup("вода"), Some(2));
        assert_eq!(lookup("город"), Some(1));
        assert_eq!(lookup("километров"), Some(3));
        assert_eq!(lookup("привет"), Some(2));
        assert_eq!(lookup("москва"), Some(2));
        assert_eq!(lookup("исследователи"), Some(2));
    }

    #[test]
    fn does_not_find_unknown_words() {
        for word in ["замок", "уже", "ёлка", "дом", "hello", ""] {
            assert_eq!(lookup(word), None, "{word}");
        }
        assert_eq!(table().unwrap().stressed_vowel(b"hello"), None);
        assert_eq!(table().unwrap().stressed_vowel(b""), None);
    }

    #[test]
    fn marks_stress() {
        assert_eq!(
            mark("Привет, вода и город."),
            "Приве\u{301}т, вода\u{301} и го\u{301}род."
        );
        assert_eq!(
            mark("Пятнадцать километров в час."),
            "Пятна\u{301}дцать киломе\u{301}тров в час."
        );
    }

    #[test]
    fn leaves_acronyms_yo_and_unknown_words() {
        assert_eq!(
            mark("Москва и США: замок, ёлка, дом."),
            "Москва\u{301} и США: замок, ёлка, дом."
        );
    }

    #[test]
    fn keeps_capitalization() {
        assert_eq!(mark("ВОДА Вода вОда"), "ВОДА Вода\u{301} вОда\u{301}");
    }

    #[test]
    fn marks_each_part_of_a_hyphenated_word() {
        assert_eq!(mark("кое-кто"), mark("кое") + "-" + &mark("кто"));
    }

    #[test]
    fn handles_neighbouring_scripts() {
        assert_eq!(mark("вода2 water вода"), "вода\u{301}2 water вода\u{301}");
        assert_eq!(mark("no Cyrillic 123"), "no Cyrillic 123");
    }

    #[test]
    fn leaves_marked_words_alone() {
        let marked = "Приве\u{301}т, го\u{301}род, вода\u{301}.";
        assert_eq!(mark(marked), marked);
        assert_eq!(mark("вода\u{301}"), "вода\u{301}");
    }

    #[test]
    fn is_idempotent() {
        let text = "Исследователи из Москвы измерили километров пятнадцать. Вода и город, США, замок, ёлка; кое-кто 5 раз.";
        let once = mark(text);
        assert_ne!(once, text);
        assert_eq!(mark(&once), once);
    }

    #[test]
    fn table_is_intact() {
        let table = table().unwrap();
        assert_eq!(table.data.len(), 14_659_327);
        let mut buffer = Vec::new();
        let mut index = 0;
        let mut found = 0;
        for &start in &table.block_starts {
            let mut previous: Option<(Vec<u8>, u8)> = None;
            let mut first: Option<(Vec<u8>, u8)> = None;
            table.walk_block(start, &mut buffer, |word, vowel| {
                if first.is_none() {
                    first = Some((word.to_vec(), vowel));
                }
                previous = Some((word.to_vec(), vowel));
                if index % 997 == 0 {
                    assert_eq!(table.stressed_vowel(word), Some(vowel));
                    found += 1;
                }
                index += 1;
                true
            });
            for (word, vowel) in [first.unwrap(), previous.unwrap()] {
                assert_eq!(table.stressed_vowel(&word), Some(vowel));
            }
        }
        assert_eq!(index, 3_086_436);
        assert!(found > 3000);
    }

    #[test]
    fn survives_hostile_input() {
        mark("");
        mark("\u{301}\u{301}\u{301}");
        mark("😀 e\u{301} \u{200d}👨\u{200d}👩 а\u{301}\u{301}");
        mark("ѣѵ ґє ВОДА\u{301}");
        let unit = "вода город hello 123 Привет ёлка США \u{301}";
        mark(&unit.repeat(100_000 / unit.chars().count()));
        mark(&"а".repeat(100_000));
        mark(&"вода".repeat(25_000));
    }
}
