#!/bin/sh
# The certificate Speachy's macOS builds are signed with.
#
# macOS remembers a permission such as Accessibility for an app's signature.
# A build signed "ad hoc" has a signature of its own, so every update looks
# like a different app and has to be allowed again. Builds signed with one
# certificate share a signature macOS recognizes, whichever build it is. The
# certificate is self-signed: it costs nothing and is not known to Apple, so
# it does not change what Gatekeeper says about a downloaded app.
#
#   scripts/macos-signing.sh generate FILE   write a new key and certificate (PEM)
#   scripts/macos-signing.sh import FILE     make it available to codesign (macOS);
#                                            prints the identity to sign with
#   scripts/macos-signing.sh forget          remove what `import` added
#
# FILE holds the private key: keep it out of the repository. Releases read
# it from the MACOS_SIGNING_IDENTITY secret.
set -eu

keychain="$HOME/Library/Keychains/speachy-signing.keychain-db"

# The keychains codesign searches, one per line, without the one above.
other_keychains() {
    security list-keychains -d user | sed 's/^[[:space:]]*"//; s/"[[:space:]]*$//' | grep -vxF "$keychain" || true
}

# Sets the keychains codesign searches: the ones already there, then $1.
set_search_list() {
    add=$1
    set --
    while IFS= read -r path; do
        [ -n "$path" ] && set -- "$@" "$path"
    done <<EOF
$(other_keychains)
EOF
    [ -n "$add" ] && set -- "$@" "$add"
    security list-keychains -d user -s "$@"
}

case "${1:-}" in
generate)
    out=${2:?usage: macos-signing.sh generate FILE}
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    cat >"$work/openssl.cnf" <<'EOF'
[ req ]
distinguished_name = name
x509_extensions = extensions
prompt = no

[ name ]
CN = Speachy Code Signing

[ extensions ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
    # Valid for 30 years: macOS will not sign with an expired certificate,
    # and a new one would be a new signature.
    openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 10950 \
        -config "$work/openssl.cnf" -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
    # The key first, unencrypted, in the form `security import` reads.
    umask 077
    openssl pkcs8 -topk8 -nocrypt -in "$work/key.pem" >"$out"
    cat "$work/cert.pem" >>"$out"
    echo "wrote $out" >&2
    ;;

import)
    file=${2:?usage: macos-signing.sh import FILE}
    password=$(openssl rand -hex 16)
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    security create-keychain -p "$password" "$keychain"
    security set-keychain-settings -t 21600 -u "$keychain"
    security unlock-keychain -p "$password" "$keychain"
    security import "$file" -f pemseq -t agg -P "" -T /usr/bin/codesign -k "$keychain" >&2
    # Lets codesign use the key without a dialog asking for permission.
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
    set_search_list "$keychain"
    # codesign accepts the certificate's SHA-1 fingerprint as the identity.
    openssl x509 -in "$file" -noout -fingerprint -sha1 | sed 's/^.*=//; s/://g'
    ;;

forget)
    set_search_list ""
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    ;;

*)
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
