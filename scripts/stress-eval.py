#!/usr/bin/env python3
"""How often Speachy stresses a Russian word on the wrong vowel.

    scripts/stress-eval.py marks [--since REV] [--list CLASS]

The truth is `crates/speachy-core/tests/fixtures/stress-gold.txt`: Russian sentences with the stress of every word
marked. Its header gives the rules, and says who placed the marks; this script checks the rules first.

`marks` is exact. It compares the gold with `stress-spoken.txt` beside it: the text Speachy gives the voice for those
sentences, which a test (`crates/speachy-core/tests/stress.rs`) keeps equal to the code. Every gold word with two or
more vowels falls in exactly one class:

    on the gold vowel   Speachy marked a vowel the gold allows (a restored ё counts)
    on another vowel    Speachy marked a vowel the gold does not allow: an error in the table or in the gold
    left to the voice   Speachy did not mark the word
    another language    the chunk with the word was planned in a language other than Russian
    rewritten           Speachy replaced the word, so there is nothing to compare; not scored, counted

It needs Python and nothing else, takes a second, and runs on every pull request. `--since REV` compares with the two
files at that git revision. `--list CLASS` prints every word of a class.

Data shapes, in the order they are used:

    Word      a run of Cyrillic letters, exactly as `stress::mark` takes a word (a hyphen ends it), and the set of
              its vowels (counted from 1) that carry the stress
    GoldLine  one sentence of the gold, its words, and the rules it breaks
    Chunk     one line of the spoken file: its language and its words
    Entry     a gold word of two or more vowels, the spoken word that answers it, and its class
"""
from __future__ import annotations

import argparse
import collections
import dataclasses
import difflib
import pathlib
import re
import subprocess
import sys
import unicodedata

ROOT = pathlib.Path(__file__).resolve().parent.parent
GOLD_FILE = "crates/speachy-core/tests/fixtures/stress-gold.txt"
SPOKEN_FILE = "crates/speachy-core/tests/fixtures/stress-spoken.txt"
BLESS = "SPEACHY_BLESS=1 cargo test -p speachy-core --test stress"

ACUTE = "́"
VOWELS = "аеёиоуыэюя"

# The classes of a gold word with two or more vowels, in the order the report prints them.
ON_GOLD, ON_OTHER, LEFT, LANGUAGE, REWRITTEN = "gold", "other", "left", "language", "rewritten"
CLASS_LABEL = {
    ON_GOLD: "marked on the gold vowel",
    ON_OTHER: "marked on another vowel",
    LEFT: "left to the voice",
    LANGUAGE: "read as another language",
    REWRITTEN: "rewritten, not scored",
}
SCORED = (ON_GOLD, ON_OTHER, LEFT, LANGUAGE)

# The rules of the gold file, short. The header of the file has the full text.
RULES = {
    1: "A word is a run of Russian letters; a hyphen separates two words. U+0301 is the only accent.",
    2: "Every word with two or more vowels shows where its stress is: U+0301 after the stressed vowel.",
    3: "A word with ё carries no mark, because ё is the stress, unless its stress is elsewhere.",
    4: "A word that educated speakers say both ways carries both marks, and no word carries more.",
    5: "A word with one vowel carries no mark.",
    6: "The sentences are written as printed, one on a line, none twice, ё wherever it is said.",
    7: "No Latin letters.",
}


# ---- words: the one place that knows what a word is and where its stress is written -----------------------------

@dataclasses.dataclass(frozen=True)
class Word:
    text: str                  # as written, marks and case kept
    key: str                   # lower case, marks removed, ё as е: the same word in the gold and in the spoken text
    vowels: int
    stressed: frozenset[int]   # vowels (from 1) that carry the stress: after a U+0301, or ё when the word has no mark
    marked: frozenset[int]     # vowels (from 1) followed by a U+0301
    yo: frozenset[int]         # vowels (from 1) written ё


def is_cyrillic(c: str) -> bool:
    """What `stress::mark` takes for a letter of a word."""
    return "Ѐ" <= c <= "ӿ" and c.isalpha()


