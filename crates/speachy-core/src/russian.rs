//! Writes Russian numbers, units and abbreviations out as words.
//!
//! The Russian voice misreads digits, unit symbols and abbreviations. A
//! number's words depend on its neighbours: the word before it sets the case,
//! and the noun after it sets the gender and is itself inflected by the
//! number. This module forms the nominative and the genitive of cardinals,
//! and the ordinals that years and dates need. Where another case would be
//! needed, the digits stay: the voice reads digits better than wrong grammar.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Nominative,
    Genitive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrdinalCase {
    Nominative,
    Genitive,
    Dative,
    Prepositional,
    Instrumental,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gender {
    Masculine,
    Feminine,
    Neuter,
}

/// The form a noun takes after a number: after 1, after 2 to 4, after 5 and
/// up. The second is also the genitive singular.
#[derive(Clone, Copy)]
enum Form {
    One,
    Few,
    Many,
}

struct Noun {
    forms: [&'static str; 3],
    gender: Gender,
}

impl Noun {
    fn form(&self, form: Form) -> &'static str {
        self.forms[form as usize]
    }
}

const fn noun(one: &'static str, few: &'static str, many: &'static str, gender: Gender) -> Noun {
    Noun {
        forms: [one, few, many],
        gender,
    }
}

const THOUSAND: Noun = noun("тысяча", "тысячи", "тысяч", Gender::Feminine);
const MILLION: Noun = noun("миллион", "миллиона", "миллионов", Gender::Masculine);
const BILLION: Noun = noun("миллиард", "миллиарда", "миллиардов", Gender::Masculine);
const TRILLION: Noun = noun("триллион", "триллиона", "триллионов", Gender::Masculine);
const RUBLE: Noun = noun("рубль", "рубля", "рублей", Gender::Masculine);
const DOLLAR: Noun = noun("доллар", "доллара", "долларов", Gender::Masculine);
const EURO: Noun = noun("евро", "евро", "евро", Gender::Masculine);

struct Unit {
    written: &'static str,
    noun: Noun,
    /// тыс., млн …: may be followed by another unit ("тыс. ₽").
    magnitude: bool,
}

const fn unit(written: &'static str, noun: Noun) -> Unit {
    Unit {
        written,
        noun,
        magnitude: false,
    }
}

const fn magnitude(written: &'static str, noun: Noun) -> Unit {
    Unit {
        written,
        noun,
        magnitude: true,
    }
}

/// Keyed by the written form, lowercase, without the abbreviation's period.
/// A bare "г", "с" or "т" after a number is ambiguous and left out.
const UNITS: &[Unit] = &[
    unit(
        "км/ч",
        noun(
            "километр в час",
            "километра в час",
            "километров в час",
            Gender::Masculine,
        ),
    ),
    unit(
        "м/с",
        noun(
            "метр в секунду",
            "метра в секунду",
            "метров в секунду",
            Gender::Masculine,
        ),
    ),
    unit(
        "км",
        noun("километр", "километра", "километров", Gender::Masculine),
    ),
    unit("м", noun("метр", "метра", "метров", Gender::Masculine)),
    unit(
        "см",
        noun("сантиметр", "сантиметра", "сантиметров", Gender::Masculine),
    ),
    unit(
        "мм",
        noun("миллиметр", "миллиметра", "миллиметров", Gender::Masculine),
    ),
    unit(
        "кг",
        noun("килограмм", "килограмма", "килограммов", Gender::Masculine),
    ),
    unit("л", noun("литр", "литра", "литров", Gender::Masculine)),
    unit("ч", noun("час", "часа", "часов", Gender::Masculine)),
    unit("мин", noun("минута", "минуты", "минут", Gender::Feminine)),
    unit(
        "сек",
        noun("секунда", "секунды", "секунд", Gender::Feminine),
    ),
    unit(
        "%",
        noun("процент", "процента", "процентов", Gender::Masculine),
    ),
    unit(
        "°c",
        noun(
            "градус Цельсия",
            "градуса Цельсия",
            "градусов Цельсия",
            Gender::Masculine,
        ),
    ),
    unit(
        "°с",
        noun(
            "градус Цельсия",
            "градуса Цельсия",
            "градусов Цельсия",
            Gender::Masculine,
        ),
    ),
    unit(
        "°",
        noun("градус", "градуса", "градусов", Gender::Masculine),
    ),
    unit("руб", RUBLE),
    unit("₽", RUBLE),
    unit("$", DOLLAR),
    unit("€", EURO),
    unit(
        "коп",
        noun("копейка", "копейки", "копеек", Gender::Feminine),
    ),
    unit(
        "чел",
        noun("человек", "человека", "человек", Gender::Masculine),
    ),
    magnitude("тыс", THOUSAND),
    magnitude("млн", MILLION),
    magnitude("млрд", BILLION),
    magnitude("трлн", TRILLION),
];

fn currency(symbol: char) -> Option<&'static Noun> {
    match symbol {
        '$' => Some(&DOLLAR),
        '€' => Some(&EURO),
        '₽' => Some(&RUBLE),
        _ => None,
    }
}

/// Where an abbreviation can stand, which decides what happens to its period.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// May end a sentence ("и т. д."): a period before a capital stays.
    Last,
    /// Always followed by something ("ул. Ленина").
    Inside,
    /// Only an abbreviation directly before a number ("д. 5").
    BeforeNumber,
}

