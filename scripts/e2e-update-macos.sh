#!/bin/sh
# Checks the self-updater on a Mac, end to end, without touching the real
# release channel: builds the app with a throwaway signing key, serves an
# update for it from this machine, and has an installed copy update itself.
# Then checks that a tampered update is refused.
#
#   scripts/e2e-update-macos.sh
#
# Rebuilds target/release/bundle/macos/Speachy.app with the throwaway key,
# so run it after anything that needs the normal build.
set -eu
cd "$(dirname "$0")/.."
# The physical path: macOS temporary directories sit behind a symlink, and
# the updater refuses to replace an app that was started through one.
work=$(cd "$(mktemp -d)" && pwd -P)
port=8765

npx --yes @tauri-apps/cli@2 signer generate --ci -w "$work/test.key" >/dev/null
public_key=$(cat "$work/test.key.pub")
TAURI_SIGNING_PRIVATE_KEY=$(cat "$work/test.key")
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# The local server speaks plain HTTP, which the updater only accepts when
# told to; the release configuration never sets this.
npx --yes @tauri-apps/cli@2 build --bundles app --config "{
  \"bundle\": { \"createUpdaterArtifacts\": true },
  \"plugins\": { \"updater\": { \"pubkey\": \"$public_key\", \"dangerousInsecureTransportProtocol\": true } }
}"

bundle=target/release/bundle/macos
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
case "$(uname -m)" in arm64) arch=arm64 ;; *) arch=x64 ;; esac
mkdir "$work/server" "$work/installed"
cp "$bundle/Speachy.app.tar.gz" "$work/server/Speachy-99.0.0-macos-$arch.app.tar.gz"
cp "$bundle/Speachy.app.tar.gz.sig" "$work/server/Speachy-99.0.0-macos-$arch.app.tar.gz.sig"
scripts/make-update-manifest.py --version 99.0.0 --base-url "http://127.0.0.1:$port" \
    --notes "A test update." --out "$work/server/latest.json" "$work/server"
(cd "$work/server" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT
sleep 1

cp -R "$bundle/Speachy.app" "$work/installed/"
app="$work/installed/Speachy.app"
marker="$app/Contents/Resources/icon.icns"
[ -f "$marker" ] || { echo "error: $marker is not in the bundle" >&2; exit 1; }

# Runs the installed copy's updater, giving up after two minutes.
update() {
    SPEACHY_HOME="$work/home" SPEACHY_UPDATE_ENDPOINT="http://127.0.0.1:$port/latest.json" \
        "$app/Contents/MacOS/speachy" --update &
    pid=$!
    ( sleep 120; kill "$pid" 2>/dev/null ) &
    watchdog=$!
    code=0
    wait "$pid" || code=$?
    kill "$watchdog" 2>/dev/null || true
    return "$code"
}

echo "--- installed version $version sees version 99.0.0 and installs it"
rm "$marker"   # the update restores this file if it really replaces the app
update
[ -f "$marker" ] || { echo "error: the app bundle was not replaced" >&2; exit 1; }
codesign --verify --deep --strict "$app"
echo "ok: the app updated itself"

echo "--- an update that was altered after signing is refused"
rm "$marker"
printf 'x' >>"$work/server/Speachy-99.0.0-macos-$arch.app.tar.gz"
if update; then
    echo "error: a tampered update was accepted" >&2
    exit 1
fi
[ ! -f "$marker" ] || { echo "error: a tampered update was installed" >&2; exit 1; }
echo "ok: the tampered update was refused"
