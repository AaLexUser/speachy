#!/bin/sh
# The engine and the command-line tool must build without any user-interface
# toolkit, so they can be driven and tested headlessly. Fails if either
# picks up a dependency on one, or if the engine depends on a front end.
set -eu
cd "$(dirname "$0")/.."

ui_crates='^(tauri|tauri-[a-z-]+|wry|tao|gtk|webkit2gtk|tray-icon|global-hotkey)$'
status=0
for crate in speachy-core speachy-cli; do
    found=$(cargo tree -p "$crate" -e normal --prefix none | cut -d' ' -f1 | sort -u | grep -E "$ui_crates" || true)
    if [ -n "$found" ]; then
        echo "error: $crate depends on user-interface crates:" $found >&2
        status=1
    fi
done
if cargo tree -p speachy-core -e normal --prefix none | cut -d' ' -f1 | grep -qE '^(speachy|speachy-cli)$'; then
    echo "error: speachy-core depends on a front end" >&2
    status=1
fi
[ "$status" -eq 0 ] && echo "layers ok: speachy-core and speachy-cli are free of UI dependencies"
exit "$status"
