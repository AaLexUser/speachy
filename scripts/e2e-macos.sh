#!/bin/sh
# Drives the built app on a Mac the way a user does: puts another app in
# full screen, selects text in it and presses the Speachy shortcut. Takes a
# screenshot at each step and checks the app's log.
#
#   scripts/e2e-macos.sh path/to/Speachy.app/Contents/MacOS/speachy path/to/speachy-cli [output-dir]
#
# Needs a desktop session in which `osascript` may send keystrokes
# (Accessibility permission), as on GitHub's macOS runners. It opens
# TextEdit and types shortcuts, so do not run it on a machine in use.
set -eu
app=$1
cli=$2
out=${3:-e2e}
mkdir -p "$out"
home=$(mktemp -d)
log="$out/app.log"

keys() { osascript -e "tell application \"System Events\" to keystroke \"$1\" using {$2}"; }
shot() { screencapture -x "$out/$1.png" || echo "note: no screenshot for $1"; }

# A voice to speak with.
"$cli" --home "$home" install kitten-nano-v0.2 >/dev/null

SPEACHY_HOME="$home" RUST_LOG=info "$app" >"$log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; osascript -e "tell application \"TextEdit\" to quit saving no" 2>/dev/null || true' EXIT
sleep 8

# Another app, in full screen, holding some text.
printf 'Speachy reads the selected text aloud. This sentence was selected in another app.\n' >"$home/sample.txt"
open -a TextEdit "$home/sample.txt"
sleep 4
osascript -e 'tell application "TextEdit" to activate'
sleep 1
keys f "control down, command down"
sleep 5
shot 1-other-app-full-screen

# Nothing is selected: the pill should say so, on top of the full-screen app.
keys s "control down, option down"
sleep 1.2
shot 2-pill-nothing-selected
sleep 3

# Select everything and press the shortcut: the selection is read aloud.
keys a "command down"
sleep 0.6
keys s "control down, option down"
sleep 2.5
shot 3-pill-speaking
sleep 6
shot 4-after

kill "$pid" 2>/dev/null || true
echo "--- app log ---"
cat "$log"

status=0
check() {
    if grep -q "$1" "$log"; then
        echo "ok: $2"
    else
        echo "error: $2 — not seen in the log" >&2
        status=1
    fi
}
check "notice: No text is selected" "the shortcut fired with nothing selected"
check "speaking the selection" "the selection was read from the other app"
check "first audio ready" "speech was synthesized and queued for playback"
exit "$status"
