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
updater="\"plugins\": { \"updater\": { \"pubkey\": \"$public_key\", \"dangerousInsecureTransportProtocol\": true } }"
bundle=target/release/bundle/macos
old_version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
new_version=99.0.0
case "$(uname -m)" in arm64) arch=arm64 ;; *) arch=x64 ;; esac
mkdir "$work/server" "$work/installed" "$work/tampered"

# The version users have installed. Two copies: one to update, one to offer
# a tampered update to.
npx --yes @tauri-apps/cli@2 build --bundles app --config "{ $updater }"
cp -R "$bundle/Speachy.app" "$work/installed/"
cp -R "$bundle/Speachy.app" "$work/tampered/"

# A newer version, as a signed update archive. The signature covers the
# version, so this has to be a real build of that version.
npx --yes @tauri-apps/cli@2 build --bundles app --config "{
  \"version\": \"$new_version\",
  \"bundle\": { \"createUpdaterArtifacts\": true },
  $updater
}"
archive="$work/server/Speachy-$new_version-macos-$arch.app.tar.gz"
cp "$bundle/Speachy.app.tar.gz" "$archive"
cp "$bundle/Speachy.app.tar.gz.sig" "$archive.sig"
scripts/make-update-manifest.py --version "$new_version" --base-url "http://127.0.0.1:$port" \
    --notes "A test update." --out "$work/server/latest.json" "$work/server"
(cd "$work/server" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT
sleep 1

version_of() {
    /usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$1/Contents/Info.plist"
}

# Runs the updater of the app bundle $1, giving up after two minutes.
update() {
    SPEACHY_HOME="$work/home" SPEACHY_UPDATE_ENDPOINT="http://127.0.0.1:$port/latest.json" \
        "$1/Contents/MacOS/speachy" --update &
    pid=$!
    ( sleep 120; kill "$pid" 2>/dev/null ) &
    watchdog=$!
    code=0
    wait "$pid" || code=$?
    kill "$watchdog" 2>/dev/null || true
    return "$code"
}

app="$work/installed/Speachy.app"
echo "--- installed version $(version_of "$app") is offered $new_version and installs it"
[ "$(version_of "$app")" = "$old_version" ]
update "$app"
if [ "$(version_of "$app")" != "$new_version" ]; then
    echo "error: the app is still version $(version_of "$app")" >&2
    exit 1
fi
codesign --verify --deep --strict "$app"
"$app/Contents/MacOS/speachy" --update | grep -q "is the newest version"
echo "ok: the app updated itself to $new_version and now reports being up to date"

app="$work/tampered/Speachy.app"
echo "--- an update that was altered after signing is refused"
printf 'x' >>"$archive"
if update "$app"; then
    echo "error: a tampered update was accepted" >&2
    exit 1
fi
if [ "$(version_of "$app")" != "$old_version" ]; then
    echo "error: a tampered update was installed" >&2
    exit 1
fi
echo "ok: the tampered update was refused and version $old_version is untouched"
