#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "numpy==2.5.3",
#     "torch==2.15.0",
#     "transformers==5.24.1",
#     "huggingface-hub==2.5.0",
# ]
# ///
"""How often Speachy stresses a Russian word on the wrong vowel.

    scripts/stress-eval.py marks [--since REV] [--list CLASS]
    scripts/stress-eval.py heard NAME [--cli PATH] [--home DIR] [--voice V] [--readings N]
    scripts/stress-eval.py compare A B

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

`heard` is estimated, run by hand, and needs numpy, torch and transformers (declared above, so `uv run` fetches them;
`marks` and `compare` import none of them). It has the built CLI read the gold aloud, as one document, in one process
per reading, and a Russian phoneme recognizer listens: for every word of the voice's speech it says which vowel carried
the stress. The report is how often a word was heard off its gold vowel, per class of `marks`, with a 95 % range from
resampling sentences. A run is saved as `target/stress-eval/NAME.json` with what was measured, and its audio is kept in
`target/stress-eval/NAME/`. `compare A B` pairs two saved runs word by word and says whether a difference is real.

The recognizer, `omogr/wav2vec2-lv-60-ru-ipa`, is licensed CC BY-NC-SA 4.0: nothing of it is in this repository. `heard`
fetches it (1.26 GB, once) from Hugging Face at the revision pinned below, and reads its weights with
`torch.load(weights_only=True)`. Nothing is fitted: the heard vowel is the one with the highest stress-symbol
probability before it, and a chunk that cannot be aligned counts every one of its words as heard off.

Data shapes, in the order they are used:

    Word      a run of Cyrillic letters, exactly as `stress::mark` takes a word (a hyphen ends it), and the set of
              its vowels (counted from 1) that carry the stress
    GoldLine  one sentence of the gold, its words, and the rules it breaks
    Chunk     one line of the spoken file: its language and its words
    Entry     a gold word of two or more vowels, the spoken word that answers it, and its class
    Row       an Entry as one run heard it: per reading, the vowel heard and when, in the saved audio
    Run       what a `heard` run measured, and with what; the file `compare` reads
"""
from __future__ import annotations

import argparse
import collections
import dataclasses
import datetime
import difflib
import itertools
import json
import os
import pathlib
import random
import re
import subprocess
import sys
import unicodedata
import wave

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
    chunk: int = -1            # where the spoken word is: the chunk (of the whole document) and its place in the chunk
    at: int = -1

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
    chunks: list[Chunk]        # every chunk of the spoken file, in order
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


def scored(line: GoldLine) -> list[int]:
    return [i for i, word in enumerate(line.words) if word.vowels >= 2]


def answered(line: GoldLine, found: dict[int, int]) -> bool:
    """Whether the spoken block is this sentence's. The spoken file is made from the gold, so a block that answers
    fewer than three quarters of the sentence's words of two or more vowels belongs to another sentence: the fixture
    is stale. Rewriting replaces numbers and short abbreviations, which have fewer vowels, so those words are all
    but always answered in a sentence's own block."""
    return 4 * sum(i in found for i in scored(line)) >= 3 * len(scored(line))


def stale_block(line: GoldLine, found: dict[int, int], where: str) -> StaleSpoken:
    return StaleSpoken(
        f"{where}, line {line.number}: only {sum(i in found for i in scored(line))} of its {len(scored(line))} words "
        f"of two or more vowels are answered in its block of {SPOKEN_FILE}:\n  {line.bare}\n"
        "The spoken file is stale."
    )


def pair(gold: list[GoldLine], spoken: list[list[Chunk]], where: str = GOLD_FILE) -> Paired:
    if not gold:
        raise SystemExit(f"{where} has no sentences.")
    if len(gold) != len(spoken):
        raise StaleSpoken(
            f"{where} has {len(gold)} sentences and {SPOKEN_FILE} has {len(spoken)}: the spoken file is stale."
        )
    entries, added, total, every_chunk = [], [], 0, []
    for line, chunks in zip(gold, spoken):
        heard, langs = spoken_words(chunks)
        places = [(len(every_chunk) + c, k) for c, chunk in enumerate(chunks) for k in range(len(chunk.words))]
        found = match(line.words, heard)
        if not answered(line, found):
            raise stale_block(line, found, where)
        total += len(line.words)
        for i, word in enumerate(line.words):
            if word.vowels >= 2:
                j = found.get(i)
                s, lang = (heard[j], langs[j]) if j is not None else (None, None)
                entries.append(Entry(line.number, i, line.bare, word, s, classify(word, s, lang),
                                     *(places[j] if j is not None else (-1, -1))))
        every_chunk += chunks
        matched = set(found.values())
        added += [Added(line.number, w) for j, w in enumerate(heard) if j not in matched and w.vowels >= 2]
    return Paired({line.bare for line in gold}, len(gold), total, every_chunk, entries, added)


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


