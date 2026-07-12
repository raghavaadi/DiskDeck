#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-package-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

mkdir -p "$TMP/valid/DiskDeck/DiskDeck.app/Contents/MacOS"
: > "$TMP/valid/DiskDeck/DiskDeck.app/Contents/MacOS/diskdeck"
: > "$TMP/valid/DiskDeck/Install DiskDeck.command"
(cd "$TMP/valid" && zip -qry "$TMP/valid.zip" DiskDeck)
"$ROOT/scripts/check-dist.sh" "$TMP/valid.zip" >/dev/null

if "$ROOT/scripts/check-release-artifact.sh" \
    "$TMP/valid.zip" 1.0.0 > "$TMP/release-check.out" 2>&1; then
    echo "FAIL: unsigned fixture was accepted as a public release" >&2
    exit 1
fi
grep -Fq 'Developer ID Application' "$TMP/release-check.out" || {
    echo "FAIL: unsigned release rejection did not name the required identity" >&2
    cat "$TMP/release-check.out" >&2
    exit 1
}

mkdir -p "$TMP/bad"
cp -R "$TMP/valid/DiskDeck" "$TMP/bad/"
: > "$TMP/bad/DiskDeck/DiskDeck.app/Contents/MacOS/._diskdeck"
(cd "$TMP/bad" && zip -qry "$TMP/bad.zip" DiskDeck)

if "$ROOT/scripts/check-dist.sh" "$TMP/bad.zip" >/dev/null 2>&1; then
    echo "FAIL: AppleDouble metadata was accepted" >&2
    exit 1
fi

echo "package artifact checks passed"