def spans(text: str) -> list[tuple[int, int]]:
    """Where the words of `text` are: runs of Cyrillic letters and U+0301, as `stress::mark` cuts them."""
    found, i = [], 0
    while i < len(text):
        if not is_cyrillic(text[i]):
            i += 1
            continue
        j = i + 1
        while j < len(text) and (is_cyrillic(text[j]) or text[j] == ACUTE):
            j += 1
        found.append((i, j))
        i = j
    return found


def word_of(text: str) -> Word:
    vowels, marked, yo, previous = 0, set(), set(), ""
    for c in text.lower():
        if c in VOWELS:
            vowels += 1
            if c == "ё":
                yo.add(vowels)
        elif c == ACUTE and previous in VOWELS:
            marked.add(vowels)
        previous = c
    key = text.lower().replace(ACUTE, "").replace("ё", "е")
    return Word(text, key, vowels, frozenset(marked or yo), frozenset(marked), frozenset(yo))


def words_of(line: str) -> list[Word]:
    return [word_of(line[a:b]) for a, b in spans(line)]


# ---- the gold: sentences, and the rules that make them a truth ---------------------------------------------------

@dataclasses.dataclass(frozen=True)
class Violation:
    rule: int                  # a key of RULES
    what: str
    missing_mark: bool = False  # rule 2 broken by a word with no mark at all: the case a draft helps with


@dataclasses.dataclass(frozen=True)
class GoldLine:
    number: int                # line of the file, from 1
    text: str
    words: list[Word]
    violations: list[Violation]

    @property
    def bare(self) -> str:
        return self.text.replace(ACUTE, "")


def check_word(token: str) -> list[Violation]:
    out = []
    word = word_of(token)
    marks = token.count(ACUTE)
    has_yo = "ё" in token.lower()
    for i, c in enumerate(token):
        if c != ACUTE:
            continue
        before = token[i - 1].lower() if i else ""
        if before == "ё":
            out.append(Violation(3, f'"{token}": a mark on ё, which is stressed already'))
        elif before not in VOWELS:
            out.append(Violation(2, f'"{token}": a mark that does not follow a vowel'))
    if word.vowels < 2 and marks:
        out.append(Violation(5, f'"{token}" has {word.vowels} vowel{"s" * (word.vowels != 1)} and a stress mark'))
    elif word.vowels >= 2 and not marks and not has_yo:
        out.append(Violation(2, f'"{token}" has {word.vowels} vowels and no stress mark', missing_mark=True))
    if has_yo and marks > 1:
        out.append(Violation(3, f'"{token}" has ё and {marks} marks'))
    elif marks > 2:
        out.append(Violation(4, f'"{token}" has {marks} marks'))
    return out


def check_line(text: str) -> list[Violation]:
    out = []
    if text != text.strip():
        out.append(Violation(6, "space at an end of the line"))
    for c in dict.fromkeys(text):
        name = unicodedata.name(c, "")
        if c == ACUTE or not (c.isalpha() or unicodedata.combining(c)):
            continue
        if name.startswith("LATIN"):
            out.append(Violation(7, f"the Latin letter {c!r}"))
        elif not (c.lower() in VOWELS or ("а" <= c.lower() <= "я")) or unicodedata.combining(c):
            out.append(Violation(1, f"{c!r} (U+{ord(c):04X}) is not a Russian letter or U+0301"))
    for a, b in spans(text):
        out += check_word(text[a:b])
    for i, c in enumerate(text):
        if c == ACUTE and (i == 0 or not is_cyrillic(text[i - 1])):
            out.append(Violation(2, "a mark that does not follow a letter"))
    if not spans(text):
        out.append(Violation(1, "no Russian word"))
    return out