const ABBREVIATIONS: &[(&str, &str, Place)] = &[
    ("т.д.", "так далее", Place::Last),
    ("т.п.", "тому подобное", Place::Last),
    ("и др.", "и другие", Place::Last),
    ("т.е.", "то есть", Place::Inside),
    ("т.к.", "так как", Place::Inside),
    ("в т.ч.", "в том числе", Place::Inside),
    ("напр.", "например", Place::Inside),
    ("ул.", "улица", Place::Inside),
    ("ок.", "около", Place::Inside),
    ("д.", "дом", Place::BeforeNumber),
    ("кв.", "квартира", Place::BeforeNumber),
    ("стр.", "страница", Place::BeforeNumber),
];

/// Words after which a number is in the genitive.
const GENITIVE: &[&str] = &[
    "до",
    "от",
    "около",
    "более",
    "менее",
    "свыше",
    "после",
    "без",
    "из",
    "для",
    "порядка",
    "больше",
    "меньше",
];

/// Words after which a number would need a case this module does not form.
const OTHER_CASE: &[&str] = &["с", "к", "между", "над", "под", "перед"];

const MONTHS: &[&str] = &[
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

/// Nominative and genitive; one and two in the masculine.
const SMALL: [(&str, &str); 20] = [
    ("ноль", "нуля"),
    ("один", "одного"),
    ("два", "двух"),
    ("три", "трёх"),
    ("четыре", "четырёх"),
    ("пять", "пяти"),
    ("шесть", "шести"),
    ("семь", "семи"),
    ("восемь", "восьми"),
    ("девять", "девяти"),
    ("десять", "десяти"),
    ("одиннадцать", "одиннадцати"),
    ("двенадцать", "двенадцати"),
    ("тринадцать", "тринадцати"),
    ("четырнадцать", "четырнадцати"),
    ("пятнадцать", "пятнадцати"),
    ("шестнадцать", "шестнадцати"),
    ("семнадцать", "семнадцати"),
    ("восемнадцать", "восемнадцати"),
    ("девятнадцать", "девятнадцати"),
];
const TENS: [(&str, &str); 10] = [
    ("", ""),
    ("", ""),
    ("двадцать", "двадцати"),
    ("тридцать", "тридцати"),
    ("сорок", "сорока"),
    ("пятьдесят", "пятидесяти"),
    ("шестьдесят", "шестидесяти"),
    ("семьдесят", "семидесяти"),
    ("восемьдесят", "восьмидесяти"),
    ("девяносто", "девяноста"),
];
const HUNDREDS: [(&str, &str); 10] = [
    ("", ""),
    ("сто", "ста"),
    ("двести", "двухсот"),
    ("триста", "трёхсот"),
    ("четыреста", "четырёхсот"),
    ("пятьсот", "пятисот"),
    ("шестьсот", "шестисот"),
    ("семьсот", "семисот"),
    ("восемьсот", "восьмисот"),
    ("девятьсот", "девятисот"),
];

fn pick(forms: (&'static str, &'static str), case: Case) -> &'static str {
    match case {
        Case::Nominative => forms.0,
        Case::Genitive => forms.1,
    }
}

fn small(n: u64, case: Case, gender: Gender) -> &'static str {
    match (n, case, gender) {
        (1, Case::Nominative, Gender::Feminine) => "одна",
        (1, Case::Nominative, Gender::Neuter) => "одно",
        (1, Case::Genitive, Gender::Feminine) => "одной",
        (2, Case::Nominative, Gender::Feminine) => "две",
        _ => pick(SMALL[n as usize % 20], case),
    }
}

fn below_thousand(n: u64, case: Case, gender: Gender, out: &mut Vec<&'static str>) {
    let (hundreds, rest) = (n / 100 % 10, n % 100);
    if hundreds > 0 {
        out.push(pick(HUNDREDS[hundreds as usize], case));
    }
    if rest >= 20 {
        out.push(pick(TENS[(rest / 10) as usize], case));
        if rest % 10 > 0 {
            out.push(small(rest % 10, case, gender));
        }
    } else if rest > 0 {
        out.push(small(rest, case, gender));
    }
}

fn form(n: u64, case: Case) -> Form {
    let (last, last_two) = (n % 10, n % 100);
    match case {
        _ if (11..=14).contains(&last_two) => Form::Many,
        Case::Nominative if last == 1 => Form::One,
        Case::Nominative if (2..=4).contains(&last) => Form::Few,
        Case::Genitive if last == 1 => Form::Few,
        _ => Form::Many,
    }
}

/// `n` up to 999 999 999 999.
fn cardinal(n: u64, case: Case, gender: Gender) -> String {
    if n == 0 {
        return pick(SMALL[0], case).to_string();
    }
    let mut words = Vec::new();
    for (scale, noun) in [
        (1_000_000_000, &BILLION),
        (1_000_000, &MILLION),
        (1_000, &THOUSAND),
    ] {
        let group = n / scale % 1000;
        if group == 0 {
            continue;
        }
        // "тысяча", not "одна тысяча"; but "один миллион".
        if !(scale == 1_000 && group == 1) {
            below_thousand(group, case, noun.gender, &mut words);
        }
        words.push(noun.form(form(group, case)));
    }
    below_thousand(n % 1000, case, gender, &mut words);
    words.join(" ")
}

#[derive(Clone, Copy)]
enum Ending {
    /// пятый, пятого
    Hard,
    /// второй, второго
    Stressed,
    /// третий, третьего
    Soft,
}

