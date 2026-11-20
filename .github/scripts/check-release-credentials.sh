#!/usr/bin/env bash
# Checks the release secrets in a minute, before anything slow: every one
# is set, and Apple accepts the App Store Connect key. Apple refuses the
# key when an agreement in App Store Connect expires, and tells only the
# Account Holder, by email.
#
# On macOS it asks notarytool, as release.sh does. Elsewhere, for the
# weekly check on Linux, it asks Apple's Notary API with a token signed by
# the key (notary-api-check.py) and also checks the Developer ID
# certificate's password and expiry and the two deploy keys.
#
# Env: the secrets named below, as release.yml passes them.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
missing=()
for name in DEVELOPER_ID_P12_BASE64 DEVELOPER_ID_P12_PASSWORD APP_STORE_CONNECT_KEY_P8 \
  APP_STORE_CONNECT_KEY_ID APP_STORE_CONNECT_ISSUER_ID SPARKLE_ED_PRIVATE_KEY TAP_DEPLOY_KEY \
  ${EXTRA_SECRETS:-}; do
  [[ -n "${!name:-}" ]] || missing+=("$name")
done
if ((${#missing[@]})); then
  echo "::error::These release secrets are empty or missing: ${missing[*]}"
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf '%s\n' "$APP_STORE_CONNECT_KEY_P8" >"$work/notary.p8"

if [[ "$(uname)" == Darwin ]]; then
  if ! out="$(xcrun notarytool history --key "$work/notary.p8" \
    --key-id "$APP_STORE_CONNECT_KEY_ID" --issuer "$APP_STORE_CONNECT_ISSUER_ID" 2>&1 >/dev/null)"; then
    # notarytool's message says why: a revoked key, an expired membership,
    # or an agreement waiting in App Store Connect.
    echo "::error::notarytool refused the App Store Connect key: ${out//$'\n'/ }"
    exit 1
  fi
  echo "notarytool accepts the App Store Connect key."
  exit 0
fi

python3 -I "$here/notary-api-check.py" "$work/notary.p8"

# The certificate: the password opens it, and it has a month left.
printf '%s' "$DEVELOPER_ID_P12_BASE64" | base64 --decode >"$work/id.p12"
cert() {
  openssl pkcs12 -in "$work/id.p12" -passin env:DEVELOPER_ID_P12_PASSWORD -nokeys -clcerts "$@" 2>/dev/null
}
# Keychain Access exports with ciphers OpenSSL 3 keeps in its legacy provider.
if ! cert >"$work/id.pem" && ! cert -legacy >"$work/id.pem"; then
  echo "::error::DEVELOPER_ID_P12_PASSWORD does not open DEVELOPER_ID_P12_BASE64"
  exit 1
fi
subject="$(openssl x509 -in "$work/id.pem" -noout -subject)"
end="$(openssl x509 -in "$work/id.pem" -noout -enddate | cut -d= -f2)"
if [[ "$subject" != *"Developer ID Application"* ]]; then
  echo "::error::The certificate is not a Developer ID Application one: $subject"
  exit 1
fi
if ! openssl x509 -in "$work/id.pem" -noout -checkend 0 >/dev/null; then
  echo "::error::The Developer ID certificate expired on $end"
  exit 1
fi
if ! openssl x509 -in "$work/id.pem" -noout -checkend $((30 * 86400)) >/dev/null; then
  echo "::warning::The Developer ID certificate expires on $end"
fi
echo "The Developer ID certificate opens and is valid until $end."

if ! printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | base64 --decode >/dev/null 2>&1; then
  echo "::error::SPARKLE_ED_PRIVATE_KEY is not base64"
  exit 1
fi

# The deploy keys still reach their repositories. Reading is all a check
# can do without pushing.
reach() {
  local secret="$1" repo="$2"
  printf '%s\n' "${!secret}" >"$work/$secret"
  chmod 600 "$work/$secret"
  if ! GIT_SSH_COMMAND="ssh -i $work/$secret -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new" \
    git ls-remote -q "git@github.com:$repo.git" HEAD >/dev/null; then
    echo "::error::$secret does not reach $repo"
    exit 1
  fi
  echo "$secret reaches $repo."
}
reach TAP_DEPLOY_KEY Fanzzzd/homebrew-agent-screen
if [[ -n "${RELEASE_DEPLOY_KEY:-}" ]]; then
  reach RELEASE_DEPLOY_KEY "${GITHUB_REPOSITORY:-Fanzzzd/agent-screen}"
fi
