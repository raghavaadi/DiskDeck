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

mkdir -p "$TMP/bad"
cp -R "$TMP/valid/DiskDeck" "$TMP/bad/"
: > "$TMP/bad/DiskDeck/DiskDeck.app/Contents/MacOS/._diskdeck"
(cd "$TMP/bad" && zip -qry "$TMP/bad.zip" DiskDeck)

if "$ROOT/scripts/check-dist.sh" "$TMP/bad.zip" >/dev/null 2>&1; then
    echo "FAIL: AppleDouble metadata was accepted" >&2
    exit 1
fi

echo "package artifact checks passed"
