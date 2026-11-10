//! Language identification for text, used to pick a voice.
//!
//! Selections are often a sentence or less, where statistical detectors
//! guess badly. So three signals are combined:
//!
//! 1. the **script** (Hangul, Kana, Han, Arabic, … settle the language
//!    outright; Cyrillic narrows it to three);
//! 2. **function words and distinctive letters** of each language, which
//!    work even on a handful of words;
//! 3. whatlang's trigram model, trusted when it is confident.
//!
//! When nothing is conclusive the caller falls back to the user's language.

use whatlang::{Detector, Lang};

#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    /// ISO 639-1 code.
    pub code: &'static str,
    /// Whether the evidence was strong enough to override a prior guess
    /// (such as the language of the surrounding text).
    pub reliable: bool,
}

const LANGS: &[(&str, Lang)] = &[
    ("ar", Lang::Ara),
    ("bg", Lang::Bul),
    ("cs", Lang::Ces),
    ("da", Lang::Dan),
    ("de", Lang::Deu),
    ("el", Lang::Ell),
    ("en", Lang::Eng),
    ("es", Lang::Spa),
    ("et", Lang::Est),
    ("fi", Lang::Fin),
    ("fr", Lang::Fra),
    ("hi", Lang::Hin),
    ("hr", Lang::Hrv),
    ("hu", Lang::Hun),
    ("id", Lang::Ind),
    ("it", Lang::Ita),
    ("ja", Lang::Jpn),
    ("ko", Lang::Kor),
    ("lt", Lang::Lit),
    ("lv", Lang::Lav),
    ("nl", Lang::Nld),
    ("pl", Lang::Pol),
    ("pt", Lang::Por),
    ("ro", Lang::Ron),
    ("ru", Lang::Rus),
    ("sk", Lang::Slk),
    ("sl", Lang::Slv),
    ("sv", Lang::Swe),
    ("tr", Lang::Tur),
    ("uk", Lang::Ukr),
    ("vi", Lang::Vie),
    ("zh", Lang::Cmn),
];

