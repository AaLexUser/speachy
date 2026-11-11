#!/bin/sh
# Checks a built speachy-cli end to end: installs the smallest model into a
# scratch directory and synthesizes a sentence with it. Downloads ~33 MB.
#
#   scripts/smoke-cli.sh path/to/speachy-cli
#
# RUN_PREFIX, if set, is put in front of the binary (for example
# "arch -x86_64" to run an Intel build on Apple Silicon).
set -eu
bin=$1
home=$(mktemp -d)
run() { ${RUN_PREFIX:-} "$bin" --home "$home" "$@"; }

run --version
run install kitten-nano-v0.2
run plan "Release builds are checked before they are published."
run speak --out "$home/out.wav" "Release builds are checked before they are published."
# Two seconds of 24 kHz 16-bit audio is 96 kB; an empty or tiny file means
# nothing was synthesized.
size=$(wc -c <"$home/out.wav")
if [ "$size" -lt 96000 ]; then
    echo "error: only $size bytes of audio were produced" >&2
    exit 1
fi
echo "ok: synthesized $size bytes of audio"
