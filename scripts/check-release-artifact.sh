#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
ARCHIVE=${1:?usage: check-release-artifact.sh path/to/DiskDeck.zip VERSION}
EXPECTED_VERSION=${2:?usage: check-release-artifact.sh path/to/DiskDeck.zip VERSION}

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

printf '%s\n' "$EXPECTED_VERSION" | LC_ALL=C grep -Eq \
    '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
    || fail "expected version is not canonical SemVer: $EXPECTED_VERSION"

"$ROOT/scripts/check-dist.sh" "$ARCHIVE" >/dev/null

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-release-check.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
COPYFILE_DISABLE=1 ditto -x -k "$ARCHIVE" "$TMP"

APP="$TMP/DiskDeck/DiskDeck.app"
PLIST="$APP/Contents/Info.plist"
DETAILS="$TMP/codesign-details.txt"

[ -d "$APP" ] || fail "archive is missing DiskDeck.app"

if ! codesign --verify --deep --strict "$APP" >/dev/null 2>&1; then
    fail "release app is not validly signed with Developer ID Application"
fi
codesign -dv --verbose=4 "$APP" 2> "$DETAILS" \
    || fail "cannot inspect release code signature"

grep -Fq 'Authority=Developer ID Application:' "$DETAILS" \
    || fail "release app is not signed with Developer ID Application"
grep -Fq '(runtime)' "$DETAILS" \
    || fail "release app is missing hardened runtime"
grep -Fq 'Timestamp=' "$DETAILS" \
    || fail "release app is missing a secure signing timestamp"
grep -Fq 'TeamIdentifier=65KMSM8WL8' "$DETAILS" \
    || fail "release app is not signed by DiskDeck team 65KMSM8WL8"

"$ROOT/scripts/check-universal-binary.sh" \
    "$APP/Contents/MacOS/diskdeck" >/dev/null

BUNDLE_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST" 2>/dev/null) \
    || fail "cannot read release bundle identifier"
[ "$BUNDLE_ID" = 'com.buddyhq.headroom-rs' ] \
    || fail "unexpected release bundle identifier: $BUNDLE_ID"

BUNDLE_VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST" 2>/dev/null) \
    || fail "cannot read release bundle version"
[ "$BUNDLE_VERSION" = "$EXPECTED_VERSION" ] \
    || fail "release version $BUNDLE_VERSION does not match $EXPECTED_VERSION"

xcrun stapler validate "$APP" >/dev/null 2>&1 \
    || fail "release app has no valid stapled notarization ticket"
spctl --assess --type execute --verbose=4 "$APP" >/dev/null 2>&1 \
    || fail "Gatekeeper rejected the release app"

echo "public release artifact checks passed"