/// Very common words of each Latin-script language. Shared words (such as
/// "de") are fine: they simply do not separate the languages that share them.
const WORDS: &[(&str, &[&str])] = &[
    (
        "en",
        &[
            "the", "and", "of", "to", "is", "are", "was", "that", "this", "with", "for", "it",
            "you", "not", "have", "from", "at", "be", "we", "can", "will", "your", "over",
            "through", "what", "how",
        ],
    ),
    (
        "es",
        &[
            "el", "la", "los", "las", "de", "que", "y", "en", "un", "una", "es", "por", "con",
            "para", "no", "se", "del", "al", "lo", "como", "pero", "más", "esta", "este", "muy",
            "hola",
        ],
    ),
    (
        "fr",
        &[
            "le", "la", "les", "de", "des", "et", "un", "une", "est", "que", "qui", "dans", "pour",
            "pas", "sur", "ce", "ceci", "il", "elle", "nous", "vous", "avec", "au", "du", "je",
            "bonjour", "merci", "très",
        ],
    ),
    (
        "de",
        &[
            "der", "die", "das", "und", "ist", "nicht", "ein", "eine", "ich", "sie", "mit", "auf",
            "für", "den", "dem", "zu", "von", "es", "auch", "wir", "sich", "dies", "wie", "aber",
            "oder", "heute",
        ],
    ),
    (
        "it",
        &[
            "il", "la", "di", "che", "e", "è", "un", "una", "per", "non", "sono", "con", "del",
            "della", "gli", "le", "si", "come", "questo", "questa", "anche", "più", "ma", "ciao",
            "molto",
        ],
    ),
    (
        "pt",
        &[
            "o", "a", "os", "as", "de", "que", "e", "é", "um", "uma", "para", "não", "com", "em",
            "do", "da", "se", "por", "mais", "como", "mas", "isso", "você", "muito", "olá", "está",
        ],
    ),
    (
        "nl",
        &[
            "de", "het", "een", "en", "van", "is", "dat", "niet", "ik", "je", "op", "te", "voor",
            "met", "zijn", "er", "maar", "ook", "dit", "naar", "deze", "wij", "hallo", "wat",
            "hoe",
        ],
    ),
    (
        "pl",
        &[
            "i", "w", "nie", "na", "to", "jest", "się", "że", "z", "do", "jak", "co", "ale", "po",
            "tak", "dla", "czy", "ten", "być", "są", "bardzo", "cześć", "oraz", "przez",
        ],
    ),
    (
        "cs",
        &[
            "a", "je", "se", "na", "že", "to", "v", "s", "z", "do", "ale", "jako", "pro", "by",
            "jsem", "jsou", "tak", "co", "když", "nebo", "ahoj", "velmi", "který",
        ],
    ),
    (
        "sk",
        &[
            "a", "je", "sa", "na", "že", "to", "v", "s", "z", "do", "ale", "ako", "pre", "by",
            "som", "sú", "tak", "čo", "keď", "alebo", "ahoj", "veľmi", "ktorý",
        ],
    ),
    (
        "sl",
        &[
            "in", "je", "se", "na", "da", "v", "za", "z", "ne", "so", "ki", "pa", "kot", "ali",
            "tudi", "sem", "bil", "kaj", "zelo", "živjo", "lahko",
        ],
    ),
    (
        "hr",
        &[
            "i", "je", "se", "na", "da", "u", "za", "s", "ne", "su", "koji", "ali", "kao", "ili",
            "sam", "bio", "što", "vrlo", "bok", "može", "nije", "ovo",
        ],
    ),
    (
        "da",
        &[
            "og", "i", "det", "at", "en", "den", "til", "er", "som", "på", "de", "med", "han",
            "af", "for", "ikke", "der", "jeg", "et", "har", "hej", "meget", "hvad",
        ],
    ),
    (
        "sv",
        &[
            "och", "i", "det", "att", "en", "som", "är", "på", "av", "för", "med", "inte", "den",
            "har", "jag", "ett", "om", "men", "var", "hej", "mycket", "vad", "till",
        ],
    ),
    (
        "fi",
        &[
            "ja", "on", "ei", "se", "että", "oli", "hän", "mutta", "kun", "niin", "ovat", "tämä",
            "olen", "myös", "tai", "joka", "kuin", "hei", "hyvin", "mitä", "sekä",
        ],
    ),
    (
        "et",
        &[
            "ja", "on", "ei", "et", "see", "oli", "ta", "kui", "aga", "ka", "mis", "või", "ning",
            "oma", "selle", "olen", "tere", "väga", "kes", "siis",
        ],
    ),
    (
        "hu",
        &[
            "a", "az", "és", "hogy", "nem", "is", "egy", "van", "meg", "de", "ez", "volt", "már",
            "csak", "mint", "vagy", "még", "szia", "nagyon", "mit", "itt",
        ],
    ),
    (
        "ro",
        &[
            "și", "de", "la", "în", "a", "că", "este", "un", "o", "cu", "nu", "pe", "din", "sunt",
            "pentru", "care", "mai", "dar", "sau", "salut", "foarte", "ce", "acest",
        ],
    ),
    (
        "lt",
        &[
            "ir", "yra", "kad", "su", "į", "iš", "bet", "kaip", "tai", "ar", "ne", "buvo", "apie",
            "labai", "labas", "kas", "arba", "nuo", "aš", "mes",
        ],
    ),
    (
        "lv",
        &[
            "un", "ir", "ka", "ar", "no", "uz", "bet", "kā", "tas", "vai", "ne", "bija", "par",
            "ļoti", "sveiki", "kas", "es", "mēs", "arī", "lai",
        ],
    ),
    (
        "tr",
        &[
            "ve", "bir", "bu", "için", "ile", "de", "da", "çok", "ama", "gibi", "daha", "ne",
            "var", "olarak", "ben", "sen", "değil", "merhaba", "veya", "kadar", "en", "mi",
        ],
    ),
    (
        "id",
        &[
            "dan", "yang", "di", "ini", "itu", "dengan", "untuk", "tidak", "dari", "ke", "akan",
            "ada", "saya", "kami", "pada", "juga", "atau", "halo", "sangat", "apa", "adalah",
        ],
    ),
    (
        "vi",
        &[
            "và", "là", "của", "có", "không", "một", "cho", "được", "này", "trong", "với", "tôi",
            "bạn", "đã", "những", "các", "để", "xin", "chào", "rất", "người",
        ],
    ),
];

/// Letters that occur in essentially one of the supported languages.
const LETTERS: &[(&str, &str)] = &[
    ("de", "ß"),
    ("es", "ñ¿¡"),
    ("pt", "ãõ"),
    ("fr", "œùêèëîïç"),
    ("pl", "łąężźćńś"),
    ("cs", "řěů"),
    ("sk", "ľĺŕôä"),
    ("hu", "őű"),
    ("ro", "țșă"),
    ("tr", "ğış"),
    ("da", "øæ"),
    ("sv", "å"),
    ("lv", "āēīūļņķģ"),
    ("lt", "ėųįū"),
    ("et", "õ"),
    ("vi", "ạảấầẩẫậắằẳẵặẹẻẽếềểễệỉịọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹơưđ"),
];

fn code_of(lang: Lang) -> Option<&'static str> {
    LANGS
        .iter()
        .find(|(_, l)| *l == lang)
        .map(|(code, _)| *code)
}

fn whatlang_of(code: &str) -> Option<Lang> {
    LANGS.iter().find(|(c, _)| *c == code).map(|(_, l)| *l)
}