const ORDINAL_SMALL: [(&str, Ending); 20] = [
    ("нулев", Ending::Stressed),
    ("перв", Ending::Hard),
    ("втор", Ending::Stressed),
    ("трет", Ending::Soft),
    ("четвёрт", Ending::Hard),
    ("пят", Ending::Hard),
    ("шест", Ending::Stressed),
    ("седьм", Ending::Stressed),
    ("восьм", Ending::Stressed),
    ("девят", Ending::Hard),
    ("десят", Ending::Hard),
    ("одиннадцат", Ending::Hard),
    ("двенадцат", Ending::Hard),
    ("тринадцат", Ending::Hard),
    ("четырнадцат", Ending::Hard),
    ("пятнадцат", Ending::Hard),
    ("шестнадцат", Ending::Hard),
    ("семнадцат", Ending::Hard),
    ("восемнадцат", Ending::Hard),
    ("девятнадцат", Ending::Hard),
];
const ORDINAL_TENS: [(&str, Ending); 10] = [
    ("", Ending::Hard),
    ("", Ending::Hard),
    ("двадцат", Ending::Hard),
    ("тридцат", Ending::Hard),
    ("сороков", Ending::Stressed),
    ("пятидесят", Ending::Hard),
    ("шестидесят", Ending::Hard),
    ("семидесят", Ending::Hard),
    ("восьмидесят", Ending::Hard),
    ("девяност", Ending::Hard),
];
const ORDINAL_HUNDREDS: [&str; 10] = [
    "",
    "сот",
    "двухсот",
    "трёхсот",
    "четырёхсот",
    "пятисот",
    "шестисот",
    "семисот",
    "восьмисот",
    "девятисот",
];

fn ending(ending: Ending, case: OrdinalCase) -> &'static str {
    match (ending, case) {
        (Ending::Hard, OrdinalCase::Nominative) => "ый",
        (Ending::Stressed, OrdinalCase::Nominative) => "ой",
        (Ending::Soft, OrdinalCase::Nominative) => "ий",
        (Ending::Soft, OrdinalCase::Genitive) => "ьего",
        (Ending::Soft, OrdinalCase::Dative) => "ьему",
        (Ending::Soft, OrdinalCase::Prepositional) => "ьем",
        (Ending::Soft, OrdinalCase::Instrumental) => "ьим",
        (_, OrdinalCase::Genitive) => "ого",
        (_, OrdinalCase::Dative) => "ому",
        (_, OrdinalCase::Prepositional) => "ом",
        (_, OrdinalCase::Instrumental) => "ым",
    }
}

/// Masculine ordinal; only the last word is an ordinal ("две тысячи шестом").
fn ordinal(n: u64, case: OrdinalCase) -> String {
    let rest = n % 1000;
    let (head, stem, kind) = if rest == 0 {
        let thousands = n / 1000;
        let prefix = if thousands == 1 {
            String::new()
        } else {
            cardinal(thousands, Case::Genitive, Gender::Masculine)
        };
        (0, format!("{prefix}тысячн"), Ending::Hard)
    } else if rest.is_multiple_of(100) {
        let hundreds = ORDINAL_HUNDREDS[(rest / 100) as usize];
        (n - rest, hundreds.to_string(), Ending::Hard)
    } else if rest % 100 < 20 || rest.is_multiple_of(10) {
        let last = rest % 100;
        let (stem, kind) = if last < 20 {
            ORDINAL_SMALL[last as usize]
        } else {
            ORDINAL_TENS[(last / 10) as usize]
        };
        (n - last, stem.to_string(), kind)
    } else {
        let (stem, kind) = ORDINAL_SMALL[(rest % 10) as usize];
        (n - rest % 10, stem.to_string(), kind)
    };
    let last = format!("{stem}{}", ending(kind, case));
    if head == 0 {
        last
    } else {
        format!(
            "{} {last}",
            cardinal(head, Case::Nominative, Gender::Masculine)
        )
    }
}

#[derive(Clone, Copy)]
enum Amount {
    Whole(u64),
    Decimal {
        whole: u64,
        fraction: u64,
        places: usize,
    },
}

/// Nominative singular, genitive singular, plural.
const WHOLE_PART: [&str; 3] = ["целая", "целой", "целых"];
const DECIMAL_PLACES: [[&str; 3]; 3] = [
    ["десятая", "десятой", "десятых"],
    ["сотая", "сотой", "сотых"],
    ["тысячная", "тысячной", "тысячных"],
];

