//! English grapheme-to-phoneme conversion for Kokoro.
//!
//! Kokoro was trained on the phonemes of the misaki G2P. sherpa-onnx can
//! instead look words up in a plain lexicon, but that silently drops every
//! word the lexicon lacks — inflected forms, numbers, names. So English is
//! phonemized here (misaki-rs: dictionary, part-of-speech tagging, number
//! and abbreviation expansion) and handed to the model as phonemes.
//!
//! Words even misaki does not know (product names, jargon) go through
//! [`RuleFallback`], a small set of spelling rules: approximate, but a
//! mispronounced word is better than a missing one.

use misaki_rs::fallback::FallbackError;
use misaki_rs::{Fallback, Language, G2P};
use std::sync::Mutex;

/// Phoneme sequences longer than this are split, staying well inside the
/// model's 510-token context.
const MAX_PHONEMES: usize = 380;

pub struct Phonemizer {
    // The tagger inside is not `Sync`; calls are short, so a lock is fine.
    g2p: Mutex<G2P>,
}

impl Phonemizer {
    pub fn new(british: bool) -> Self {
        let lang = if british {
            Language::EnglishGB
        } else {
            Language::EnglishUS
        };
        Self {
            g2p: Mutex::new(G2P::with_fallback(
                lang,
                Some(Box::new(RuleFallback { british })),
            )),
        }
    }

    /// Phonemizes `text` into sentences of Unicode codepoints, the form
    /// sherpa-onnx takes pre-phonemized input in.
    pub fn phonemize(&self, text: &str) -> Vec<Vec<i32>> {
        let phonemes = match self.g2p.lock().unwrap().g2p(text) {
            Ok((phonemes, _)) => phonemes,
            Err(err) => {
                log::warn!("could not phonemize {text:?}: {err}");
                return Vec::new();
            }
        };
        split_phonemes(&phonemes)
            .into_iter()
            .map(|sentence| {
                sentence
                    .chars()
                    .filter(|c| *c != '❓')
                    .map(|c| c as i32)
                    .collect::<Vec<i32>>()
            })
            .filter(|sentence| !sentence.is_empty())
            .collect()
    }
}

