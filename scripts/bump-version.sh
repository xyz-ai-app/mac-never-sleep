#!/usr/bin/env bash
set -euo pipefail

# Single source of truth: [workspace.package] version in Cargo.toml.
# Usage:
#   ./scripts/bump-version.sh 0.3.4
#   ./scripts/bump-version.sh --print
#   ./scripts/bump-version.sh --check
#   ./scripts/bump-version.sh --stamp-plist path/to/Info.plist

ROOT="${NEVER_SLEEP_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
export NEVER_SLEEP_ROOT="$ROOT"
exec python3 - "$ROOT" "$@" <<'PY'
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1])
ARGS = sys.argv[2:]

SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
PAGES = ("site/index.html", "site/zh/index.html")
LOCK_PACKAGES = ("never-sleep", "never-sleep-core")


def die(message: str, code: int = 1) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(code)


def usage() -> None:
    die(
        "Usage: scripts/bump-version.sh <x.y.z> | --print | --check | --stamp-plist PATH",
        2,
    )


def workspace_version(text: str | None = None) -> str:
    if text is None:
        text = (ROOT / "Cargo.toml").read_text()
    try:
        section = text.split("[workspace.package]", 1)[1]
    except IndexError:
        die("Cargo.toml missing [workspace.package]")
    for line in section.splitlines():
        stripped = line.strip()
        if stripped.startswith("version = \""):
            return stripped[len("version = \"") :].rstrip('"')
    die("workspace.package version missing")
    raise AssertionError("unreachable")


def bundle_version(semver: str) -> str:
    major, minor, patch = (int(part) for part in semver.split("."))
    if minor > 99 or patch > 99:
        die("minor and patch must be 0-99 so CFBundleVersion stays monotonic")
    return str(major * 10_000 + minor * 100 + patch)


def set_workspace_version(text: str, new: str) -> str:
    at = text.find("[workspace.package]")
    if at < 0:
        die("Cargo.toml missing [workspace.package]")
    rest = text[at:]
    nxt = rest.find("\n[", 1)
    section = rest if nxt < 0 else rest[:nxt]
    after = "" if nxt < 0 else rest[nxt:]
    updated, n = re.subn(
        r'(?m)^version = "[^"]*"',
        f'version = "{new}"',
        section,
        count=1,
    )
    if n != 1:
        die("could not replace workspace.package version")
    return text[:at] + updated + after


def set_plist_versions(text: str, short: str, build: str) -> str:
    def replace_key(src: str, key: str, value: str) -> str:
        pattern = rf"(<key>{re.escape(key)}</key>\s*<string>)[^<]*(</string>)"
        updated, n = re.subn(pattern, rf"\g<1>{value}\g<2>", src, count=1)
        if n != 1:
            die(f"missing {key} in Info.plist")
        return updated

    text = replace_key(text, "CFBundleShortVersionString", short)
    return replace_key(text, "CFBundleVersion", build)


def set_page_versions(text: str, version: str) -> str:
    updated, n = re.subn(
        r'"softwareVersion":\s*"[^"]*"',
        f'"softwareVersion": "{version}"',
        text,
        count=1,
    )
    if n != 1:
        die("missing softwareVersion in page")
    updated, n = re.subn(
        r"(data-release-tag(?:\s[^>]*)?>)v?[0-9]+\.[0-9]+\.[0-9]+",
        rf"\g<1>v{version}",
        updated,
    )
    if n < 1:
        die("missing data-release-tag version fallback")
    return updated


def set_lock_versions(text: str, version: str) -> str:
    for name in LOCK_PACKAGES:
        updated, n = re.subn(
            rf'(name = "{re.escape(name)}"\nversion = ")[^"]*(")',
            rf"\g<1>{version}\g<2>",
            text,
            count=1,
        )
        if n != 1:
            die(f"missing {name} in Cargo.lock")
        text = updated
    return text


def plist_string(text: str, key: str) -> str:
    try:
        rest = text.split(f"<key>{key}</key>", 1)[1]
        return rest.split("<string>", 1)[1].split("</string>", 1)[0].strip()
    except IndexError:
        die(f"missing {key} in Info.plist")
        raise AssertionError("unreachable")


def apply(version: str) -> None:
    if not SEMVER.match(version):
        die(f"expected x.y.z, got {version!r}")
    build = bundle_version(version)
    cargo = ROOT / "Cargo.toml"
    cargo.write_text(set_workspace_version(cargo.read_text(), version))
    plist = ROOT / "packaging/Info.plist"
    plist.write_text(set_plist_versions(plist.read_text(), version, build))
    for rel in PAGES:
        path = ROOT / rel
        path.write_text(set_page_versions(path.read_text(), version))
    lock = ROOT / "Cargo.lock"
    if lock.exists():
        lock.write_text(set_lock_versions(lock.read_text(), version))


def check() -> None:
    version = workspace_version()
    build = bundle_version(version)
    errors: list[str] = []
    plist = (ROOT / "packaging/Info.plist").read_text()
    got_short = plist_string(plist, "CFBundleShortVersionString")
    if got_short != version:
        errors.append(
            f"packaging/Info.plist CFBundleShortVersionString is {got_short!r}, want {version!r}"
        )
    got_build = plist_string(plist, "CFBundleVersion")
    if got_build != build:
        errors.append(
            f"packaging/Info.plist CFBundleVersion is {got_build!r}, want {build!r} (from {version})"
        )
    for rel in PAGES:
        html = (ROOT / rel).read_text()
        if f'"softwareVersion": "{version}"' not in html:
            errors.append(f"{rel} softwareVersion is stale")
        if f"v{version}" not in html:
            errors.append(f"{rel} data-release-tag fallback is stale")
    lock = ROOT / "Cargo.lock"
    if lock.exists():
        text = lock.read_text()
        for name in LOCK_PACKAGES:
            match = re.search(
                rf'name = "{re.escape(name)}"\nversion = "([^"]*)"',
                text,
            )
            if not match or match.group(1) != version:
                got = match.group(1) if match else "missing"
                errors.append(f"Cargo.lock {name} version is {got!r}, want {version!r}")
    if errors:
        die("stale derived version files:\n" + "\n".join(errors))


def stamp_plist(path: str) -> None:
    version = workspace_version()
    build = bundle_version(version)
    dest = Path(path)
    dest.write_text(set_plist_versions(dest.read_text(), version, build))


if not ARGS:
    usage()

cmd = ARGS[0]
if cmd == "--print":
    print(workspace_version())
elif cmd == "--check":
    check()
elif cmd == "--stamp-plist":
    if len(ARGS) != 2:
        usage()
    stamp_plist(ARGS[1])
elif cmd.startswith("-"):
    usage()
else:
    if len(ARGS) != 1:
        usage()
    apply(cmd)
PY