#[derive(Debug, Default)]
struct Scripts {
    latin: usize,
    cyrillic: usize,
    han: usize,
    kana: usize,
    hangul: usize,
    arabic: usize,
    devanagari: usize,
    greek: usize,
}

fn scripts(text: &str) -> Scripts {
    let mut s = Scripts::default();
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        match c as u32 {
            0x0041..=0x024F | 0x1E00..=0x1EFF => s.latin += 1,
            0x0400..=0x052F => s.cyrillic += 1,
            0x3040..=0x30FF => s.kana += 1,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => s.han += 1,
            0x1100..=0x11FF | 0xAC00..=0xD7AF => s.hangul += 1,
            0x0600..=0x06FF | 0x0750..=0x077F => s.arabic += 1,
            0x0900..=0x097F => s.devanagari += 1,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => s.greek += 1,
            _ => {}
        }
    }
    s
}

/// Detects the language of `text`, answering only with one of `candidates`
/// (ISO 639-1 codes of languages a voice exists for).
pub fn detect(text: &str, candidates: &[String]) -> Option<Detected> {
    let has = |code: &str| candidates.iter().any(|c| c == code);
    let sure = |code: &'static str| {
        has(code).then_some(Detected {
            code,
            reliable: true,
        })
    };
    let s = scripts(text);
    // CJK characters carry more per character than letters do.
    let cjk = (s.han + s.kana + s.hangul) * 2;
    let top = [s.latin, s.cyrillic, cjk, s.arabic, s.devanagari, s.greek]
        .into_iter()
        .max()
        .unwrap_or(0);
    if top == 0 {
        return None;
    }
    if top == cjk {
        return if s.hangul > s.han + s.kana {
            sure("ko")
        } else if s.kana > 0 {
            sure("ja")
        } else {
            sure("zh").or_else(|| sure("ja"))
        };
    }
    if top == s.arabic {
        return sure("ar");
    }
    if top == s.devanagari {
        return sure("hi");
    }
    if top == s.greek {
        return sure("el");
    }
    if top == s.cyrillic {
        return detect_cyrillic(text, candidates);
    }
    detect_latin(text, candidates)
}

fn detect_cyrillic(text: &str, candidates: &[String]) -> Option<Detected> {
    let options: Vec<&'static str> = ["ru", "uk", "bg"]
        .into_iter()
        .filter(|c| candidates.iter().any(|x| x == c))
        .collect();
    let pick = |code: &str, reliable: bool| {
        options
            .iter()
            .find(|c| **c == code)
            .map(|&code| Detected { code, reliable })
    };
    if let [only] = options.as_slice() {
        return Some(Detected {
            code: only,
            reliable: true,
        });
    }
    let lower = text.to_lowercase();
    if lower.contains(['і', 'ї', 'є', 'ґ']) {
        if let Some(found) = pick("uk", true) {
            return Some(found);
        }
    }
    if lower.contains(['ы', 'э', 'ё']) {
        if let Some(found) = pick("ru", true) {
            return Some(found);
        }
    }
    let allow: Vec<Lang> = options.iter().filter_map(|c| whatlang_of(c)).collect();
    let guess = Detector::with_allowlist(allow).detect(text)?;
    let code = code_of(guess.lang())?;
    // Bulgarian has no letter of its own; require the model to be confident
    // before preferring it over Russian.
    if code == "bg" && guess.confidence() < 0.5 {
        return pick("ru", false).or(Some(Detected {
            code,
            reliable: false,
        }));
    }
    Some(Detected {
        code,
        reliable: guess.is_reliable() || guess.confidence() >= 0.5,
    })
}

