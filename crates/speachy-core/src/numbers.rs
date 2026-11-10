//! Spells numbers and a few symbols out as English words.
//!
//! KittenTTS pronounces words by looking them up in a lexicon; digits and
//! symbols are not in it and would be skipped silently.

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [&str; 5] = ["", "thousand", "million", "billion", "trillion"];

fn below_thousand(n: u64, out: &mut Vec<&'static str>) {
    let (hundreds, rest) = (n / 100, n % 100);
    if hundreds > 0 {
        out.push(ONES[hundreds as usize]);
        out.push("hundred");
    }
    if rest >= 20 {
        out.push(TENS[(rest / 10) as usize]);
        if rest % 10 > 0 {
            out.push(ONES[(rest % 10) as usize]);
        }
    } else if rest > 0 || hundreds == 0 {
        out.push(ONES[rest as usize]);
    }
}

/// `1234` → "one thousand two hundred thirty four".
pub fn cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".into();
    }
    let mut groups = Vec::new();
    let mut rest = n;
    while rest > 0 {
        groups.push(rest % 1000);
        rest /= 1000;
    }
    let mut words = Vec::new();
    for (i, &group) in groups.iter().enumerate().rev() {
        if group == 0 {
            continue;
        }
        below_thousand(group, &mut words);
        if i > 0 {
            words.push(SCALES.get(i).copied().unwrap_or(""));
        }
    }
    words.join(" ")
}

/// `21` → "twenty first".
pub fn ordinal(n: u64) -> String {
    let words = cardinal(n);
    let (head, last) = words
        .rsplit_once(' ')
        .map_or(("", words.as_str()), |(h, l)| (h, l));
    let last = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    if head.is_empty() {
        last
    } else {
        format!("{head} {last}")
    }
}

