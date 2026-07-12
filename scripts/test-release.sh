#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
LIB="$ROOT/scripts/release-lib.sh"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_ok() {
    "$@" || fail "expected success: $*"
}

assert_fail() {
    if "$@"; then
        fail "expected failure: $*"
    fi
}

[ -f "$LIB" ] || fail "missing scripts/release-lib.sh"
# shellcheck disable=SC1090
. "$LIB"

assert_ok diskdeck_validate_tag v1.0.0
assert_ok diskdeck_validate_tag v0.2.3
assert_fail diskdeck_validate_tag 1.0.0
assert_fail diskdeck_validate_tag v1.0
assert_fail diskdeck_validate_tag v01.0.0
assert_fail diskdeck_validate_tag v1.0.0-beta
assert_fail diskdeck_validate_tag v1.0.0.1

[ "$(diskdeck_tag_version v1.2.3)" = "1.2.3" ] \
    || fail "tag version did not remove exactly one leading v"
[ "$(diskdeck_package_version "$ROOT")" = "1.0.0" ] \
    || fail "Cargo package version was not parsed exactly"

assert_ok diskdeck_is_distribution_identity \
    'Developer ID Application: Example Person (TEAM123456)'
assert_fail diskdeck_is_distribution_identity \
    'Apple Development: Example Person (TEAM123456)'
assert_fail diskdeck_is_distribution_identity '-'
assert_fail diskdeck_is_distribution_identity ''

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-release-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

printf '%s\n' \
    '# Changelog' \
    '' \
    '## [1.1.0] - 2026-08-01' \
    '' \
    '- Later.' \
    '' \
    '## [1.0.0] - 2026-07-12' \
    '' \
    '- First public build.' \
    '- Safe by default.' \
    '' \
    '## [0.9.0] - 2026-07-01' \
    '' \
    '- Preview.' > "$TMP/CHANGELOG.md"

diskdeck_extract_release_notes \
    "$TMP/CHANGELOG.md" v1.0.0 "$TMP/notes.md"
grep -Fxq -- '- First public build.' "$TMP/notes.md" \
    || fail "release notes omitted the requested section"
grep -Fxq -- '- Safe by default.' "$TMP/notes.md" \
    || fail "release notes omitted a requested-section line"
if grep -Fq -- '- Later.' "$TMP/notes.md" || \
   grep -Fq -- '- Preview.' "$TMP/notes.md"; then
    fail "release notes crossed a version boundary"
fi
assert_fail diskdeck_extract_release_notes \
    "$TMP/CHANGELOG.md" v2.0.0 "$TMP/missing.md"

echo "release policy checks passed"