fn part(n: u64, case: Case, forms: [&'static str; 3]) -> &'static str {
    match case {
        _ if n % 10 != 1 || n % 100 == 11 => forms[2],
        Case::Nominative => forms[0],
        Case::Genitive => forms[1],
    }
}

impl Amount {
    fn form(self, case: Case) -> Form {
        match self {
            Amount::Whole(n) => form(n, case),
            // "около двух с половиной тысяч", "менее полутора тысяч": a
            // half in the genitive takes the plural, unlike other decimals
            // ("до двух целых трёх десятых процента").
            Amount::Decimal {
                whole: 1..,
                fraction: 5,
                places: 1,
            } if case == Case::Genitive => Form::Many,
            Amount::Decimal { .. } => Form::Few,
        }
    }

    fn words(self, case: Case, gender: Gender) -> String {
        match self {
            Amount::Whole(n) => cardinal(n, case, gender),
            Amount::Decimal {
                whole: 1,
                fraction: 5,
                places: 1,
            } => match (case, gender) {
                (Case::Genitive, _) => "полутора",
                (_, Gender::Feminine) => "полторы",
                _ => "полтора",
            }
            .to_string(),
            Amount::Decimal {
                whole,
                fraction: 5,
                places: 1,
            } if whole >= 1 => format!("{} с половиной", cardinal(whole, case, gender)),
            Amount::Decimal {
                whole,
                fraction,
                places,
            } => format!(
                "{} {} {} {}",
                cardinal(whole, case, Gender::Feminine),
                part(whole, case, WHOLE_PART),
                cardinal(fraction, case, Gender::Feminine),
                part(fraction, case, DECIMAL_PLACES[places.clamp(1, 3) - 1]),
            ),
        }
    }

    /// The gender of a following word that is not in the unit table, from
    /// its ending.
    fn gender_before(self, case: Case, word: &str) -> Gender {
        let last = word.chars().last();
        let feminine_plural = matches!(last, Some('ы' | 'и'));
        let (ends_in_one, ends_in_two) = match self {
            Amount::Whole(n) => (n % 10 == 1 && n % 100 != 11, n % 10 == 2 && n % 100 != 12),
            Amount::Decimal { .. } if feminine_plural => return Gender::Feminine,
            Amount::Decimal { .. } => return Gender::Masculine,
        };
        match case {
            Case::Nominative if ends_in_two && feminine_plural => Gender::Feminine,
            Case::Nominative if ends_in_one && matches!(last, Some('а' | 'я')) => {
                Gender::Feminine
            }
            Case::Nominative if ends_in_one && matches!(last, Some('о' | 'е')) => Gender::Neuter,
            // The word after a genitive is itself genitive: "одной книги".
            Case::Genitive if ends_in_one && feminine_plural => Gender::Feminine,
            _ => Gender::Masculine,
        }
    }
}

enum Figure {
    Amount(Amount),
    Time { hours: u64, minutes: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Digits,
    Space,
    Symbol,
}

#[derive(Clone, Copy)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
}

impl Token<'_> {
    fn symbol(&self) -> Option<char> {
        (self.kind == Kind::Symbol)
            .then(|| self.text.chars().next())
            .flatten()
    }
}

fn kind_of(c: char) -> Kind {
    if c.is_ascii_digit() {
        Kind::Digits
    } else if c.is_alphabetic() {
        Kind::Word
    } else if c.is_whitespace() {
        Kind::Space
    } else {
        Kind::Symbol
    }
}

/// Runs of letters, digits and whitespace; every other character alone.
fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let kind = kind_of(c);
        let mut end = start + c.len_utf8();
        if kind != Kind::Symbol {
            while let Some(&(j, next)) = chars.peek() {
                if kind_of(next) != kind {
                    break;
                }
                end = j + next.len_utf8();
                chars.next();
            }
        }
        tokens.push(Token {
            kind,
            text: &text[start..end],
        });
    }
    tokens
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    SentenceStart,
    AfterStop,
    Inside,
}

struct Writer {
    out: String,
    /// The last word written, lowercase, if nothing but space followed it.
    previous: Option<String>,
    position: Position,
}

impl Writer {
    fn copy(&mut self, token: Token) {
        self.out.push_str(token.text);
        match token.kind {
            Kind::Space => {
                if self.position == Position::AfterStop {
                    self.position = Position::SentenceStart;
                }
            }
            Kind::Word => {
                self.previous = Some(token.text.to_lowercase());
                self.position = Position::Inside;
            }
            Kind::Digits | Kind::Symbol => {
                self.previous = None;
                self.position = if matches!(token.symbol(), Some('.' | '!' | '?' | '…')) {
                    Position::AfterStop
                } else {
                    Position::Inside
                };
            }
        }
    }

    fn copy_all(&mut self, tokens: &[Token]) {
        for &token in tokens {
            self.copy(token);
        }
    }

    fn say(&mut self, words: &str, capitalize: bool) {
        let mut chars = words.chars();
        match chars.next() {
            Some(first) if capitalize => {
                self.out.extend(first.to_uppercase());
                self.out.push_str(chars.as_str());
            }
            _ => self.out.push_str(words),
        }
        self.previous = words.rsplit(' ').next().map(str::to_lowercase);
        self.position = Position::Inside;
    }

    fn stop(&mut self) {
        self.out.push('.');
        self.previous = None;
        self.position = Position::AfterStop;
    }

    fn previous_is(&self, words: &[&str]) -> bool {
        self.previous
            .as_deref()
            .is_some_and(|word| words.contains(&word))
    }
}

/// Writes out what a Russian voice cannot read: numbers, units, abbreviations.
pub fn spell_out(text: &str) -> String {
    let tokens = tokenize(text);
    let mut writer = Writer {
        out: String::with_capacity(text.len() * 2),
        previous: None,
        position: Position::SentenceStart,
    };
    let mut i = 0;
    while i < tokens.len() {
        i = match abbreviation(&tokens, i, &mut writer) {
            Some(next) => next,
            None => match number(&tokens, i, &mut writer) {
                Some(next) => next,
                None => {
                    writer.copy(tokens[i]);
                    i + 1
                }
            },
        };
    }
    writer.out
}

/// Matches `pattern` (lowercase) at `tokens[i]`, allowing a space after
/// each period inside it ("т. д."). Returns the index after the match.
fn matches(tokens: &[Token], i: usize, pattern: &str) -> Option<usize> {
    let pattern = tokenize(pattern);
    let mut k = i;
    for (n, expected) in pattern.iter().enumerate() {
        let token = tokens.get(k)?;
        let same = token.kind == expected.kind
            && (token.kind == Kind::Space || token.text.to_lowercase() == expected.text);
        if !same {
            return None;
        }
        k += 1;
        let word_follows = pattern.get(n + 1).is_some_and(|p| p.kind == Kind::Word);
        if expected.text == "."
            && word_follows
            && tokens.get(k).is_some_and(|t| t.kind == Kind::Space)
        {
            k += 1;
        }
    }
    Some(k)
}

