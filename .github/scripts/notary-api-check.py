"""Asks Apple's Notary API for the team's past submissions, as
`notarytool history` does, to learn whether Apple accepts the App Store
Connect key. Runs where notarytool doesn't: the weekly check on Linux.

Usage: notary-api-check.py <key.p8>
Env: APP_STORE_CONNECT_KEY_ID, APP_STORE_CONNECT_ISSUER_ID.
Standard library and the openssl command only.
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

URL = "https://appstoreconnect.apple.com/notary/v2/submissions"


def b64url(data: bytes) -> bytes:
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def raw_signature(der: bytes) -> bytes:
    """An ECDSA signature from DER (SEQUENCE of r and s) to JWS's r || s."""
    if der[0] != 0x30:
        raise ValueError("not a DER sequence")
    i = 2 if der[1] < 0x80 else 2 + (der[1] & 0x7F)
    out = b""
    for _ in range(2):
        if der[i] != 0x02:
            raise ValueError("not a DER integer")
        n = der[i + 1]
        out += der[i + 2 : i + 2 + n].lstrip(b"\0").rjust(32, b"\0")
        i += 2 + n
    return out


def token(key_path: str, key_id: str, issuer: str) -> str:
    now = int(time.time())
    header = b64url(json.dumps({"alg": "ES256", "kid": key_id, "typ": "JWT"}).encode())
    claims = {"iss": issuer, "iat": now, "exp": now + 600, "aud": "appstoreconnect-v1"}
    signing = header + b"." + b64url(json.dumps(claims).encode())
    der = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", key_path],
        input=signing,
        capture_output=True,
        check=True,
    ).stdout
    return (signing + b"." + b64url(raw_signature(der))).decode()


def main() -> int:
    key_path = sys.argv[1]
    bearer = token(
        key_path,
        os.environ["APP_STORE_CONNECT_KEY_ID"],
        os.environ["APP_STORE_CONNECT_ISSUER_ID"],
    )
    request = urllib.request.Request(URL, headers={"Authorization": f"Bearer {bearer}"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            print(f"The Notary API accepts the App Store Connect key ({response.status}).")
            return 0
    except urllib.error.HTTPError as error:
        # 401: Apple doesn't know the key. 403: it does, but an agreement
        # in App Store Connect waits for the Account Holder.
        body = error.read().decode(errors="replace")[:500].replace("\n", " ")
        print(f"::error::The Notary API refused the App Store Connect key ({error.code}): {body}")
        return 1
    except urllib.error.URLError as error:
        print(f"::error::The Notary API did not answer: {error.reason}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