def read_gold(text: str) -> list[GoldLine]:
    """The sentences of the gold file, each with the rules it breaks (none when it is as it should be)."""
    lines, seen = [], {}
    for number, line in enumerate(text.split("\n"), 1):
        if not line.strip() or line.startswith("#"):
            continue
        violations = check_line(line)
        bare = line.replace(ACUTE, "")
        if bare in seen:
            violations.append(Violation(6, f"the same sentence as line {seen[bare]}"))
        seen.setdefault(bare, number)
        lines.append(GoldLine(number, line, words_of(line), violations))
    return lines


# ---- the spoken file ------------------------------------------------------------------------------------------------

@dataclasses.dataclass(frozen=True)
class Chunk:
    lang: str
    words: list[Word]


class StaleSpoken(Exception):
    """stress-spoken.txt does not belong to the gold file next to it."""


def read_spoken(text: str) -> list[list[Chunk]]:
    """For each gold sentence its chunks. A line is a chunk, `[xx] ` in front when it is not Russian, an empty line
    ends a sentence, and `#` starts a comment."""
    sentences, current = [], []
    for line in text.split("\n"):
        if line.startswith("#"):
            continue
        if not line.strip():
            if current:
                sentences.append(current)
                current = []
            continue
        m = re.match(r"\[([a-z]{2,3})\] (.*)", line)
        lang, given = (m.group(1), m.group(2)) if m else ("ru", line)
        current.append(Chunk(lang, words_of(given)))
    if current:
        sentences.append(current)
    return sentences


# ---- pairing: the one place that says which spoken word answers which gold word ------------------------------------

@dataclasses.dataclass(frozen=True)
class Entry:
    line: int                  # line of the gold file
    index: int                 # among the words of that sentence
    sentence: str              # the sentence without marks: with `index` it names the word across revisions
    gold: Word
    spoken: Word | None        # None: rewritten
    cls: str

    @property
    def given(self) -> tuple[frozenset[int], frozenset[int]] | None:
        """What the voice is given for the word: its marked vowels and its ё. None when Speachy rewrote it."""
        return None if self.spoken is None else (self.spoken.marked, self.spoken.yo)


@dataclasses.dataclass(frozen=True)
class Added:
    line: int
    word: Word


@dataclasses.dataclass
class Paired:
    bare: set[str]             # the gold sentences, without marks
    sentences: int
    gold_words: int
    entries: list[Entry]       # every gold word of two or more vowels
    added: list[Added]         # words of two or more vowels Speachy wrote where it rewrote the text

    def of(self, cls: str) -> list[Entry]:
        return [e for e in self.entries if e.cls == cls]


def match(gold: list[Word], spoken: list[Word]) -> dict[int, int]:
    """Gold word index to spoken word index. Rewriting only replaces and inserts, it never reorders, so the longest
    common runs of keys, taken in order, are the words that are the same."""
    blocks = difflib.SequenceMatcher(None, [w.key for w in gold], [w.key for w in spoken], autojunk=False)
    return {b.a + k: b.b + k for b in blocks.get_matching_blocks() for k in range(b.size)}


def spoken_words(chunks: list[Chunk]) -> tuple[list[Word], list[str]]:
    flat = [(w, c.lang) for c in chunks for w in c.words]
    return [w for w, _ in flat], [lang for _, lang in flat]


def classify(gold: Word, spoken: Word | None, lang: str | None) -> str:
    if spoken is None:
        return REWRITTEN
    if lang != "ru":
        return LANGUAGE
    if not spoken.stressed:
        return LEFT
    lost_yo = gold.yo & (spoken.stressed - spoken.yo)   # an accent on е where the gold has ё: the voice says е
    return ON_GOLD if spoken.stressed <= gold.stressed and not lost_yo else ON_OTHER


def answered(line: GoldLine, found: dict[int, int]) -> bool:
    """Whether the spoken block is this sentence's. The spoken file is made from the gold, so a block that answers
    fewer than half of the sentence's words belongs to another sentence: the fixture is stale. Rewriting replaces a
    few words of a sentence, never most of them."""
    return 2 * len(found) >= len(line.words)