fn digits(s: &str) -> String {
    s.bytes()
        .map(|b| ONES[(b - b'0') as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads a four-digit year the way people say it: 1999 → "nineteen ninety nine".
fn year(n: u64) -> String {
    let (high, low) = (n / 100, n % 100);
    match (n, low) {
        (2000..=2009, _) => cardinal(n),
        (_, 0) => format!("{} hundred", cardinal(high)),
        (_, 1..=9) => format!("{} oh {}", cardinal(high), cardinal(low)),
        _ => format!("{} {}", cardinal(high), cardinal(low)),
    }
}

fn currency(symbol: char) -> Option<(&'static str, &'static str)> {
    Some(match symbol {
        '$' => ("dollar", "dollars"),
        '€' => ("euro", "euros"),
        '£' => ("pound", "pounds"),
        '¥' => ("yen", "yen"),
        _ => return None,
    })
}

/// Replaces numbers, currency amounts and a few symbols with words.
pub fn spell_out_english(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next_is_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if let (Some(unit), true) = (currency(c), next_is_digit) {
            let (number, len) = read_number(&chars[i + 1..]);
            pad(&mut out);
            out.push_str(&number.words);
            out.push(' ');
            out.push_str(if number.is_one { unit.0 } else { unit.1 });
            i += 1 + len;
            continue;
        }
        if c.is_ascii_digit() {
            let (number, len) = read_number(&chars[i..]);
            // "v2", "A4": keep the letter and the number as separate words.
            pad(&mut out);
            out.push_str(&number.words);
            i += len;
            if chars.get(i).is_some_and(|n| n.is_alphabetic()) {
                out.push(' ');
            }
            continue;
        }
        let word = match c {
            '%' => Some("percent"),
            '&' => Some("and"),
            '+' => Some("plus"),
            '=' => Some("equals"),
            '@' => Some("at"),
            _ => None,
        };
        match word {
            Some(word) => {
                pad(&mut out);
                out.push_str(word);
                if chars.get(i + 1).is_some_and(|n| !n.is_whitespace()) {
                    out.push(' ');
                }
            }
            None => out.push(c),
        }
        i += 1;
    }
    out
}

/// Makes sure a word about to be appended is separated from what precedes it.
fn pad(out: &mut String) {
    if out
        .chars()
        .next_back()
        .is_some_and(|p| !p.is_whitespace() && !matches!(p, '(' | '[' | '"' | '-'))
    {
        out.push(' ');
    }
}

struct Number {
    words: String,
    is_one: bool,
}

/// Reads a number at the start of `chars`. Returns its words and how many
/// characters it spans.
fn read_number(chars: &[char]) -> (Number, usize) {
    let digit_run = |from: usize| {
        chars[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let mut len = digit_run(0);
    let mut integer: String = chars[..len].iter().collect();
    let mut grouped = false;
    // Thousands separators: "1,234,567".
    while len <= 3 || grouped {
        if chars.get(len) == Some(&',') && digit_run(len + 1) == 3 && integer.len() <= 15 {
            integer.extend(&chars[len + 1..len + 4]);
            len += 4;
            grouped = true;
        } else {
            break;
        }
    }
    let value: Option<u64> = if integer.len() <= 15 {
        integer.parse().ok()
    } else {
        None
    };

    // Decimal part: "3.14" (but not the period that ends a sentence). A
    // dotted run such as the version "2.0.1" is read part by part.
    if chars.get(len) == Some(&'.') && digit_run(len + 1) > 0 {
        let mut words = value.map_or_else(|| digits(&integer), cardinal);
        while chars.get(len) == Some(&'.') && digit_run(len + 1) > 0 {
            let part_len = digit_run(len + 1);
            let part: String = chars[len + 1..len + 1 + part_len].iter().collect();
            words.push_str(" point ");
            words.push_str(&digits(&part));
            len += 1 + part_len;
        }
        return (
            Number {
                words,
                is_one: false,
            },
            len,
        );
    }
    let Some(value) = value else {
        return (
            Number {
                words: digits(&integer),
                is_one: false,
            },
            len,
        );
    };
    // Ordinals: "1st", "22nd", "3rd", "4th".
    let suffix: String = chars[len..]
        .iter()
        .take(2)
        .collect::<String>()
        .to_lowercase();
    let after_suffix = chars.get(len + 2).is_some_and(|c| c.is_alphanumeric());
    if matches!(suffix.as_str(), "st" | "nd" | "rd" | "th") && !after_suffix {
        return (
            Number {
                words: ordinal(value),
                is_one: false,
            },
            len + 2,
        );
    }
    let leading_zero = integer.len() > 1 && integer.starts_with('0');
    let words = if leading_zero {
        digits(&integer)
    } else if !grouped && integer.len() == 4 && (1100..=2099).contains(&value) {
        year(value)
    } else {
        cardinal(value)
    };
    (
        Number {
            words,
            is_one: value == 1,
        },
        len,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinals_and_ordinals() {
        assert_eq!(cardinal(0), "zero");
        assert_eq!(cardinal(21), "twenty one");
        assert_eq!(cardinal(105), "one hundred five");
        assert_eq!(cardinal(1_000_000), "one million");
        assert_eq!(cardinal(12_345), "twelve thousand three hundred forty five");
        assert_eq!(ordinal(1), "first");
        assert_eq!(ordinal(12), "twelfth");
        assert_eq!(ordinal(20), "twentieth");
        assert_eq!(ordinal(23), "twenty third");
        assert_eq!(ordinal(100), "one hundredth");
    }

    #[test]
    fn spells_out_text() {
        let cases = [
            ("I have 3 apples.", "I have three apples."),
            ("It costs $5.", "It costs five dollars."),
            ("Only $1 left", "Only one dollar left"),
            ("Pi is 3.14 roughly", "Pi is three point one four roughly"),
            (
                "In 1999 and 2024, then 2005.",
                "In nineteen ninety nine and twenty twenty four, then two thousand five.",
            ),
            (
                "1,234,567 people",
                "one million two hundred thirty four thousand five hundred sixty seven people",
            ),
            ("the 2nd and 31st", "the second and thirty first"),
            ("50% off", "fifty percent off"),
            ("R&D", "R and D"),
            ("version 2.0.1", "version two point zero point one"),
            ("v2 and A4", "v two and A four"),
            ("call 007", "call zero zero seven"),
            ("2 + 2 = 4", "two plus two equals four"),
            ("3rds", "three rds"),
            ("No digits here.", "No digits here."),
        ];
        for (input, want) in cases {
            assert_eq!(spell_out_english(input), want, "{input}");
        }
    }

    #[test]
    fn huge_numbers_are_read_digit_by_digit() {
        assert_eq!(
            spell_out_english("12345678901234567890"),
            digits("12345678901234567890")
        );
    }
}