/// Splits a phoneme string after sentence punctuation, and at spaces where
/// a sentence is still too long.
fn split_phonemes(phonemes: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for word in phonemes.split(' ') {
        let len = current.chars().count();
        if len > 0 && len + 1 + word.chars().count() > MAX_PHONEMES {
            sentences.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        if word.ends_with(['.', '!', '?']) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }
    sentences
}

/// Pronounces unknown words by spelling rules, and short all-capital words
/// letter by letter.
pub struct RuleFallback {
    british: bool,
}

const LETTER_NAMES: [&str; 26] = [
    "ˈA",
    "bˈi",
    "sˈi",
    "dˈi",
    "ˈi",
    "ˈɛf",
    "ʤˈi",
    "ˈAʧ",
    "ˈI",
    "ʤˈA",
    "kˈA",
    "ˈɛl",
    "ˈɛm",
    "ˈɛn",
    "ˈO",
    "pˈi",
    "kjˈu",
    "ˈɑɹ",
    "ˈɛs",
    "tˈi",
    "jˈu",
    "vˈi",
    "dˈʌbᵊljˌu",
    "ˈɛks",
    "wˈI",
    "zˈi",
];

/// Letter groups and their sounds, longest first within each first letter.
const GROUPS: &[(&str, &str)] = &[
    ("tion", "ʃən"),
    ("sion", "ʒən"),
    ("ough", "O"),
    ("augh", "ɔ"),
    ("eigh", "A"),
    ("igh", "I"),
    ("tch", "ʧ"),
    ("dge", "ʤ"),
    ("sch", "sk"),
    ("sh", "ʃ"),
    ("ch", "ʧ"),
    ("th", "θ"),
    ("ph", "f"),
    ("wh", "w"),
    ("ck", "k"),
    ("ng", "ŋ"),
    ("qu", "kw"),
    ("gh", "ɡ"),
    ("ee", "i"),
    ("ea", "i"),
    ("oo", "u"),
    ("ou", "W"),
    ("ow", "O"),
    ("oa", "O"),
    ("ai", "A"),
    ("ay", "A"),
    ("ei", "A"),
    ("ey", "i"),
    ("oi", "Y"),
    ("oy", "Y"),
    ("au", "ɔ"),
    ("aw", "ɔ"),
    ("ie", "i"),
    ("ue", "u"),
    ("ew", "ju"),
    ("ar", "ɑɹ"),
    ("er", "əɹ"),
    ("ir", "əɹ"),
    ("ur", "əɹ"),
    ("or", "ɔɹ"),
];

fn is_vowel_letter(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

fn is_vowel_sound(c: char) -> bool {
    "AIOWYiuæɑɔəɛɪʊʌ".contains(c)
}

impl RuleFallback {
    fn spell(&self, word: &str) -> String {
        word.chars()
            .filter(char::is_ascii_alphabetic)
            .map(|c| match (c.to_ascii_lowercase(), self.british) {
                ('z', true) => "zˈɛd",
                (c, _) => LETTER_NAMES[(c as u8 - b'a') as usize],
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn sound_out(&self, word: &str) -> String {
        let letters: Vec<char> = word
            .to_lowercase()
            .chars()
            .filter(char::is_ascii_alphabetic)
            .collect();
        let n = letters.len();
        let mut out = String::new();
        let mut i = 0;
        while i < n {
            let rest: String = letters[i..].iter().collect();
            // A doubled consonant sounds once.
            if i + 1 < n && letters[i] == letters[i + 1] && !is_vowel_letter(letters[i]) {
                i += 1;
                continue;
            }
            if let Some((group, sound)) = GROUPS.iter().find(|(group, _)| rest.starts_with(group)) {
                out.push_str(sound);
                i += group.len();
                continue;
            }
            let c = letters[i];
            let next = letters.get(i + 1).copied();
            // "Magic e": in vowel-consonant-e at the end of a word the vowel
            // is long and the e is silent ("make", "node").
            let magic_e =
                i + 3 == n && letters[n - 1] == 'e' && next.is_some_and(|c| !is_vowel_letter(c));
            let last = i + 1 == n;
            let sound = match c {
                'a' if magic_e => "A",
                'i' if magic_e => "I",
                'o' if magic_e => "O",
                'u' if magic_e => "u",
                'e' if magic_e => "i",
                'a' => "æ",
                'e' if last && out.chars().any(is_vowel_sound) => "",
                'e' => "ɛ",
                'i' => "ɪ",
                'o' if last => "O",
                'o' => "ɑ",
                'u' => "ʌ",
                'y' if i == 0 => "j",
                'y' if last => "i",
                'y' => "ɪ",
                'c' if matches!(next, Some('e' | 'i' | 'y')) => "s",
                'c' | 'q' => "k",
                'g' => "ɡ",
                'j' => "ʤ",
                'r' => "ɹ",
                'x' => "ks",
                'b' | 'd' | 'f' | 'h' | 'k' | 'l' | 'm' | 'n' | 'p' | 's' | 't' | 'v' | 'w'
                | 'z' => {
                    out.push(c);
                    ""
                }
                _ => "",
            };
            out.push_str(sound);
            i += 1;
        }
        // Stress the first syllable.
        match out.char_indices().find(|(_, c)| is_vowel_sound(*c)) {
            Some((at, _)) => {
                out.insert(at, 'ˈ');
                out
            }
            None => self.spell(word),
        }
    }
}

impl Fallback for RuleFallback {
    fn phonemize(&self, word: &str) -> Result<String, FallbackError> {
        let letters = word.chars().filter(|c| c.is_ascii_alphabetic()).count();
        if letters == 0 {
            return Err(FallbackError::NoPhonemes {
                word: word.to_string(),
            });
        }
        let acronym = letters <= 5 && word.chars().all(|c| !c.is_lowercase());
        Ok(if acronym {
            self.spell(word)
        } else {
            self.sound_out(word)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(sentences: &[Vec<i32>]) -> String {
        sentences
            .iter()
            .map(|s| {
                s.iter()
                    .map(|&c| char::from_u32(c as u32).unwrap())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn phonemizes_words_numbers_and_abbreviations() {
        let us = Phonemizer::new(false);
        let got = text(&us.phonemize("In 2024, Dr. Smith's selected articles cost $5."));
        // Nothing is dropped: the year, the title, the possessive, the
        // inflected words and the amount are all pronounced.
        for part in [
            "twˈɛnti",
            "dˈɑktəɹ",
            "smˈɪθs",
            "səlˈɛktᵻd",
            "ˈɑɹTəkᵊlz",
            "fˈIv",
            "dˈɑləɹz",
        ] {
            assert!(got.contains(part), "missing {part} in {got}");
        }
        assert!(!got.contains('❓'));
    }

    #[test]
    fn unknown_words_are_sounded_out_not_dropped() {
        let us = Phonemizer::new(false);
        let got = text(&us.phonemize("Speachy runs kubernetes."));
        assert!(got.contains("spˈiʧi"), "{got}");
        assert!(got.contains("kˈʌbəɹnɛtɛs"), "{got}");
        let fallback = RuleFallback { british: false };
        assert_eq!(fallback.phonemize("XKCD").unwrap(), "ˈɛks kˈA sˈi dˈi");
        assert_eq!(fallback.phonemize("zoomable").unwrap(), "zˈumæbl");
        assert_eq!(fallback.phonemize("node").unwrap(), "nˈOd");
        assert!(fallback.phonemize("123").is_err());
        assert_eq!(
            RuleFallback { british: true }.phonemize("Z").unwrap(),
            "zˈɛd"
        );
    }

    #[test]
    fn british_and_american_differ() {
        let us = text(&Phonemizer::new(false).phonemize("I can't dance."));
        let gb = text(&Phonemizer::new(true).phonemize("I can't dance."));
        assert_ne!(us, gb);
    }

    #[test]
    fn sentences_are_split_and_long_ones_bounded() {
        let us = Phonemizer::new(false);
        assert_eq!(us.phonemize("One. Two! Three?").len(), 3);
        let long = "the quick brown fox jumps over the lazy dog and ".repeat(20);
        let parts = us.phonemize(&long);
        assert!(parts.len() > 1);
        assert!(
            parts.iter().all(|p| p.len() <= MAX_PHONEMES),
            "{:?}",
            parts.iter().map(Vec::len).collect::<Vec<_>>()
        );
        assert!(us.phonemize("").is_empty());
    }
}