def stale_block(line: GoldLine, found: dict[int, int], where: str) -> StaleSpoken:
    return StaleSpoken(
        f"{where}, line {line.number}: only {len(found)} of its {len(line.words)} words are answered in its block of "
        f"{SPOKEN_FILE}:\n  {line.bare}\nThe spoken file is stale."
    )


def pair(gold: list[GoldLine], spoken: list[list[Chunk]], where: str = GOLD_FILE) -> Paired:
    if not gold:
        raise SystemExit(f"{where} has no sentences.")
    if len(gold) != len(spoken):
        raise StaleSpoken(
            f"{where} has {len(gold)} sentences and {SPOKEN_FILE} has {len(spoken)}: the spoken file is stale."
        )
    entries, added, total = [], [], 0
    for line, chunks in zip(gold, spoken):
        heard, langs = spoken_words(chunks)
        found = match(line.words, heard)
        if not answered(line, found):
            raise stale_block(line, found, where)
        total += len(line.words)
        for i, word in enumerate(line.words):
            if word.vowels >= 2:
                j = found.get(i)
                s, lang = (heard[j], langs[j]) if j is not None else (None, None)
                entries.append(Entry(line.number, i, line.bare, word, s, classify(word, s, lang)))
        matched = set(found.values())
        added += [Added(line.number, w) for j, w in enumerate(heard) if j not in matched and w.vowels >= 2]
    return Paired({line.bare for line in gold}, len(gold), total, entries, added)


# ---- the draft of a new sentence -------------------------------------------------------------------------------------

def draft(line: GoldLine, chunks: list[Chunk]) -> tuple[str, list[str]]:
    """`line` as Speachy marks it: the marks the spoken file has for the words that have none, and ё where Speachy
    restores it. Also the words with two or more vowels that stay unmarked, which a person must decide."""
    heard, langs = spoken_words(chunks)
    found = match(line.words, heard)
    out, last, left = [], 0, []
    for i, (a, b) in enumerate(spans(line.text)):
        word = line.words[i]
        j = found.get(i)
        theirs = heard[j] if j is not None and langs[j] == "ru" else None
        needs = word.vowels >= 2 and not word.stressed and ACUTE not in word.text
        out.append(line.text[last:a])
        text = line.text[a:b]
        if needs and theirs is not None and theirs.stressed:
            at, n = next(iter(theirs.stressed)), 0
            for k, c in enumerate(text):
                if c.lower() in VOWELS:
                    n += 1
                    if n == at:
                        keep = "ё" if "ё" in theirs.text.lower() and ACUTE not in theirs.text else c
                        letter = keep.upper() if c.isupper() else keep
                        text = text[:k] + letter + (ACUTE if keep == c else "") + text[k + 1:]
                        break
        elif needs:
            left.append(word.text)
        out.append(text)
        last = b
    out.append(line.text[last:])
    return "".join(out), left


# ---- reading the files, now and at another revision -----------------------------------------------------------------

def decode(data: bytes, name: str) -> str:
    try:
        return data.decode("utf-8").replace("\r\n", "\n")
    except UnicodeDecodeError as err:
        raise SystemExit(f"{name} is not UTF-8: byte {err.start} cannot be read.") from None


def read_file(path: str) -> str:
    try:
        return decode((ROOT / path).read_bytes(), path)
    except OSError as err:
        raise SystemExit(f"cannot read {path}: {err.strerror}") from None


def git(*args: str) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(["git", *args], cwd=ROOT, capture_output=True)
    except OSError as err:
        raise SystemExit(f"--since needs git: {err.strerror}") from None


def resolve(rev: str) -> str:
    """The commit REV names, as a full hash. `--end-of-options` keeps a REV that starts with `-` from being read as
    an option, and the hash cannot start with `-` either."""
    done = git("rev-parse", "--verify", "--quiet", "--end-of-options", f"{rev}^{{commit}}")
    if done.returncode:
        raise SystemExit(f"--since {rev}: no such revision in this repository.")
    return done.stdout.decode().strip()