# ---- the heard half: the recognizer ------------------------------------------------------------------------------------
# numpy, torch and transformers are imported inside the functions that need them, so `marks` and `compare` run on the
# standard library alone.

RECOGNIZER = "omogr/wav2vec2-lv-60-ru-ipa"
REVISION = "83df7f90c3b2e1e0c1dd0fb8dec0d6fec6e84d25"    # the commit of 2024-09-03 on Hugging Face
FILES = ("config.json", "vocab.json", "pytorch_model.bin")
RATE = 16000                                              # what the recognizer takes
FRAME = 0.02                                              # seconds per output frame (320 samples)

# What a consonant letter accepts: the recognizer's symbols whose first character is one of these.
SOUNDS = {"б": "bp", "п": "bp", "в": "vf", "ф": "vf", "г": "ɡkxv", "к": "kɡx", "х": "xkɡ", "д": "dt", "т": "dt",
          "з": "zsʐʂɕ", "с": "zsʐʂɕ", "ж": "ʐʂɕzs", "ш": "ʐʂɕzs", "щ": "ɕʂ", "ч": "tɕ", "ц": "ts", "л": "lɫ",
          "м": "mn", "н": "nm", "р": "r", "й": "j"}
VOWEL_SOUNDS = set("aeiouyəɐɛɪɔʊʌæɑɒɜɨʉɯøœɵɚᵻäõũĩãẽɤɘɞ")
# Slot kinds that are not letters: J a glide, M the stress symbol, X palatalization or length, W a word boundary,
# ч2 and ц2 the second sound of an affricate.
OTHER = {"J": lambda p: p == "j", "M": lambda p: p == "`", "X": lambda p: p in "ʲː", "W": lambda p: p == "|",
         "ч2": lambda p: p in "ɕʂ", "ц2": lambda p: p == "s"}

Slot = tuple[str, int, int | None, bool]    # what it accepts, word, vowel of the word (from 1) or None, optional


