#!/bin/sh
# Starts the desktop app with a scratch data directory, checks that it is
# still running after a while, and prints its log. Catches crashes at
# launch (tray, windows, shortcuts, engine start-up). Needs a display.
#
#   scripts/smoke-app.sh path/to/speachy [screenshot.png]
#
# RUN_PREFIX, if set, is put in front of the binary (for example
# "arch -x86_64" to run an Intel build on Apple Silicon).
set -eu
bin=$1
shot=${2:-}
home=$(mktemp -d)
log="$home/app.log"

SPEACHY_HOME="$home" RUST_LOG=info ${RUN_PREFIX:-} "$bin" >"$log" 2>&1 &
pid=$!
sleep 15

status=0
if kill -0 "$pid" 2>/dev/null; then
    echo "ok: still running after 15 s"
    # A picture of what a user would see, where the platform can take one.
    if [ -n "$shot" ] && command -v screencapture >/dev/null 2>&1; then
        screencapture -x "$shot" || echo "note: no screenshot (screen capture not permitted here)"
    fi
    kill "$pid" 2>/dev/null || true
else
    code=0
    wait "$pid" || code=$?
    echo "error: the app exited within 15 s (exit code $code)" >&2
    status=1
fi
echo "--- app log ---"
cat "$log"
exit "$status"