fn detect_latin(text: &str, candidates: &[String]) -> Option<Detected> {
    let options: Vec<&'static str> = WORDS
        .iter()
        .map(|(code, _)| *code)
        .filter(|c| candidates.iter().any(|x| x == c))
        .collect();
    match options.as_slice() {
        [] => return None,
        [only] => {
            return Some(Detected {
                code: only,
                reliable: true,
            })
        }
        _ => {}
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();

    // Votes from function words and distinctive letters.
    let mut scores: Vec<(&'static str, usize)> = options
        .iter()
        .map(|&code| {
            let list = WORDS
                .iter()
                .find(|(c, _)| *c == code)
                .map_or(&[][..], |(_, l)| *l);
            let word_hits = words.iter().filter(|w| list.contains(w)).count();
            let letters = LETTERS
                .iter()
                .find(|(c, _)| *c == code)
                .map_or("", |(_, l)| *l);
            let letter_hits = lower
                .chars()
                .filter(|c| letters.contains(*c))
                .count()
                .min(3);
            (code, word_hits + letter_hits * 2)
        })
        .collect();
    scores.sort_by_key(|score| std::cmp::Reverse(score.1));
    let (best, best_score) = scores[0];
    let runner_up = scores[1].1;

    let allow: Vec<Lang> = options.iter().filter_map(|c| whatlang_of(c)).collect();
    let guess = Detector::with_allowlist(allow).detect(text);
    let guess_code = guess.as_ref().and_then(|g| code_of(g.lang()));
    let guess_conf = guess.as_ref().map_or(0.0, |g| g.confidence());

    // Enough text for the trigram model: believe it when it is sure, and
    // when the word evidence does not clearly say otherwise.
    if let (Some(code), Some(g)) = (guess_code, &guess) {
        if g.is_reliable() && (best == code || best_score < runner_up + 3) {
            return Some(Detected {
                code,
                reliable: true,
            });
        }
    }
    if best_score >= 2 && best_score > runner_up {
        let agreed = guess_code == Some(best);
        return Some(Detected {
            code: best,
            reliable: best_score >= runner_up + 2 || agreed,
        });
    }
    if best_score >= 1 && guess_code == Some(best) {
        return Some(Detected {
            code: best,
            reliable: best_score > runner_up,
        });
    }
    match guess_code {
        Some(code) if guess_conf >= 0.6 => Some(Detected {
            code,
            reliable: false,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<String> {
        LANGS.iter().map(|(code, _)| code.to_string()).collect()
    }

    #[test]
    fn detects_sentences() {
        let cases = [
            ("The quick brown fox jumps over the lazy dog and keeps running through the forest.", "en"),
            ("Rust is a general-purpose programming language that emphasizes performance and type safety.", "en"),
            ("Привет! Это проверка синтеза речи на русском языке, сегодня хорошая погода.", "ru"),
            ("Привіт! Це перевірка синтезу мовлення українською мовою, сьогодні гарна погода.", "uk"),
            ("Bonjour, ceci est un test de synthèse vocale, il fait très beau aujourd'hui.", "fr"),
            ("Dies ist ein Test der Sprachsynthese, und das Wetter ist heute wirklich schön.", "de"),
            ("Hola, esta es una prueba de síntesis de voz y hoy hace muy buen tiempo.", "es"),
            ("Questo è un test della sintesi vocale e oggi il tempo è molto bello.", "it"),
            ("Este é um teste de síntese de voz e hoje o tempo está muito bom, não é?", "pt"),
            ("Dit is een test van de spraaksynthese en het weer is vandaag erg mooi.", "nl"),
            ("To jest test syntezy mowy, a pogoda jest dzisiaj bardzo ładna.", "pl"),
            ("Bu bir konuşma sentezi testidir ve bugün hava çok güzel.", "tr"),
            ("今天天气很好，我们一起去公园散步吧。", "zh"),
            ("こんにちは、これは音声合成のテストです。", "ja"),
            ("안녕하세요, 이것은 음성 합성 테스트입니다.", "ko"),
            ("مرحبا، هذا اختبار لتحويل النص إلى كلام.", "ar"),
            ("नमस्ते, यह वाक् संश्लेषण का परीक्षण है।", "hi"),
            ("Γεια σας, αυτή είναι μια δοκιμή σύνθεσης ομιλίας.", "el"),
        ];
        for (text, want) in cases {
            let got = detect(text, &all()).unwrap_or_else(|| panic!("no result for {text}"));
            assert_eq!(got.code, want, "{text}");
            assert!(got.reliable, "{text}");
        }
    }

    #[test]
    fn short_selections() {
        let cases = [
            ("Open the settings window", Some("en")),
            ("Meeting at 5pm tomorrow, bring the docs.", Some("en")),
            ("Hola, esta es una prueba.", Some("es")),
            ("Привет, мир", Some("ru")),
            ("¿Dónde está?", Some("es")),
            ("Straße", Some("de")),
            ("你好", Some("zh")),
            // Too little to go on: the caller's default language applies.
            ("Hello world", None),
            ("OK", None),
            ("12345 !!!", None),
        ];
        for (text, want) in cases {
            assert_eq!(detect(text, &all()).map(|d| d.code), want, "{text}");
        }
    }

    #[test]
    fn only_answers_with_a_candidate() {
        let candidates = vec!["en".to_string(), "zh".to_string()];
        assert_eq!(
            detect("Dies ist ein Test der Sprachsynthese.", &candidates)
                .unwrap()
                .code,
            "en"
        );
        assert!(detect("Привет, мир", &candidates).is_none());
        assert_eq!(
            detect("Привет, мир", &["ru".to_string(), "en".to_string()])
                .unwrap()
                .code,
            "ru"
        );
        assert!(detect("hello", &[]).is_none());
    }

    #[test]
    fn mostly_latin_text_with_a_foreign_word_stays_latin() {
        let got = detect(
            "The word 東京 means Tokyo, and it is the capital of the country.",
            &all(),
        )
        .unwrap();
        assert_eq!(got.code, "en");
    }
}
