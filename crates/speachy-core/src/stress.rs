//! Marks the stressed vowel of Russian words, and restores ё.
//!
//! Supertonic sometimes stresses the wrong syllable of a Russian word, but it
//! follows an acute accent (U+0301) placed after a vowel. Most Russian text
//! also prints е where ё is meant ("еще", "идет"), which changes both the
//! vowel and the stress. Both come from a dictionary table built by
//! `scripts/make-stress-dict.py`: words in byte order of Windows-1251,
//! front-coded in blocks of 64 and compressed with bzip2. Words whose reading
//! depends on the meaning ("замок", "берет", "все") are not in it, nor are
//! words already written with ё, so they are left as they are.

use std::io::Read;
use std::sync::OnceLock;

const ACCENT: char = '\u{301}';
const COMPRESSED: &[u8] = include_bytes!("../data/ru-stress.bz2");

/// Windows-1251 bytes of а е и о у ы э ю я.
const VOWELS: [u8; 9] = [0xE0, 0xE5, 0xE8, 0xEE, 0xF3, 0xFB, 0xFD, 0xFE, 0xFF];

/// What the table knows about a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    /// The stress falls on this vowel, counting from 1.
    Stressed(u8),
    /// This vowel is printed е and read ё, which is always stressed.
    Yo(u8),
}

impl Reading {
    fn from_byte(byte: u8) -> Reading {
        match byte & 0x80 {
            0 => Reading::Stressed(byte),
            _ => Reading::Yo(byte & 0x7F),
        }
    }
}

/// The decompressed dictionary and where its blocks start. Each entry is
/// `shared`, `rest` and reading bytes, then `rest` bytes of the word after the
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

    /// Calls `visit` with each word of the block starting at `from` and its
    /// reading, until the block ends or `visit` returns false.
    fn walk_block(
        &self,
        from: usize,
        word: &mut Vec<u8>,
        mut visit: impl FnMut(&[u8], Reading) -> bool,
    ) {
        word.clear();
        let mut at = from;
        let mut first = true;
        while let Some(head) = self.data.get(at..at + 3) {
            let (shared, rest, reading) = (head[0], head[1], Reading::from_byte(head[2]));
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
            if !visit(word, reading) {
                return;
            }
            at = end;
        }
    }

    /// How `word` (lowercase Windows-1251) is read, or `None` when the table
    /// does not have the word.
    fn reading(&self, word: &[u8]) -> Option<Reading> {
        let block = self
            .block_starts
            .partition_point(|&start| self.first_word(start) <= word)
            .checked_sub(1)?;
        let mut buffer = Vec::with_capacity(64);
        let mut found = None;
        self.walk_block(self.block_starts[block], &mut buffer, |entry, reading| {
            if entry == word {
                found = Some(reading);
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

/// Reads the table into memory now. The first [`mark`] does it otherwise,
/// and takes about half a second longer for it.
pub fn warm() {
    table();
}

/// Puts an accent after the stressed vowel of each Russian word whose stress
/// is known, so the voice stresses it there, and writes ё where a word
/// printed with е can only be read with ё.
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
    // An acronym is spelled out, not stressed.
    let all_capitals = letters > 1 && capitals == letters;
    let reading = if all_capitals {
        None
    } else {
        table.reading(bytes)
    };
    let Some(reading) = reading else {
        out.push_str(word);
        return;
    };
    let mut vowels = 0;
    for (c, byte) in word.chars().zip(bytes.iter()) {
        if !VOWELS.contains(byte) {
            out.push(c);
            continue;
        }
        vowels += 1;
        match reading {
            Reading::Stressed(at) if at == vowels => {
                out.push(c);
                out.push(ACCENT);
            }
            Reading::Yo(at) if at == vowels => out.push(if c.is_uppercase() { 'Ё' } else { 'ё' }),
            _ => out.push(c),
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

    fn lookup(word: &str) -> Option<Reading> {
        table().unwrap().reading(&cp1251(word))
    }

    #[test]
    fn looks_words_up() {
        use Reading::{Stressed, Yo};
        assert_eq!(lookup("вода"), Some(Stressed(2)));
        assert_eq!(lookup("город"), Some(Stressed(1)));
        assert_eq!(lookup("километров"), Some(Stressed(3)));
        assert_eq!(lookup("привет"), Some(Stressed(2)));
        assert_eq!(lookup("москва"), Some(Stressed(2)));
        assert_eq!(lookup("исследователи"), Some(Stressed(2)));
        assert_eq!(lookup("еще"), Some(Yo(2)));
        assert_eq!(lookup("самолет"), Some(Yo(3)));
        assert_eq!(lookup("мед"), Some(Yo(1)));
    }

    #[test]
    fn does_not_find_unknown_words() {
        // "берет" is "берёт" or "бере́т", "все" is "всё" or "все".
        for word in ["замок", "уже", "берет", "все", "ёлка", "дом", "hello", ""]
        {
            assert_eq!(lookup(word), None, "{word}");
        }
        assert_eq!(table().unwrap().reading(b"hello"), None);
        assert_eq!(table().unwrap().reading(b""), None);
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
    fn restores_yo() {
        assert_eq!(
            mark("Он еще идет, а самолет уже летит. Мед и ее чай."),
            "Он ещё идёт, а самолёт уже лети\u{301}т. Мёд и её чай."
        );
        assert_eq!(mark("Еще не все берет."), "Ещё не все берет.");
        let restored = mark("Зеленый самолет.");
        assert_eq!(restored, "Зелёный самолёт.");
        assert_eq!(mark(&restored), restored);
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
        assert_eq!(table.data.len(), 14_969_676);
        let mut buffer = Vec::new();
        let mut index = 0;
        let mut found = 0;
        for &start in &table.block_starts {
            let mut previous: Option<(Vec<u8>, Reading)> = None;
            let mut first: Option<(Vec<u8>, Reading)> = None;
            table.walk_block(start, &mut buffer, |word, reading| {
                if first.is_none() {
                    first = Some((word.to_vec(), reading));
                }
                previous = Some((word.to_vec(), reading));
                if index % 997 == 0 {
                    assert_eq!(table.reading(word), Some(reading));
                    found += 1;
                }
                index += 1;
                true
            });
            for (word, reading) in [first.unwrap(), previous.unwrap()] {
                assert_eq!(table.reading(&word), Some(reading));
            }
        }
        assert_eq!(index, 3_153_889);
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