def show(commit: str, path: str) -> str:
    done = git("show", f"{commit}:{path}")
    if done.returncode:
        raise SystemExit(f"{commit[:12]} has no {path}: {done.stderr.decode(errors='replace').strip()}")
    return decode(done.stdout, f"{path} at {commit[:12]}")


# ---- the report ------------------------------------------------------------------------------------------------------

def n(count: int) -> str:
    return f"{count:,}"


def table(paired: Paired) -> list[str]:
    scored = sum(len(paired.of(c)) for c in SCORED)
    rows = [f"  {'':27}{'words':>8}{'share':>9}"]
    for cls in SCORED:
        count = len(paired.of(cls))
        rows.append(f"  {CLASS_LABEL[cls]:27}{n(count):>8}{100 * count / max(scored, 1):>8.1f} %")
    return rows


def count(k: int, one: str, many: str) -> str:
    return f"{n(k)} {one if k == 1 else many}"


def word_rows(entries: list[Entry], limit: int | None, indent: int = 5) -> list[str]:
    pad = " " * indent
    rows = [f"{pad}{'line':>4}  {'gold':18}{'Speachy':18}"]
    for e in entries[:limit]:
        rows.append(f"{pad}{e.line:>4}  {e.gold.text:18}{e.spoken.text if e.spoken else '-':18}")
    if limit is not None and len(entries) > limit:
        rows.append(f"{pad}... {len(entries) - limit} more (--list prints all)")
    return rows


def key(e: Entry) -> tuple[str, int]:
    return (e.sentence, e.index)


def marks_report(now: Paired, before: Paired | None, since: tuple[str, str] | None) -> str:
    multi = len(now.entries)
    scored = sum(len(now.of(c)) for c in SCORED)
    out = [
        "Stress marks: what Speachy gives the voice, against the gold",
        f"  gold     {count(now.sentences, 'sentence', 'sentences')}, {count(now.gold_words, 'word', 'words')}, "
        f"{n(multi)} with two or more vowels",
        f"  scored   {n(scored)} of them. {count(len(now.of(REWRITTEN)), 'word is', 'words are')} rewritten before "
        "being read and cannot be scored.",
        f"           Speachy adds {count(len(now.added), 'word', 'words')} of two or more vowels of its own where it "
        "rewrites the text (numbers and units, among others).",
        "           The gold has no stress for them.",
        "",
        *table(now),
    ]
    other = now.of(ON_OTHER)
    if other:
        out += ["", "  Marked on another vowel: each is an error in the stress table or in the gold, "
                "to be settled by a person.", *word_rows(other, 20)]
    if before is not None:
        out += ["", *since_report(now, before, *since)]
    return "\n".join(out)


def since_report(now: Paired, before: Paired, rev: str, commit: str) -> list[str]:
    """A word is marked differently when its class changed or what the voice is given for it changed, so a mark that
    moves from one vowel to another inside a class counts."""
    was = {key(e): e for e in before.entries}
    moved = [(was[key(e)], e) for e in now.entries
             if key(e) in was and (was[key(e)].cls != e.cls or was[key(e)].given != e.given)]
    head = f"Since {rev} ({commit[:12]})"
    if not moved:
        out = [f"{head}: no word is marked differently."]
    else:
        out = [f"{head}: {count(len(moved), 'word is', 'words are')} marked differently."]
        for cls in CLASS_LABEL:
            a, b = len(before.of(cls)), len(now.of(cls))
            if a or b:
                out.append(f"  {CLASS_LABEL[cls]:27}{n(a):>7} -> {n(b):>5}   ({b - a:+,})")
        out.append("  moved:")
        counts = collections.Counter((a.cls, b.cls) for a, b in moved)
        for (a, b), k in sorted(counts.items(), key=lambda kv: -kv[1]):
            how = f"{CLASS_LABEL[a]}, a different vowel now" if a == b else f"{CLASS_LABEL[a]} -> {CLASS_LABEL[b]}"
            out.append(f"     {n(k):>5}  {how}")
        arrived = [b for a, b in moved if b.cls == ON_OTHER]
        if arrived:
            out += ["  now marked on another vowel:", *word_rows(arrived, 30)]
    new, gone = len(now.bare - before.bare), len(before.bare - now.bare)
    if new or gone:
        out.append(f"  The gold changed: {count(new, 'sentence is', 'sentences are')} new, {n(gone)} gone. "
                   "Their words are in the counts and not among the words marked differently.")
    return out


