#!/usr/bin/env bash
# Check that a Sparkle EdDSA private key belongs to the public key the app
# checks updates against (SUPublicEDKey in Info.plist, ADR 0012). A key from
# another pair signs updates every installed app rejects, with no error at
# release time (#284).
#
# Usage: check-sparkle-key.sh <private-key-file> [Info.plist]
#
# The key file holds base64, as `generate_keys -x` writes it: a 32-byte
# seed, or Sparkle's older 96-byte form, whose last 32 bytes are the public
# key. A seed needs OpenSSL 3 to derive its public key.
set -euo pipefail

key_file="$1"
plist="${2:-$(cd "$(dirname "$0")/../.." && pwd)/apps/macos/resources/Info.plist}"

# PlistBuddy on macOS; python3 on the Linux runner, which has no PlistBuddy.
if [[ -x /usr/libexec/PlistBuddy ]]; then
  want="$(/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" "$plist")"
else
  want="$(python3 -c 'import plistlib, sys; print(plistlib.load(open(sys.argv[1], "rb"))["SUPublicEDKey"])' "$plist")"
fi
if [[ -z "$want" ]]; then
  echo "error: $plist has no SUPublicEDKey" >&2
  exit 1
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tr -d ' \n\r\t' <"$key_file" | base64 --decode >"$work/key" 2>/dev/null || {
  echo "error: the Sparkle key is not base64" >&2
  exit 1
}

# macOS's own openssl is LibreSSL, which can't read a raw Ed25519 seed:
# use Homebrew's openssl@3, or $OPENSSL.
openssl3() {
  local candidate
  for candidate in "${OPENSSL:-}" /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl openssl; do
    if [[ -n "$candidate" ]] && "$candidate" version 2>/dev/null | grep -q '^OpenSSL 3'; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

case "$(wc -c <"$work/key" | tr -d ' ')" in
  32)
    # PKCS#8 wrapping of a raw Ed25519 seed (RFC 8410).
    { printf '\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x70\x04\x22\x04\x20'; cat "$work/key"; } >"$work/key.der"
    if ! openssl="$(openssl3)"; then
      echo "error: deriving the Sparkle key's public key needs OpenSSL 3, and none was found at \$OPENSSL, /opt/homebrew/opt/openssl@3 or on PATH: brew install openssl@3" >&2
      exit 1
    fi
    "$openssl" pkey -inform DER -in "$work/key.der" -pubout -outform DER -out "$work/pub.der"
    got="$(tail -c 32 "$work/pub.der" | base64)"
    ;;
  96)
    got="$(tail -c 32 "$work/key" | base64)"
    ;;
  *)
    echo "error: the Sparkle key is $(wc -c <"$work/key" | tr -d ' ') bytes, not a 32-byte seed or Sparkle's 96-byte key" >&2
    exit 1
    ;;
esac

if [[ "$got" != "$want" ]]; then
  echo "error: the Sparkle private key's public key is $got, but Info.plist's SUPublicEDKey is $want: installed apps would reject every update it signs" >&2
  exit 1
fi
echo "The Sparkle key matches SUPublicEDKey."
