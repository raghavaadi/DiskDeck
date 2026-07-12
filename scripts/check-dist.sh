#!/bin/sh
set -eu

ARCHIVE=${1:?usage: check-dist.sh path/to/DiskDeck.zip}

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -f "$ARCHIVE" ] || fail "archive not found: $ARCHIVE"

ENTRIES=$(mktemp "${TMPDIR:-/tmp}/diskdeck-zip-entries.XXXXXX")
trap 'rm -f "$ENTRIES"' EXIT HUP INT TERM

unzip -Z1 "$ARCHIVE" > "$ENTRIES" || fail "cannot read archive: $ARCHIVE"

if LC_ALL=C grep -Eq '(^|/)(\._[^/]*|__MACOSX)(/|$)' "$ENTRIES"; then
    fail "archive contains macOS AppleDouble metadata"
fi

if LC_ALL=C grep -Eq '(^/|(^|/)\.\.(/|$))' "$ENTRIES"; then
    fail "archive contains an unsafe path"
fi

grep -Fxq 'DiskDeck/DiskDeck.app/Contents/MacOS/diskdeck' "$ENTRIES" \
    || fail "archive is missing the DiskDeck executable"
grep -Fxq 'DiskDeck/Install DiskDeck.command' "$ENTRIES" \
    || fail "archive is missing the recipient installer"

echo "distribution archive checks passed"