def list_report(now: Paired, cls: str) -> str:
    if cls == "added":
        rows = [f"  {'line':>4}  {'Speachy wrote':18}"] + [f"  {a.line:>4}  {a.word.text:18}" for a in now.added]
        return f"Words Speachy wrote: {n(len(now.added))}\n" + "\n".join(rows)
    entries = now.of(cls)
    return f"{CLASS_LABEL[cls]}: {count(len(entries), 'word', 'words')}\n" + "\n".join(word_rows(entries, None, 2))


# ---- command line ----------------------------------------------------------------------------------------------------

def fail_rules(gold: list[GoldLine], bad: GoldLine, spoken_text: str) -> str:
    first = bad.violations[0]
    out = [f"{GOLD_FILE}, line {bad.number}: {first.what}.", f"  {bad.text}"]
    rest = bad.violations[1:]
    out += [f"  also: {v.what}." for v in rest[:3]]
    if len(rest) > 3:
        out.append(f"  and {len(rest) - 3} more on this line.")
    out.append(f"Rule {first.rule}: {RULES[first.rule]}")
    if any(v.missing_mark for v in bad.violations):
        sentences = read_spoken(spoken_text)
        block = sentences[gold.index(bad)] if len(sentences) == len(gold) else None
        found = match(bad.words, spoken_words(block)[0]) if block else {}
        if block is None:
            out += ["", f"No draft: {SPOKEN_FILE} does not have this sentence yet. Remake it first: {BLESS}"]
        elif not answered(bad, found):
            out += ["", f"No draft: its block of {SPOKEN_FILE} is not this sentence's. Remake it first: {BLESS}"]
        else:
            text, left = draft(bad, block)
            out += ["", "Speachy marks the sentence like this, a draft to check against its meaning and correct:",
                    f"  {text}"]
            if left:
                out.append(f"Speachy left these words alone, decide each from the sentence: {', '.join(left)}")
    return "\n".join(out)


def marks(since: str | None, list_class: str | None) -> int:
    commit = resolve(since) if since else None
    gold_text, spoken_text = read_file(GOLD_FILE), read_file(SPOKEN_FILE)
    gold = read_gold(gold_text)
    bad = [line for line in gold if line.violations]
    if bad:
        print(fail_rules(gold, bad[0], spoken_text), file=sys.stderr)
        if len(bad) > 1:
            print(f"{len(bad) - 1} more lines break rules.", file=sys.stderr)
        return 1
    try:
        now = pair(gold, read_spoken(spoken_text))
    except StaleSpoken as err:
        print(f"{err}\nRemake it: {BLESS}", file=sys.stderr)
        return 1
    before = None
    if commit:
        where = f"{GOLD_FILE} at {commit[:12]}"
        try:
            before = pair(read_gold(show(commit, GOLD_FILE)), read_spoken(show(commit, SPOKEN_FILE)), where)
        except StaleSpoken as err:
            print(f"{err}\nThe files at {since} do not belong together; use another revision.", file=sys.stderr)
            return 1
    if list_class:
        print(list_report(now, list_class))
    else:
        print(marks_report(now, before, (since, commit) if since and commit else None))
    return 0


def main(argv: list[str]) -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    m = commands.add_parser("marks", help="what Speachy marks, against the gold: exact, no models")
    m.add_argument("--since", metavar="REV", help="compare with the two files at this git revision")
    m.add_argument("--list", metavar="CLASS", choices=[*CLASS_LABEL, "added"],
                   help="print every word of a class: " + ", ".join([*CLASS_LABEL, "added"]))
    args = parser.parse_args(argv)
    return marks(args.since, args.list)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
