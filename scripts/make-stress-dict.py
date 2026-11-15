#!/usr/bin/env python3
"""Builds the table of Russian word stress that Speachy ships.

    scripts/make-stress-dict.py OUT.bz2

The source is the dictionary of RUAccent (https://huggingface.co/ruaccent/accentuator,
MIT licence), at a fixed revision: 3.2 million word forms, each with a plus sign before its
stressed vowel; a list of words that are stressed differently in different meanings; and the
words that are printed with е where ё is meant.

Kept: words with two or more vowels whose stress does not depend on the meaning, and words
printed with е that can only be read with one ё. Left out: words with one vowel and no ё to
restore, words already written with ё (always stressed there) and the ambiguous ones.

The table is the kept words in byte order (Windows-1251), front-coded, compressed with bzip2:

    shared   1 byte    length of the prefix shared with the previous word; 0 every 64th word
    rest     1 byte    length of what follows the prefix
    vowel    1 byte    which vowel is stressed, counting from 1; with the top bit set, that
                       vowel is an е to be read ё
    bytes    `rest` bytes of the word, Windows-1251
"""
import bz2, gzip, json, sys, urllib.request

REVISION = "b78ae5ea1e62beaf138bed1865cd8c3b0b5ca855"
SOURCE = f"https://huggingface.co/ruaccent/accentuator/resolve/{REVISION}/dictionary"
VOWELS = "аеёиоуыэюя"
LETTERS = set("абвгдежзийклмнопрстуфхцчшщъыьэюя-")
BLOCK = 64


def fetch(name):
    with urllib.request.urlopen(f"{SOURCE}/{name}") as response:
        return json.loads(gzip.decompress(response.read()).decode("utf-8"))


YO = 0x80


def main(out):
    accents, ambiguous = fetch("accents.json.gz"), fetch("omographs.json.gz")
    with_yo = fetch("yo_words.json.gz")
    # "берет" is "берёт" or "бере́т": marking either reading would be a guess.
    ambiguous = set(ambiguous) | set(fetch("yo_homographs.json.gz"))
    entries = {}
    for word, stressed in accents.items():
        vowels = sum(c in VOWELS for c in word)
        if vowels < 2 or "ё" in word or word in ambiguous or word in with_yo:
            continue
        if stressed.count("+") != 1 or not set(word) <= LETTERS or len(word) > 255:
            continue
        entries[word] = sum(c in VOWELS for c in stressed[: stressed.index("+")]) + 1
    for word, restored in with_yo.items():
        places = [i for i, (a, b) in enumerate(zip(word, restored)) if a != b]
        if len(places) != 1 or word in ambiguous or not set(word) <= LETTERS or len(word) > 255:
            continue
        entries[word] = YO | (sum(c in VOWELS for c in word[: places[0]]) + 1)
    words = sorted((word.encode("cp1251"), which) for word, which in entries.items())
    table, previous = bytearray(), b""
    for i, (word, which) in enumerate(words):
        shared = 0
        if i % BLOCK:
            while shared < min(len(word), len(previous)) and word[shared] == previous[shared]:
                shared += 1
        rest = word[shared:]
        table += bytes([shared, len(rest), which]) + rest
        previous = word
    packed = bz2.compress(bytes(table), 9)
    with open(out, "wb") as f:
        f.write(packed)
    print(f"{len(words)} words, {len(table)} bytes, {len(packed)} compressed", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1])