fn next_visible<'a>(tokens: &[Token<'a>], k: usize) -> Option<Token<'a>> {
    tokens
        .get(k..)?
        .iter()
        .find(|t| t.kind != Kind::Space)
        .copied()
}

/// Whether an abbreviation's period, consumed up to `k`, also ends the sentence.
fn ends_sentence(tokens: &[Token], k: usize, can_end: bool) -> bool {
    match next_visible(tokens, k) {
        None => true,
        Some(next) => {
            can_end && next.kind == Kind::Word && next.text.starts_with(char::is_uppercase)
        }
    }
}

fn abbreviation(tokens: &[Token], i: usize, writer: &mut Writer) -> Option<usize> {
    let token = tokens[i];
    if token.text == "№" {
        writer.say("номер", false);
        if tokens.get(i + 1).is_some_and(|t| t.kind != Kind::Space) {
            writer.out.push(' ');
        }
        return Some(i + 1);
    }
    if token.kind != Kind::Word {
        return None;
    }
    let capitalize = token.text.starts_with(char::is_uppercase);
    if let Some(end) = matches(tokens, i, "г.") {
        let city = next_visible(tokens, end)
            .is_some_and(|t| t.kind == Kind::Word && t.text.starts_with(char::is_uppercase));
        if city {
            let spoken = if writer.previous_is(&["в"]) {
                "городе"
            } else {
                "город"
            };
            writer.say(spoken, capitalize);
            return Some(end);
        }
    }
    for &(written, spoken, place) in ABBREVIATIONS {
        let Some(end) = matches(tokens, i, written) else {
            continue;
        };
        let before_number = next_visible(tokens, end).is_some_and(|t| t.kind == Kind::Digits);
        if place == Place::BeforeNumber && !before_number {
            continue;
        }
        writer.say(spoken, capitalize);
        if ends_sentence(tokens, end, place == Place::Last) {
            writer.stop();
        }
        return Some(end);
    }
    None
}

fn is_group_space(token: &Token) -> bool {
    matches!(token.text, " " | "\u{a0}" | "\u{2009}" | "\u{202f}")
}

/// Reads the figure starting at the digits `tokens[i]`. `Err` holds the end
/// of a run to leave as written: a version, a long digit string.
fn figure(tokens: &[Token], i: usize) -> Result<(Figure, usize), usize> {
    let digits = |k: usize| {
        tokens
            .get(k)
            .filter(|t| t.kind == Kind::Digits)
            .map(|t| t.text)
    };
    let symbol = |k: usize| tokens.get(k).and_then(Token::symbol);
    let separator = |k: usize| matches!(symbol(k), Some('.' | ',')) && digits(k + 1).is_some();
    let first = tokens[i].text;

    if let (true, Some(':'), Some(minutes)) = (first.len() <= 2, symbol(i + 1), digits(i + 2)) {
        let seconds = symbol(i + 3) == Some(':') && digits(i + 4).is_some();
        let (hours, minutes_value) = (
            first.parse::<u64>().unwrap_or(99),
            minutes.parse::<u64>().unwrap_or(99),
        );
        if minutes.len() == 2 && !seconds && hours <= 23 && minutes_value <= 59 {
            let time = Figure::Time {
                hours,
                minutes: minutes_value,
            };
            return Ok((time, i + 3));
        }
    }

    let mut whole = first.to_string();
    let mut k = i + 1;
    if first.len() <= 3 {
        while tokens.get(k).is_some_and(is_group_space)
            && digits(k + 1).is_some_and(|d| d.len() == 3)
        {
            whole.push_str(tokens[k + 1].text);
            k += 2;
        }
    }
    let leading_zero = whole.len() > 1 && whole.starts_with('0');
    if whole.len() > 12 || leading_zero {
        return Err(k);
    }
    let whole_value: u64 = whole.parse().map_err(|_| k)?;
    if !separator(k) {
        return Ok((Figure::Amount(Amount::Whole(whole_value)), k));
    }
    if separator(k + 2) {
        while separator(k) {
            k += 2;
        }
        return Err(k);
    }
    let fraction = tokens[k + 1].text;
    k += 2;
    if fraction.len() > 3 {
        return Err(k);
    }
    let decimal = Amount::Decimal {
        whole: whole_value,
        fraction: fraction.parse().map_err(|_| k)?,
        places: fraction.len(),
    };
    Ok((Figure::Amount(decimal), k))
}

fn is_dash(token: Option<&Token>) -> bool {
    matches!(token.and_then(Token::symbol), Some('-' | '–' | '—'))
}

/// The unit at `k` (after one optional space) and the index after it,
/// with whether the unit's period also ended the sentence.
fn unit_at(tokens: &[Token], k: usize) -> Option<(&'static Unit, usize, bool)> {
    let space = tokens
        .get(k)
        .is_some_and(|t| t.kind == Kind::Space && !t.text.contains('\n'));
    let at = k + usize::from(space);
    let (unit, mut end) = UNITS
        .iter()
        .filter_map(|unit| Some((unit, matches(tokens, at, unit.written)?)))
        .max_by_key(|&(_, end)| end)?;
    let mut stop = false;
    if tokens[end - 1].kind == Kind::Word && tokens.get(end).and_then(Token::symbol) == Some('.') {
        end += 1;
        stop = ends_sentence(tokens, end, true);
    }
    Some((unit, end, stop))
}

