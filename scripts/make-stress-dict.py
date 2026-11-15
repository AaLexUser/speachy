#!/usr/bin/env python3
"""Builds the table of Russian word stress that Speachy ships.

    scripts/make-stress-dict.py OUT.bz2

The source is the dictionary of RUAccent (https://huggingface.co/ruaccent/accentuator,
MIT licence), at a fixed revision: 3.2 million word forms, each with a plus sign before its
stressed vowel, and a list of words that are stressed differently in different meanings.

Kept: words with two or more vowels whose stress does not depend on the meaning. Left out:
words with one vowel, words with ё (always stressed there) and the ambiguous ones.

The table is the kept words in byte order (Windows-1251), front-coded, compressed with bzip2:

    shared   1 byte    length of the prefix shared with the previous word; 0 every 64th word
    rest     1 byte    length of what follows the prefix
    vowel    1 byte    which vowel is stressed, counting from 1
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


def main(out):
    accents, ambiguous = fetch("accents.json.gz"), fetch("omographs.json.gz")
    words = []
    for word, stressed in accents.items():
        vowels = sum(c in VOWELS for c in word)
        if vowels < 2 or "ё" in word or word in ambiguous:
            continue
        if stressed.count("+") != 1 or not set(word) <= LETTERS or len(word) > 255:
            continue
        which = sum(c in VOWELS for c in stressed[: stressed.index("+")]) + 1
        words.append((word.encode("cp1251"), which))
    words.sort()
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
