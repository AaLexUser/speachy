#!/bin/sh
# Drives the built app on a Mac the way a user does: puts another app in
# full screen, selects text in it and presses the Speachy shortcut, with
# captions on. Clicks on a caption, which must reach the app under it, and
# reads again with the pill off. Takes a screenshot at each step and checks
# the app's log.
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
# Presses a key by its position on the keyboard. `keystroke` types a
# punctuation mark as text, which no shortcut sees.
keycode() { osascript -e "tell application \"System Events\" to key code $1 using {$2}"; }
shot() { screencapture -x "$out/$1.png" || echo "note: no screenshot for $1"; }
launch() {
    SPEACHY_HOME="$home" RUST_LOG=info "$app" >>"$log" 2>&1 &
    pid=$!
}
# The middle of the caption card, in screen points from the top left: the
# card is the bottom 46 points of Speachy's 680 by 60 captions window.
caption_point() {
    osascript -l JavaScript -e '
        ObjC.import("CoreGraphics");
        const all = ObjC.deepUnwrap(ObjC.castRefToObject(
            $.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0)));
        const w = all.find((w) => /speachy/i.test(w.kCGWindowOwnerName)
            && Math.round(w.kCGWindowBounds.Width) === 680
            && Math.round(w.kCGWindowBounds.Height) === 60);
        w ? `${w.kCGWindowBounds.X + 340} ${w.kCGWindowBounds.Y + 37}` : "";'
}
click() {
    osascript -l JavaScript -e "
        ObjC.import('CoreGraphics');
        const at = \$.CGPointMake($1, $2);
        for (const type of [\$.kCGEventLeftMouseDown, \$.kCGEventLeftMouseUp]) {
            \$.CGEventPost(\$.kCGHIDEventTap,
                \$.CGEventCreateMouseEvent(null, type, at, \$.kCGMouseButtonLeft));
            delay(0.05);
        }"
}

# A voice to speak with, and captions.
"$cli" --home "$home" install kitten-nano-v0.2 >/dev/null
"$cli" --home "$home" settings captions=true >/dev/null

: >"$log"
launch
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
shot 3-captions-and-pill-speaking

# A click on the caption goes to TextEdit under it: the selection gives way
# to the insertion point, so a typed letter is added to the text instead of
# replacing all of it. The next reading says which happened: it is two
# characters longer (a line break and the letter), not one character long.
# TextEdit itself is not asked: that needs a permission macOS asks for in a
# dialog.
point=$(caption_point || true)
if [ -n "$point" ]; then
    # shellcheck disable=SC2086
    click $point
    sleep 0.3
    osascript -e 'tell application "System Events" to keystroke "Z"'
    sleep 0.3
    echo "clicked the caption at $point and typed a letter"
else
    echo "no captions window on screen"
fi
shot 3b-after-click-on-caption

# Faster, while it speaks: the pill shows the new speed.
keycode 24 "control down, option down" # the = key
sleep 0.8
shot 4-pill-faster
sleep 6
shot 5-after

# Again with the pill off: the captions take its place.
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
"$cli" --home "$home" settings show_overlay=false start_hidden=true >/dev/null
launch
sleep 8
osascript -e 'tell application "TextEdit" to activate'
sleep 1
keys a "command down"
sleep 0.6
keys s "control down, option down"
sleep 2.5
shot 6-captions-pill-off
sleep 6

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
check "speed: 1.1" "the faster shortcut changed the speed while speaking"
# How many characters each reading had, in order.
set -- $(sed -n 's/.*speaking the selection (\([0-9]*\) characters).*/\1/p' "$log")
if [ -n "$point" ] && [ $# -ge 2 ] && [ "$2" -eq $(($1 + 2)) ]; then
    echo "ok: a click on the caption reached the app under it"
else
    echo "error: a click on the caption did not reach the app under it (readings of ${1:-no} and ${2:-no} characters)" >&2
    status=1
fi
exit "$status"