/// A year or a date: the noun after the number makes it an ordinal.
struct Ordinal {
    case: OrdinalCase,
    /// The noun to write after the number, if it is not copied as written.
    noun: Option<&'static str>,
    end: usize,
    stop: bool,
}

fn ordinal_noun(
    tokens: &[Token],
    at: usize,
    values: (u64, u64),
    writer: &Writer,
) -> Option<Ordinal> {
    let word = tokens.get(at).filter(|t| t.kind == Kind::Word)?;
    let lower = word.text.to_lowercase();
    let after_k = writer.previous_is(&["к"]);
    let within = |range: std::ops::RangeInclusive<u64>| {
        range.contains(&values.0) && range.contains(&values.1)
    };
    let copied = |case| {
        Some(Ordinal {
            case,
            noun: None,
            end: at + 1,
            stop: false,
        })
    };
    if within(1..=31) && MONTHS.contains(&lower.as_str()) {
        return copied(if after_k {
            OrdinalCase::Dative
        } else {
            OrdinalCase::Genitive
        });
    }
    if !within(1000..=2999) {
        return None;
    }
    match lower.as_str() {
        "год" => return copied(OrdinalCase::Nominative),
        "года" => return copied(OrdinalCase::Genitive),
        "году" if after_k => return copied(OrdinalCase::Dative),
        "году" => return copied(OrdinalCase::Prepositional),
        "годом" => return copied(OrdinalCase::Instrumental),
        _ => {}
    }
    let (end, plural) = match matches(tokens, at, "г.") {
        Some(end) => (end, false),
        None => (matches(tokens, at, "гг.")?, true),
    };
    let (case, singular, plural_noun) = if writer.previous_is(&["в"]) {
        (OrdinalCase::Prepositional, "году", "годах")
    } else if after_k {
        (OrdinalCase::Dative, "году", "годам")
    } else if writer.previous_is(GENITIVE) || writer.previous_is(&["с"]) {
        (OrdinalCase::Genitive, "года", "годов")
    } else {
        (OrdinalCase::Nominative, "год", "годы")
    };
    Some(Ordinal {
        case,
        noun: Some(if plural { plural_noun } else { singular }),
        end,
        stop: ends_sentence(tokens, end, true),
    })
}

/// Whether one of `words` comes within the next six words or numbers.
fn ahead(tokens: &[Token], i: usize, words: &[&str]) -> bool {
    tokens[i..]
        .iter()
        .filter(|t| matches!(t.kind, Kind::Word | Kind::Digits))
        .take(6)
        .any(|t| words.contains(&t.text.to_lowercase().as_str()))
}

