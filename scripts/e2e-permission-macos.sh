#!/bin/sh
# Checks, on a Mac, that Accessibility access given to one build of the app
# still holds after it is replaced by a newer build: what a user goes
# through when the app updates itself.
#
#   scripts/e2e-permission-macos.sh OLD.app NEW.app
#
# OLD.app and NEW.app are two different builds signed with the same
# certificate (scripts/macos-signing.sh). The certificate does not have to
# be on this machine, as it is not on a user's.
#
# Two checks:
#   1. the signature: NEW.app meets the requirement macOS records for
#      OLD.app when access is granted;
#   2. the real thing: OLD.app is granted access the way System Settings
#      does it, is replaced by NEW.app, and NEW.app is asked whether it has
#      access. As a control, a copy of NEW.app signed ad hoc must not.
#
# The second check writes to the system's permission database, which needs
# `sudo` and System Integrity Protection off, as on GitHub's macOS runners.
# Where that is not possible it stops with exit code 75 after the first.
set -eu
old=$(cd "$1" && pwd -P)
new=$(cd "$2" && pwd -P)
work=$(cd "$(mktemp -d)" && pwd -P)
database="/Library/Application Support/com.apple.TCC/TCC.db"
id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$old/Contents/Info.plist")

requirement_of() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p'; }
build_of() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }

echo "--- 1. the signature"
requirement=$(requirement_of "$old")
echo "macOS records for the installed app: $requirement"
case "$requirement" in
"" | *cdhash*)
    echo "error: this build is signed ad hoc, so its permissions end with it" >&2
    exit 1
    ;;
esac
if [ "$(build_of "$old")" = "$(build_of "$new")" ]; then
    echo "error: the two apps are the same build; nothing would be tested" >&2
    exit 1
fi
codesign --verify -R="$requirement" "$new"
echo "ok: the newer build ($(build_of "$new")) meets it; the installed one is $(build_of "$old")"

echo "--- 2. the permission itself"
sw_vers
csrutil status || true

# Starts the app bundle $1 the way Finder does, so that it answers for its
# own permissions, and prints what it logs about Accessibility access:
# "granted" or "not granted". $2 names the log.
ask() {
    log="$work/$2.log"
    : >"$log"
    open -n --env "SPEACHY_HOME=$work/home" --env RUST_LOG=info \
        --stdout "$log" --stderr "$log" "$1" --args --hidden
    tries=0
    until grep -q "accessibility access:" "$log"; do
        tries=$((tries + 1))
        if [ "$tries" -gt 80 ]; then
            echo "error: the app did not report within 40 s" >&2
            cat "$log" >&2
            break
        fi
        sleep 0.5
    done
    pkill -f "$1/Contents/MacOS/" 2>/dev/null || true
    tries=0
    while pgrep -f "$1/Contents/MacOS/" >/dev/null 2>&1 && [ "$tries" -lt 40 ]; do
        tries=$((tries + 1))
        sleep 0.5
    done
    sed -n 's/.*accessibility access: //p' "$log" | head -1
}

# Records Accessibility access for the app bundle $1, as System Settings
# does when the user switches it on.
grant() {
    printf '%s\n' "$(requirement_of "$1")" | csreq -r- -b "$work/requirement.bin"
    blob=$(xxd -p "$work/requirement.bin" | tr -d '\n')
    sudo sqlite3 "$database" "INSERT OR REPLACE INTO access
        (service, client, client_type, auth_value, auth_reason, auth_version, csreq, indirect_object_identifier, flags)
        VALUES ('kTCCServiceAccessibility', '$id', 0, 2, 4, 1, X'$blob', 'UNUSED', 0);"
}

cleanup() {
    tccutil reset Accessibility "$id" >/dev/null 2>&1 || true
    pkill -f "$work/app/Speachy.app/Contents/MacOS/" 2>/dev/null || true
}
trap cleanup EXIT

mkdir "$work/app" "$work/home"
installed="$work/app/Speachy.app"
cp -R "$old" "$installed"
tccutil reset Accessibility "$id" >/dev/null 2>&1 || true

state=$(ask "$installed" 1-before)
echo "before access is given, the installed app reports: ${state:-nothing}"
if [ "$state" != "not granted" ]; then
    echo "cannot tell: on this machine the app has access without being given it" >&2
    exit 75
fi

if ! grant "$installed"; then
    echo "cannot tell: the permission database cannot be written on this machine" >&2
    exit 75
fi
state=$(ask "$installed" 2-granted)
if [ "$state" != "granted" ]; then
    # The permission service may be holding on to its earlier answer.
    sudo killall tccd 2>/dev/null || true
    sleep 2
    state=$(ask "$installed" 2-granted-again)
fi
echo "after access is given, the installed app reports: ${state:-nothing}"
if [ "$state" != "granted" ]; then
    echo "cannot tell: giving access through the database has no effect on this machine" >&2
    exit 75
fi

rm -rf "$installed"
cp -R "$new" "$installed"
state=$(ask "$installed" 3-updated)
echo "after the update, the newer build reports: ${state:-nothing}"
if [ "$state" != "granted" ]; then
    echo "error: the update lost Accessibility access" >&2
    exit 1
fi

# The control: the same newer build signed ad hoc, as releases up to 0.1.2
# were. If this one had access too, the check above would mean nothing.
codesign --force --deep -s - "$installed" 2>/dev/null
state=$(ask "$installed" 4-ad-hoc)
echo "the same build signed ad hoc reports: ${state:-nothing}"
if [ "$state" != "not granted" ]; then
    echo "error: an ad hoc build has access too, so this test cannot tell the difference" >&2
    exit 1
fi
echo "ok: access given to the installed build carries over to the update, and only with the shared certificate"
