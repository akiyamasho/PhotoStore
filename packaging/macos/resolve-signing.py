#!/usr/bin/env python3
"""Resolve and validate the certificate-backed identity for production packaging."""

from __future__ import annotations

import json
import base64
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import NoReturn


CONFIG_PATH = Path.home() / "Library/Application Support/PhotoStore/signing/release-signing.json"
IDENTITY_LINE = re.compile(r'^\s*\d+\)\s+([0-9A-Fa-f]{40}) "(.*)"\s*$')


def fail(message: str) -> NoReturn:
    print(f"error: macOS production signing: {message}", file=sys.stderr)
    raise SystemExit(2)


def run_security(*args: str) -> str:
    try:
        result = subprocess.run(["/usr/bin/security", *args], check=False, capture_output=True, text=True)
    except OSError as exc:
        fail(f"could not run the macOS security tool ({exc})")
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        fail(f"security {' '.join(args[:3])} failed{': ' + detail if detail else ''}")
    return result.stdout


def identities(keychain: str | None) -> list[tuple[str, str]]:
    args = ["find-identity", "-v", "-p", "codesigning"]
    if keychain:
        args.append(keychain)
    output = run_security(*args)
    return [(match.group(1).upper(), match.group(2)) for line in output.splitlines() if (match := IDENTITY_LINE.match(line))]


def configured_identity() -> tuple[str, str, str]:
    if not CONFIG_PATH.is_file():
        fail(
            f"no persistent signing identity is configured at {CONFIG_PATH}. "
            "Create the local PhotoStore identity or set MACOS_SIGN_IDENTITY to a valid certificate-backed identity."
        )
    try:
        config = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        fail(f"cannot read {CONFIG_PATH}: {exc}")
    if not isinstance(config, dict) or config.get("schemaVersion") != 1 or config.get("kind") != "local":
        fail(f"{CONFIG_PATH} must use schemaVersion 1 and kind 'local'")
    identity = config.get("identitySha1")
    cert_sha256 = config.get("certificateSha256")
    keychain_value = config.get("keychainPath")
    if not isinstance(identity, str) or not re.fullmatch(r"[0-9A-Fa-f]{40}", identity):
        fail(f"{CONFIG_PATH} has an invalid identitySha1")
    if not isinstance(cert_sha256, str) or not re.fullmatch(r"[0-9A-Fa-f]{64}", cert_sha256):
        fail(f"{CONFIG_PATH} has an invalid certificateSha256")
    if not isinstance(keychain_value, str) or not keychain_value.strip():
        fail(f"{CONFIG_PATH} has an empty keychainPath")
    keychain = str(Path(keychain_value).expanduser())
    if not Path(keychain).is_file():
        fail(f"configured keychain does not exist: {keychain}")

    if not any(sha1 == identity.upper() for sha1, _ in identities(keychain)):
        fail("the configured certificate does not have a valid code-signing identity in the configured keychain")
    certs = run_security("find-certificate", "-a", "-p", keychain)
    pem_blocks = re.findall(r"-----BEGIN CERTIFICATE-----\s+.+?-----END CERTIFICATE-----", certs, re.S)
    wanted = (identity.upper(), cert_sha256.upper())
    found_pair = False
    for pem in pem_blocks:
        try:
            der = base64.b64decode("".join(pem.splitlines()[1:-1]), validate=True)
        except ValueError:
            continue
        if (hashlib.sha1(der).hexdigest().upper(), hashlib.sha256(der).hexdigest().upper()) == wanted:
            found_pair = True
            break
    if not found_pair:
        fail("the configured certificate fingerprints are not present together in the configured keychain")
    return identity.upper(), keychain, "local"


def resolve() -> tuple[str, str, str]:
    requested = os.environ.get("MACOS_SIGN_IDENTITY")
    requested_keychain = os.environ.get("MACOS_KEYCHAIN")
    if requested is None or not requested.strip():
        if requested_keychain:
            fail("MACOS_KEYCHAIN was set without MACOS_SIGN_IDENTITY")
        return configured_identity()
    requested = requested.strip()
    if requested == "-" or requested.lower().startswith("cdhash"):
        fail("ad-hoc and cdhash-only signing are not allowed for stable production packages; use package-dev.sh for ad-hoc builds")
    keychain = str(Path(requested_keychain).expanduser()) if requested_keychain else ""
    if keychain and not Path(keychain).is_file():
        fail(f"MACOS_KEYCHAIN does not exist: {keychain}")
    matches = identities(keychain or None)
    requested_upper = requested.upper()
    selected = next((entry for entry in matches if entry[0] == requested_upper), None)
    if selected is None:
        selected = next((entry for entry in matches if entry[1] == requested), None)
    if selected is None:
        fail("MACOS_SIGN_IDENTITY is not a valid certificate-backed code-signing identity in the selected keychain")
    identity, label = selected
    kind = "developer_id" if "Developer ID Application" in label else "certificate"
    return identity, keychain, kind


if __name__ == "__main__":
    identity, keychain, kind = resolve()
    sys.stdout.buffer.write(identity.encode() + b"\0" + keychain.encode() + b"\0" + kind.encode() + b"\0")