fn number(tokens: &[Token], i: usize, writer: &mut Writer) -> Option<usize> {
    let symbol = |k: usize| tokens.get(k).and_then(Token::symbol);
    let is_digits = |k: usize| tokens.get(k).is_some_and(|t| t.kind == Kind::Digits);
    let is_word = |k: usize| tokens.get(k).is_some_and(|t| t.kind == Kind::Word);

    let minus_allowed =
        i == 0 || tokens[i - 1].kind == Kind::Space || matches!(symbol(i - 1), Some('(' | '['));
    let (minus, prefix, start) = match symbol(i) {
        Some('-' | '−') if is_digits(i + 1) && minus_allowed => (true, None, i + 1),
        Some(c) if is_digits(i + 1) => (false, Some(currency(c)?), i + 1),
        _ if is_digits(i) => (false, None, i),
        _ => return None,
    };
    if start > 0 && is_word(start - 1) {
        return None;
    }
    let (first, mut k) = match figure(tokens, start) {
        Ok(read) => read,
        Err(end) => {
            writer.copy_all(&tokens[i..end]);
            return Some(end);
        }
    };
    if is_word(k) {
        return None;
    }

    let joined = |k: usize| is_dash(tokens.get(k)) && is_digits(k + 1);
    if joined(k) && joined(k + 2) {
        // A phone number or an ISO date, not a range.
        while joined(k) {
            k += 2;
        }
        writer.copy_all(&tokens[i..k]);
        return Some(k);
    }
    let mut range = None;
    if let (Figure::Amount(_), true) = (&first, joined(k)) {
        if let Ok((Figure::Amount(second), end)) = figure(tokens, k + 1) {
            if !is_word(end) {
                range = Some((tokens[k].text, second));
                k = end;
            }
        }
    }
    let capitalize = writer.position == Position::SentenceStart;
    let dash = range.map_or("", |(dash, _)| dash);
    let gap = k + usize::from(tokens.get(k).is_some_and(|t| t.kind == Kind::Space));

    let whole = |figure: &Figure| match figure {
        Figure::Amount(Amount::Whole(n)) => Some(*n),
        _ => None,
    };
    let values = match (whole(&first), range) {
        (Some(a), None) => Some((a, a)),
        (Some(a), Some((_, Amount::Whole(b)))) => Some((a, b)),
        _ => None,
    };
    if let (false, None, Some(values)) = (minus, prefix, values) {
        if let Some(ordinal_noun) = ordinal_noun(tokens, gap, values, writer) {
            let mut words = ordinal(values.0, ordinal_noun.case);
            if range.is_some() {
                words = format!("{words}{dash}{}", ordinal(values.1, ordinal_noun.case));
            }
            let noun = ordinal_noun.noun.unwrap_or(tokens[gap].text);
            writer.say(&format!("{words} {noun}"), capitalize);
            if ordinal_noun.stop {
                writer.stop();
            }
            return Some(ordinal_noun.end);
        }
    }

    let case = if writer.previous_is(GENITIVE)
        || (writer.previous_is(&["с"]) && ahead(tokens, i, &["до", "по"]))
    {
        Case::Genitive
    } else if writer.previous_is(OTHER_CASE) {
        writer.copy_all(&tokens[i..k]);
        return Some(k);
    } else {
        Case::Nominative
    };
    let sign = if minus { "минус " } else { "" };

    let amount = match first {
        Figure::Amount(amount) => amount,
        Figure::Time { hours, minutes } => {
            if prefix.is_some() {
                writer.copy_all(&tokens[i..k]);
                return Some(k);
            }
            let minutes = match minutes {
                0 => "ноль ноль".to_string(),
                1..=9 => format!("ноль {}", cardinal(minutes, case, Gender::Feminine)),
                _ => cardinal(minutes, case, Gender::Feminine),
            };
            let hours = cardinal(hours, case, Gender::Masculine);
            writer.say(&format!("{sign}{hours} {minutes}"), capitalize);
            return Some(k);
        }
    };
    let last = range.map_or(amount, |(_, second)| second);
    let mut nouns = Vec::new();
    let mut end = k;
    let mut stop = false;
    let mut gender = Gender::Masculine;
    let unit = unit_at(tokens, k).filter(|(unit, ..)| prefix.is_none() || unit.magnitude);
    if let Some((unit, unit_end, unit_stop)) = unit {
        nouns.push(unit.noun.form(last.form(case)));
        gender = unit.noun.gender;
        (end, stop) = (unit_end, unit_stop);
        if let Some(currency) = prefix {
            nouns.push(currency.form(Form::Many));
        } else if let Some((second, second_end, second_stop)) =
            unit_at(tokens, end).filter(|(second, ..)| unit.magnitude && !second.magnitude && !stop)
        {
            nouns.push(second.noun.form(Form::Many));
            (end, stop) = (second_end, second_stop);
        }
    } else if let Some(currency) = prefix {
        nouns.push(currency.form(last.form(case)));
        gender = currency.gender;
    } else if let Some(word) = tokens.get(gap).filter(|t| t.kind == Kind::Word) {
        gender = last.gender_before(case, &word.text.to_lowercase());
    }

    let mut words = format!("{sign}{}", amount.words(case, gender));
    if let Some((dash, second)) = range {
        words.push_str(dash);
        words.push_str(&second.words(case, gender));
    }
    for noun in nouns {
        words.push(' ');
        words.push_str(noun);
    }
    writer.say(&words, capitalize);
    if stop {
        writer.stop();
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_out_article_sentences() {
        let cases = [
            (
                "Выручка выросла на 15% и достигла 3,5 млрд рублей.",
                "Выручка выросла на пятнадцать процентов и достигла три с половиной миллиарда рублей.",
            ),
            (
                "Около 40 млн человек нуждаются в помощи.",
                "Около сорока миллионов человек нуждаются в помощи.",
            ),
            (
                "Температура опустится до −20 °C, а скорость ветра составит 15 м/с.",
                "Температура опустится до минус двадцати градусов Цельсия, а скорость ветра составит пятнадцать метров в секунду.",
            ),
            (
                "Поезд разгоняется до 250 км/ч и проходит 650 км за 4 ч.",
                "Поезд разгоняется до двухсот пятидесяти километров в час и проходит шестьсот пятьдесят километров за четыре часа.",
            ),
            (
                "Цена выросла с 1,2 тыс. до 2,75 тыс. рублей, т. е. более чем вдвое.",
                "Цена выросла с одной целой двух десятых тысячи до двух целых семидесяти пяти сотых тысячи рублей, то есть более чем вдвое.",
            ),
            (
                "Работаем с 9 до 18 ч.",
                "Работаем с девяти до восемнадцати часов.",
            ),
            (
                "Он пришёл с 2 друзьями и подошёл к 5 гостям.",
                "Он пришёл с 2 друзьями и подошёл к 5 гостям.",
            ),
            (
                "В магазине есть хлеб и т. д., а также соки и т. п.",
                "В магазине есть хлеб и так далее, а также соки и тому подобное.",
            ),
            (
                "хлеб и т.д., соки и т.п.",
                "хлеб и так далее, соки и тому подобное.",
            ),
            (
                "Офис находится по адресу: ул. Ленина, д. 5, кв. 12.",
                "Офис находится по адресу: улица Ленина, дом пять, квартира двенадцать.",
            ),
            (
                "Закон № 152 вступил в силу в 2006 г.",
                "Закон номер сто пятьдесят два вступил в силу в две тысячи шестом году.",
            ),
            (
                "Смартфон стоит $999, или около 90 тыс. ₽.",
                "Смартфон стоит девятьсот девяносто девять долларов, или около девяноста тысяч рублей.",
            ),
            (
                "Вес не должен превышать 2,5 кг, а длина — 60 см.",
                "Вес не должен превышать два с половиной килограмма, а длина — шестьдесят сантиметров.",
            ),
            (
                "Инфляция замедлилась до 4,3%.",
                "Инфляция замедлилась до четырёх целых трёх десятых процента.",
            ),
            (
                "Осталось менее 2,5 кг и около 1,5 тыс. рублей.",
                "Осталось менее двух с половиной килограммов и около полутора тысяч рублей.",
            ),
            (
                "Население составляет 1,4 млн чел., из них 52% — женщины.",
                "Население составляет одна целая четыре десятых миллиона человек, из них пятьдесят два процента — женщины.",
            ),
            (
                "Проект оценивается в €1,5 млрд и займёт 4 года.",
                "Проект оценивается в полтора миллиарда евро и займёт четыре года.",
            ),
            (
                "Встреча пройдёт 12 марта в 18:30.",
                "Встреча пройдёт двенадцатого марта в восемнадцать тридцать.",
            ),
            (
                "К 1 сентября всё будет готово.",
                "К первому сентября всё будет готово.",
            ),
            (
                "В 2026 году компания выйдет на рынок.",
                "В две тысячи двадцать шестом году компания выйдет на рынок.",
            ),
            (
                "С 2020 года цены выросли.",
                "С две тысячи двадцатого года цены выросли.",
            ),
            (
                "К 2030 году построят мост.",
                "К две тысячи тридцатому году построят мост.",
            ),
            (
                "2000 год был високосным.",
                "Двухтысячный год был високосным.",
            ),
            (
                "Это случилось в 1995 г. в г. Москве.",
                "Это случилось в тысяча девятьсот девяносто пятом году в городе Москве.",
            ),
            (
                "Перед 1900 годом такого не было.",
                "Перед тысяча девятисотым годом такого не было.",
            ),
            (
                "У него 2 книги и 1 ручка.",
                "У него две книги и одна ручка.",
            ),
            ("Осталось 21 место.", "Осталось двадцать одно место."),
            (
                "В классе 22 ученика и 12 парт.",
                "В классе двадцать два ученика и двенадцать парт.",
            ),
            (
                "Он сделал 1 000 000 шагов.",
                "Он сделал один миллион шагов.",
            ),
            ("Скорость — 0 км/ч.", "Скорость — ноль километров в час."),
            (
                "Доля выросла на 0,5%.",
                "Доля выросла на ноль целых пять десятых процента.",
            ),
            (
                "Это стоит от 5 до 10 тыс. рублей.",
                "Это стоит от пяти до десяти тысяч рублей.",
            ),
            ("Пришло более 100 человек.", "Пришло более ста человек."),
            (
                "Пришёл 101 человек, а ждали до 1001 гостя.",
                "Пришёл сто один человек, а ждали до тысячи одного гостя.",
            ),
            ("Это займёт 3–5 лет.", "Это займёт три–пять лет."),
            (
                "Рост составил 10-15%.",
                "Рост составил десять-пятнадцать процентов.",
            ),
            ("Версия 2.0.1 вышла вчера.", "Версия 2.0.1 вышла вчера."),
            (
                "Позвоните по номеру 88005553535123.",
                "Позвоните по номеру 88005553535123.",
            ),
            (
                "Новый iPhone 15 уже в продаже.",
                "Новый iPhone пятнадцать уже в продаже.",
            ),
            (
                "Начало в 9:05, конец в 19:00.",
                "Начало в девять ноль пять, конец в девятнадцать ноль ноль.",
            ),
            (
                "Цена: 5 ₽, 1 ₽, 2 руб. и 21 руб.",
                "Цена: пять рублей, один рубль, два рубля и двадцать один рубль.",
            ),
            (
                "Тираж: 1 тыс., 2 млн, 5 млрд.",
                "Тираж: одна тысяча, два миллиона, пять миллиардов.",
            ),
            (
                "Ждать 1 мин, 22 мин или 5 мин.",
                "Ждать одна минута, двадцать две минуты или пять минут.",
            ),
            ("На улице −5°.", "На улице минус пять градусов."),
            (
                "Яблоки, груши и др. фрукты, в т. ч. сливы, напр. венгерка.",
                "Яблоки, груши и другие фрукты, в том числе сливы, например венгерка.",
            ),
            ("Длина ок. 100 м.", "Длина около ста метров."),
            ("Привет, как дела?", "Привет, как дела?"),
            ("Hello, world 2.", "Hello, world два."),
        ];
        let failures: Vec<String> = cases
            .iter()
            .filter_map(|&(input, want)| {
                let got = spell_out(input);
                (got != want).then(|| format!("{input}\n  want: {want}\n  got:  {got}"))
            })
            .collect();
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn phone_numbers_and_iso_dates_stay_as_written() {
        for text in ["Звоните 8-800-555-35-35.", "Срок 2024-01-15."] {
            assert_eq!(spell_out(text), text);
        }
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let long: String = "1234567,".repeat(1250);
        let inputs = [
            "",
            "12345678901234567890",
            "1,,2",
            "--5",
            "№",
            "%",
            "1.",
            ",5",
            "5%%",
            long.as_str(),
        ];
        for input in inputs {
            spell_out(input);
        }
    }

    #[test]
    fn mixed_fragments_do_not_panic() {
        let fragments = [
            "1",
            "21",
            "2000",
            "999999999999",
            "0",
            "05",
            " ",
            "\u{a0}",
            ",",
            ".",
            ":",
            "-",
            "−",
            "–",
            "%",
            "°",
            "C",
            "$",
            "€",
            "₽",
            "№",
            "г",
            "гг",
            "году",
            "марта",
            "к",
            "с",
            "до",
            "тыс",
            "км",
            "/",
            "ч",
            "т",
            "д",
            "и",
            "др",
            "в",
            "Москве",
            "\n",
            "(",
            "й",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..20_000 {
            let mut input = String::new();
            for _ in 0..(seed % 12) {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                input.push_str(fragments[(seed >> 33) as usize % fragments.len()]);
            }
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            spell_out(&input);
        }
    }
}