def resample(samples, rate: int):
    """Band-limited resampling to 16 kHz through the FFT, the whole signal at once."""
    import numpy as np
    x = np.asarray(samples, dtype=np.float64)
    if rate == RATE:
        return x.astype(np.float32)
    n, m = len(x), int(round(len(x) * RATE / rate))
    X = np.fft.rfft(x)
    Y = np.zeros(m // 2 + 1, dtype=complex)
    k = min(len(X), len(Y))
    Y[:k] = X[:k]
    return (np.fft.irfft(Y, m) * (m / n)).astype(np.float32)


class Listener:
    """Which vowel of each word carries the stress, as a Russian IPA recognizer hears it.

    The recognizer writes a stress symbol before the stressed vowel. Its frames (20 ms) are aligned to the letters of
    the known text by a CTC Viterbi pass: a consonant letter accepts the sounds of its broad class, a vowel letter any
    vowel; glides, palatalization, length, word boundaries and the stress symbol are optional. Where the stress should
    be does not enter the alignment, and nothing is fitted. The heard vowel of a word is the one with the highest
    stress-symbol probability between the sound before it and itself.
    """

    def __init__(self) -> None:
        import numpy as np
        import torch
        from huggingface_hub import hf_hub_download
        from transformers import Wav2Vec2Config, Wav2Vec2ForCTC
        found = {name: hf_hub_download(RECOGNIZER, name, revision=REVISION) for name in FILES}
        vocab = json.loads(pathlib.Path(found["vocab.json"]).read_text(encoding="utf-8"))
        config = Wav2Vec2Config.from_pretrained(str(pathlib.Path(found["config.json"]).parent))
        config.vocab_size = len(vocab)
        self.model = Wav2Vec2ForCTC(config).eval()
        # The weights are a pickle. weights_only=True is the restricted unpickler: tensors and plain containers only.
        self.model.load_state_dict(torch.load(found["pytorch_model.bin"], map_location="cpu", weights_only=True))

        def ids(test):    # the symbols (not the four special tokens) a slot accepts
            return np.array(sorted(i for p, i in vocab.items() if i > 3 and test(p)))

        self.sets = {"V": ids(lambda p: p[0] in VOWEL_SOUNDS)}
        self.sets.update({letter: ids(lambda p, f=firsts: p[0] in f) for letter, firsts in SOUNDS.items()})
        self.sets.update({key: ids(test) for key, test in OTHER.items()})
        assert len(self.sets["M"]) == 1, "the vocabulary has no single stress symbol"
        self.stress = int(self.sets["M"][0])

    def log_probs(self, audio):
        """Log-probabilities [frames, symbols] of 16 kHz mono audio."""
        import torch
        z = ((audio - audio.mean()) / (audio.std() + 1e-7)).astype("float32")
        with torch.inference_mode():
            logits = self.model(torch.from_numpy(z)[None]).logits[0]
        return torch.log_softmax(logits.float(), dim=-1).numpy()

    @staticmethod
    def slots(words: list[str]) -> list[Slot]:
        out: list[Slot] = []
        for wi, word in enumerate(words):
            n = 0
            if wi:
                out.append(("W", wi, None, True))
            for ch in word:
                if ch in VOWELS:
                    n += 1
                    out.append(("M", wi, None, True))
                    if ch in "еёюя":
                        out.append(("J", wi, None, True))
                    out.append(("V", wi, n, False))
                elif ch in SOUNDS:
                    out.append((ch, wi, None, False))
                    if ch in "чц":
                        out.append((ch + "2", wi, None, True))
                    out.append(("X", wi, None, True))
        return out

    def align(self, lp, slots: list[Slot]):
        """CTC Viterbi over the slots; runs of optional slots may be skipped. Returns the frames of every slot."""
        import numpy as np
        T, L = len(lp), len(slots)
        N, NEG = 2 * L + 1, -1e30
        em = np.empty((T, N), dtype=np.float32)
        em[:, 0::2] = lp[:, [0]]
        for j, s in enumerate(slots):
            m = lp[:, self.sets[s[0]]]
            mx = m.max(axis=1)
            em[:, 2 * j + 1] = mx + np.log(np.exp(m - mx[:, None]).sum(axis=1))
        opt = [s[3] for s in slots]
        preds: list[list[int]] = [[0] for _ in range(N)]
        for i in range(1, N):
            preds[i].append(1)
            if i % 2 == 1:
                j = i // 2         # slot j may follow slot j-1, or anything back to the nearest mandatory slot
                while j >= 1:
                    preds[i].append(i - (2 * (j - 1) + 1))
                    if not opt[j - 1]:
                        break
                    preds[i].append(i - 2 * (j - 1))
                    j -= 1
        K = max(max(p) for p in preds) + 1
        ok = np.zeros((K, N), dtype=bool)
        for i, p in enumerate(preds):
            for o in p:
                ok[o, i] = True
        d = np.full(N, NEG)
        d[0] = em[0, 0]
        j = 0
        while j < L:
            d[2 * j + 1] = em[0, 2 * j + 1]
            if not opt[j]:
                break
            j += 1
        back = np.zeros((T, N), dtype=np.int16)
        idx = np.arange(N)
        for t in range(1, T):
            c = np.full((K, N), NEG)
            for o in range(K):
                c[o, o:] = np.where(ok[o, o:], d[: N - o], NEG)
            b = c.argmax(axis=0)
            d = c[b, idx] + em[t]
            back[t] = b
        ends = [N - 1]
        j = L - 1
        while j >= 0:
            ends.append(2 * j + 1)
            if not opt[j]:
                break
            ends.append(2 * j)
            j -= 1
        s = max(ends, key=lambda i: d[i])
        frames: list[list[int]] = [[] for _ in slots]
        for t in range(T - 1, -1, -1):
            if s % 2 == 1:
                frames[s // 2].append(t)
            s -= back[t, s]
        return [f[::-1] for f in frames]

    def heard(self, audio, words: list[str]) -> list[tuple[int, float]] | None:
        """For 16 kHz mono `audio` of the lower-case `words`, in order: per word the vowel (from 1) before which the
        stress symbol is most probable (0 for a word with no vowel) and the second of its first sound. None when the
        audio has fewer frames than the text has sounds."""
        import numpy as np
        lp = self.log_probs(audio)
        slots = self.slots(words)
        if sum(not s[3] for s in slots) > len(lp):
            return None
        frames = self.align(lp, slots)
        solid = [(n, fr) for n, (s, fr) in enumerate(zip(slots, frames)) if not s[3]]    # letters, all placed
        marks: list[list[float]] = [[] for _ in words]
        start = [0.0] * len(words)
        seen: set[int] = set()
        for m, (n, fr) in enumerate(solid):
            kind, wi, _, _ = slots[n]
            if wi not in seen:
                seen.add(wi)
                start[wi] = fr[0] * FRAME
            if kind == "V":
                before = solid[m - 1][1][-1] + 1 if m else 0
                # rounded to four places, as the instrument was calibrated; ties go to the earlier vowel
                marks[wi].append(round(float(np.exp(lp[before: fr[0] + 1, self.stress].max())), 4))
        return [(max(range(len(mk)), key=lambda k: (mk[k], -k)) + 1 if mk else 0, t) for mk, t in zip(marks, start)]


# ---- the heard half: the app reads the gold aloud ---------------------------------------------------------------------

RUNS = ROOT / "target" / "stress-eval"


def document(gold: list[GoldLine]) -> str:
    """The gold as a reader would select it, and as `crates/speachy-core/tests/stress.rs` plans it: marks removed, ё
    printed as е, every sentence a paragraph of its own."""
    return "\n\n".join(line.bare.replace("ё", "е").replace("Ё", "Е") for line in gold)


def default_cli() -> pathlib.Path:
    """The CLI most recently built here, in the cargo target directory."""
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    built = [p for p in (target / "release" / "speachy-cli", target / "debug" / "speachy-cli") if p.exists()]
    if not built:
        raise SystemExit(f"no speachy-cli in {target}: build it (cargo build -p speachy-cli) or pass --cli PATH.")
    return max(built, key=lambda p: p.stat().st_mtime)


def cli_args(cli: pathlib.Path, home: str | None, *rest: str) -> list[str]:
    return [str(cli), *(["--home", home] if home else []), *rest]


def check_speed(cli: pathlib.Path, home: str | None) -> None:
    """The recognizer was calibrated on speech at its natural pace."""
    done = subprocess.run(cli_args(cli, home, "run", '{"cmd":"snapshot"}'), capture_output=True, text=True)
    if done.returncode:
        raise SystemExit(f"{cli} could not give its settings: {done.stderr.strip()}")
    speed = json.loads(done.stdout)["settings"]["speed"]
    if speed != 1.0:
        raise SystemExit(f"The settings of this home have speed {round(speed, 3)}. "
                         "Measure at 1.0: speachy-cli settings speed=1.0")


@dataclasses.dataclass
class Reading:
    """One reading of the whole gold document by the CLI."""
    chunks: list[dict]         # the chunks of the `started` event: what the voice was given, its language, its voice
    wav: pathlib.Path
    rate: int
    chunk_seconds: list[float]
    seconds: float             # wall time of the CLI, which is mostly synthesis


def expected_lines(spoken_text: str) -> list[str]:
    return [line for line in spoken_text.split("\n") if line and not line.startswith("#")]


def given_lines(chunks: list[dict]) -> list[str]:
    """The chunks as lines of stress-spoken.txt show them."""
    return [("" if c["lang"] == "ru" else f"[{c['lang']}] ") + (c.get("spoken") or c["text"]) for c in chunks]


def check_given(cli: pathlib.Path, chunks: list[dict], spoken_text: str) -> None:
    """Stops when what the voice is given is not what stress-spoken.txt says: the binary is not this checkout's."""
    got, want = given_lines(chunks), expected_lines(spoken_text)
    first = next((i for i in range(max(len(got), len(want))) if got[i:i + 1] != want[i:i + 1]), None)
    if first is not None:
        raise SystemExit(
            f"{cli} gives the voice other text than {SPOKEN_FILE} says, first at chunk {first + 1} "
            f"({len(got)} chunks, the fixture has {len(want)}):\n"
            f"  the fixture   {(want[first:first + 1] or ['(no such chunk)'])[0]}\n"
            f"  this binary   {(got[first:first + 1] or ['(no such chunk)'])[0]}\n"
            f"Build the CLI from this checkout (cargo build -p speachy-cli), or accept the change: {BLESS}")


def read_aloud(cli: pathlib.Path, home: str | None, voice: str | None, text: str, spoken_text: str,
               wav: pathlib.Path) -> Reading:
    """`speachy-cli --json speak [--voice V] --out WAV -` with the document on standard input: one process, one model
    load, language detection on, the text chunked as an article is. Stops when the chunks the voice was given are not
    the lines of stress-spoken.txt (this binary is not the code of this checkout), or when `chunk_seconds` does not
    add up to the file."""
    planned = subprocess.run(cli_args(cli, home, "--json", "plan", "-"), input=text.encode(), capture_output=True)
    if planned.returncode:
        raise SystemExit(f"{cli} could not plan the document: {planned.stderr.decode(errors='replace').strip()}")
    check_given(cli, json.loads(planned.stdout), spoken_text)    # in seconds, before minutes of speech
    command = cli_args(cli, home, "--json", "speak", *(["--voice", voice] if voice else []), "--out", str(wav), "-")
    began = datetime.datetime.now()
    done = subprocess.run(command, input=text.encode(), capture_output=True)
    seconds = (datetime.datetime.now() - began).total_seconds()
    if done.returncode:
        raise SystemExit(f"{cli} failed: {done.stderr.decode(errors='replace').strip()[-2000:]}")
    events = [json.loads(line) for line in done.stdout.decode().split("\n") if line.startswith("{")]
    started = next((e["data"] for e in events if e.get("data", {}).get("type") == "started"), None)
    summary = next((e for e in events if "file" in e), None)
    if started is None or summary is None:
        raise SystemExit(f"{cli} said neither `started` nor where it wrote the audio; is it older than this checkout?")
    check_given(cli, started["chunks"], spoken_text)
    chunk_seconds = summary.get("chunk_seconds")
    if not chunk_seconds or len(chunk_seconds) != len(started["chunks"]):
        raise SystemExit(f"{cli}: `chunk_seconds` has {len(chunk_seconds or [])} values for {len(started['chunks'])} "
                         "chunks. Build the CLI from this checkout; a chunk that was not played cannot be measured.")
    return Reading(started["chunks"], wav, summary["sample_rate"], chunk_seconds, seconds)


def cut(reading: Reading) -> list:
    """The audio of each chunk at 16 kHz, cut from the file by `chunk_seconds`."""
    import numpy as np
    with wave.open(str(reading.wav)) as w:
        samples = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float32) / 32768
    edges = [round(reading.rate * s) for s in itertools.accumulate(reading.chunk_seconds)]
    if abs(edges[-1] - len(samples)) > 1:
        raise SystemExit(f"{reading.wav}: chunk_seconds add up to {edges[-1]} samples, the file has {len(samples)}.")
    edges[-1] = len(samples)
    return [resample(samples[a:b], reading.rate) for a, b in zip([0, *edges], edges)]


def listen(readings: list[Reading], paired: Paired, listener: Listener) -> tuple[list[Row], list[tuple[int, int]]]:
    """Every chunk of every reading through the listener. Returns a Row for each gold word that was spoken, and the
    (reading, chunk) pairs that could not be aligned: their words count as heard off."""
    heard: dict[tuple[int, int], list[tuple[int, float]] | None] = {}
    starts = []
    for r, reading in enumerate(readings):
        starts.append([0.0, *itertools.accumulate(reading.chunk_seconds)])
        for c, audio in enumerate(cut(reading)):
            words = [w.text.lower().replace(ACUTE, "") for w in paired.chunks[c].words]
            heard[r, c] = listener.heard(audio, words)
            print(f"\r  listened to reading {r + 1}, chunk {c + 1} of {len(paired.chunks)}", end="", file=sys.stderr)
    print(file=sys.stderr)
    rows = []
    for e in paired.entries:
        if e.spoken is None:
            continue
        got = [heard[r, e.chunk] for r in range(len(readings))]
        rows.append(Row(
            e.line, e.index, e.sentence, e.gold.text, e.cls, sorted(e.gold.stressed),
            [sorted(e.spoken.marked), sorted(e.spoken.yo)], e.chunk,
            [g[e.at][0] if g else 0 for g in got],
            [round(starts[r][e.chunk] + g[e.at][1], 2) if g else round(starts[r][e.chunk], 2) for r, g in enumerate(got)]))
    return rows, sorted(k for k, v in heard.items() if v is None)


# ---- a saved run ------------------------------------------------------------------------------------------------------

@dataclasses.dataclass
class Row:
    """One gold word as a run heard it."""
    line: int                  # line of the gold file
    index: int                 # among the words of that sentence
    sentence: str              # with `index` it names the word across runs
    word: str                  # as the gold writes it
    cls: str
    stressed: list[int]        # the vowels the gold allows
    given: list[list[int]]     # what the voice is given: its marked vowels, its ё
    chunk: int                 # the chunk the word is spoken in, among `Run.given`
    heard: list[int]           # per reading: the vowel heard stressed, 0 when its chunk could not be aligned
    at: list[float]            # per reading: seconds into that reading's WAV where the word begins

    def off(self) -> list[bool]:
        return [h not in self.stressed for h in self.heard]


@dataclasses.dataclass
class Run:
    """What a `heard` run measured, and what it was measured with."""
    name: str
    made: str                  # UTC
    cli_version: str
    commit: str                # of this checkout, `+` when it has changes
    model: str
    voice: str
    recognizer: str            # name@revision
    readings: int
    seconds: float             # of speech, in one reading
    given: list[str]           # every chunk the voice was given, as a line of stress-spoken.txt
    unaligned: list[list[int]]  # [reading, chunk] pairs, both from 0
    rows: list[Row]

    def path(self) -> pathlib.Path:
        return RUNS / f"{self.name}.json"

    def save(self) -> None:
        RUNS.mkdir(parents=True, exist_ok=True)
        self.path().write_text(json.dumps(dataclasses.asdict(self), ensure_ascii=False), encoding="utf-8")

    @staticmethod
    def load(name: str) -> "Run":
        path = pathlib.Path(name) if name.endswith(".json") or "/" in name else RUNS / f"{name}.json"
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            return Run(**{**data, "rows": [Row(**r) for r in data["rows"]]})
        except (OSError, ValueError, TypeError, KeyError) as err:
            raise SystemExit(f"cannot read the run {path}: {err}") from None


# ---- numbers ----------------------------------------------------------------------------------------------------------

DRAWS = 2000
FEW = 100                  # word readings: with fewer, a share has no range and a change no verdict


def resampled(units: list, statistic) -> tuple[float, float]:
    """The 95 % range of `statistic(units)` when the units (sentences) are drawn again with replacement. The
    words of a sentence are read together, so they are not independent readings. Draws with nothing to divide by are
    skipped."""
    rng = random.Random(1)
    values = [v for v in (statistic(rng.choices(units, k=len(units))) for _ in range(DRAWS)) if v is not None]
    values.sort()
    return values[int(0.025 * len(values))], values[int(0.975 * len(values)) - 1]


def share(sentences: list[list[int]]) -> float | None:
    """The share of word readings heard off the gold vowel, in percent, from [off, readings] of each sentence."""
    off, words = sum(s[0] for s in sentences), sum(s[1] for s in sentences)
    return 100 * off / words if words else None


# ---- the heard report --------------------------------------------------------------------------------------------------

PEOPLE = ("The same recognizer hears people reading Russian off the gold vowel in 45 of 2,388 words (1.9 %, FLEURS "
          "recordings,\n  measured once with the prototype of this instrument), and it misses about one wrong stress in "
          "five on other voices.")


def rate_line(label: str, rows: list[Row], indent: int = 2) -> str:
    per: dict[int, list[int]] = {}
    for r in rows:
        s = per.setdefault(r.line, [0, 0])
        s[0] += sum(r.off())
        s[1] += len(r.heard)
    sums = list(per.values())
    off, words = sum(s[0] for s in sums), sum(s[1] for s in sums)
    head = f"{' ' * indent}{label:{29 - indent}}{n(words):>8}{n(off):>7}{share(sums):>8.1f} %"
    if words < FEW:
        return head + "   (too few words for a range)"
    low, high = resampled(sums, share)
    return f"{head}   ({low:.1f}-{high:.1f})"


def minutes(seconds: float) -> str:
    return f"{int(seconds // 60)}:{seconds % 60:04.1f}"


def heard_report(run: Run) -> str:
    sentences = len({r.line for r in run.rows})
    out = [
        f"Stress heard: {run.cli_version} ({run.commit}), voice {run.voice}, {run.readings} readings of "
        f"{count(sentences, 'sentence', 'sentences')}, {run.seconds / 60:.0f} minutes of speech each",
        f"  recognizer {run.recognizer.replace(REVISION, REVISION[:7])}, "
        + ("every chunk aligned" if not run.unaligned else
           f"{len(run.unaligned)} of {len(run.given) * run.readings} chunk readings could not be aligned: their words "
           "count as heard off"),
        "",
        f"  {'':27}{'word':>8}{'heard off the':>14}",
        f"  {'':27}{'readings':>8}{'words':>7}{'share':>10}   95 % range",
        rate_line("all words", run.rows),
    ]
    for cls in SCORED:
        rows = [r for r in run.rows if r.cls == cls]
        if rows:
            out.append(rate_line(CLASS_LABEL[cls], rows, 4))
    out += ["", f"  {PEOPLE}", ""]
    everywhere = [r for r in run.rows if all(r.off())]
    out.append(f"  Heard off the gold vowel in every reading: {count(len(everywhere), 'word', 'words')}. "
               f"Each is in the audio of reading 1, at the time shown.")
    if everywhere:
        out.append(f"     {'line':>4}  {'word':16}{'class':27}{'gold':>5}{'heard':>7}  where")
    for r in everywhere:
        wav = (RUNS / run.name / "reading-1.wav").relative_to(ROOT)
        out.append(f"     {r.line:>4}  {r.word:16}{CLASS_LABEL[r.cls]:27}{','.join(map(str, r.stressed)):>5}"
                   f"{r.heard[0]:>7}  {wav} at {minutes(r.at[0])}")
    for reading, chunk in run.unaligned:
        out.append(f"  not aligned: reading {reading + 1}, chunk {chunk + 1}")
    out += ["", f"  Saved {run.path().relative_to(ROOT)}"]
    return "\n".join(out)


# ---- compare ----------------------------------------------------------------------------------------------------------

def signed(x: float) -> str:
    return f"{x:+.1f}"


def compare_line(label: str, pairs: list[tuple[Row, Row]]) -> str:
    """Heard-off share in each run for the same words, the difference in points, and its 95 % range from drawing
    sentences again in both runs together."""
    if not pairs:
        return f"  {label:48}{0:>6}   no such words"
    per: dict[str, list[int]] = {}
    for a, b in pairs:
        s = per.setdefault(a.sentence, [0, 0, 0, 0])
        s[0] += sum(a.off())
        s[1] += len(a.heard)
        s[2] += sum(b.off())
        s[3] += len(b.heard)
    sums = list(per.values())

    def change(units: list[list[int]]) -> float | None:
        oa, na, ob, nb = (sum(u[i] for u in units) for i in range(4))
        return 100 * (ob / nb - oa / na) if na and nb else None

    before, after = (100 * sum(s[i] for s in sums) / sum(s[i + 1] for s in sums) for i in (0, 2))
    head = f"  {label:48}{len(pairs):>6}{before:>9.1f} %{after:>8.1f} %{signed(change(sums)):>8}"
    if min(sum(s[1] for s in sums), sum(s[3] for s in sums)) < FEW:
        return head + "   (too few words for a range)"
    low, high = resampled(sums, change)
    return f"{head}   ({signed(low)} to {signed(high)})   {'real' if low > 0 or high < 0 else 'within noise'}"


def compare_report(a: Run, b: Run) -> str:
    for what in ("model", "voice", "recognizer"):
        if getattr(a, what) != getattr(b, what):
            raise SystemExit(f"{a.name} and {b.name} cannot be compared: the {what} differs "
                             f"({getattr(a, what)} and {getattr(b, what)}).")
    mine = {(r.sentence, r.index): r for r in a.rows}
    pairs = [(mine[k], r) for r in b.rows if (k := (r.sentence, r.index)) in mine and mine[k].stressed == r.stressed]
    unchanged = [p for p in pairs if a.given[p[0].chunk] == b.given[p[1].chunk]]
    changed = [p for p in pairs if a.given[p[0].chunk] != b.given[p[1].chunk]]
    different = [p for p in changed if (p[0].cls, p[0].given) != (p[1].cls, p[1].given)]
    beside = [p for p in changed if (p[0].cls, p[0].given) == (p[1].cls, p[1].given)]
    only = len(a.rows) + len(b.rows) - 2 * len(pairs)
    out = [
        f"Stress heard: {b.name} ({b.commit}) against {a.name} ({a.commit})",
        f"  voice {a.voice}, recognizer {a.recognizer.replace(REVISION, REVISION[:7])}, {a.readings} readings of "
        f"{a.name} and {b.readings} of {b.name}, {count(len(pairs), 'word', 'words')} in both"
        + (f" ({only} not compared: in one run only, or the gold changed)" if only else ""),
        "",
        f"  {'':48}{'':>6}{'heard off the gold vowel':>21}",
        f"  {'':48}{'words':>6}{a.name:>11}{b.name:>10}   change, in points, with its 95 % range",
        compare_line("all words", pairs),
        compare_line("words Speachy marks differently", different),
        compare_line("words marked the same, in a chunk that changed", beside),
        compare_line("words in chunks that did not change", unchanged),
        "",
        "  The last line is the control. The voice was given those chunks letter for letter the same in both runs, so",
        "  the line shows how much two runs differ when nothing changed. A line is real when its range does not reach",
        "  zero; one range in twenty does that by chance.",
    ]
    return "\n".join(out)


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


def load_checked() -> tuple[list[GoldLine], str, Paired]:
    """The gold, the spoken file and their pairing. Stops with the message when the gold breaks its rules or the
    spoken file is not the gold's."""
    gold_text, spoken_text = read_file(GOLD_FILE), read_file(SPOKEN_FILE)
    gold = read_gold(gold_text)
    bad = [line for line in gold if line.violations]
    if bad:
        more = f"\n{len(bad) - 1} more lines break rules." if len(bad) > 1 else ""
        raise SystemExit(fail_rules(gold, bad[0], spoken_text) + more)
    try:
        return gold, spoken_text, pair(gold, read_spoken(spoken_text))
    except StaleSpoken as err:
        raise SystemExit(f"{err}\nRemake it: {BLESS}") from None


def marks(since: str | None, list_class: str | None) -> int:
    commit = resolve(since) if since else None
    _, _, now = load_checked()
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


def git_state() -> str:
    """The short hash of this checkout, with a `+` when it has changes."""
    head = git("rev-parse", "--short", "HEAD").stdout.decode().strip() or "no commit"
    return head + ("+" if git("status", "--porcelain").stdout.strip() else "")


def heard(name: str, cli: str | None, home: str | None, voice: str | None, readings: int) -> int:
    if not re.fullmatch(r"[A-Za-z0-9._-]+", name):
        raise SystemExit(f"{name!r} is not a name for a run: use letters, digits, . _ and -.")
    if readings < 1:
        raise SystemExit("--readings must be at least 1.")
    gold, spoken_text, paired = load_checked()
    binary = pathlib.Path(cli) if cli else default_cli()
    check_speed(binary, home)
    try:
        listener = Listener()
    except ImportError as err:
        raise SystemExit(f"`heard` needs numpy, torch and transformers ({err}). The versions are pinned at the top of "
                         "this file: run it with `uv run scripts/stress-eval.py heard ...`.") from None
    out = RUNS / name
    out.mkdir(parents=True, exist_ok=True)
    reads = []
    for r in range(readings):
        print(f"{binary}: reading {r + 1} of {readings}", file=sys.stderr)
        reads.append(read_aloud(binary, home, voice, document(gold), spoken_text, out / f"reading-{r + 1}.wav"))
        print(f"  spoken in {reads[-1].seconds:.0f} s", file=sys.stderr)
    rows, unaligned = listen(reads, paired, listener)
    chunks = reads[0].chunks
    version = subprocess.run([str(binary), "--version"], capture_output=True, text=True).stdout.strip()
    run = Run(
        name, datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC"), version, git_state(),
        ",".join(sorted({c["model_id"] for c in chunks})), ",".join(sorted({f"{c['model_id']}/{c['voice_id']}" for c in chunks})),
        f"{RECOGNIZER}@{REVISION}", readings, sum(reads[0].chunk_seconds), given_lines(chunks),
        [list(k) for k in unaligned], rows)
    run.save()
    print(marks_report(paired, None, None), "", heard_report(run), sep="\n")
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
    h = commands.add_parser("heard", help="what is heard when the built app reads the gold (by hand; needs the "
                                          "recognizer)")
    h.add_argument("name", help="what to call this run: it is saved as target/stress-eval/NAME.json")
    h.add_argument("--cli", metavar="PATH", help="the speachy-cli to measure (default: the one built last)")
    h.add_argument("--home", metavar="DIR", help="the home of models and settings (default: the app's own)")
    h.add_argument("--voice", metavar="V", help="model/voice to measure (default: the home's default voice)")
    h.add_argument("--readings", type=int, default=2, help="how many times to read the gold (default 2)")
    c = commands.add_parser("compare", help="whether two saved runs differ by more than noise")
    c.add_argument("a", help="the run to compare with: a name, or the path of a saved run")
    c.add_argument("b", help="the run to judge")
    args = parser.parse_args(argv)
    if args.command == "heard":
        return heard(args.name, args.cli, args.home, args.voice, args.readings)
    if args.command == "compare":
        print(compare_report(Run.load(args.a), Run.load(args.b)))
        return 0
    return marks(args.since, args.list)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
