#!/bin/sh
set -eu

EXECUTABLE=${1:?usage: check-universal-binary.sh path/to/executable}

fail() {
    echo "FAIL: expected exactly arm64 and x86_64: $*" >&2
    exit 1
}

[ -f "$EXECUTABLE" ] || fail "executable not found: $EXECUTABLE"

if ! ARCHS=$(lipo -archs "$EXECUTABLE" 2>/dev/null); then
    fail "cannot inspect Mach-O executable"
fi

ARCH_COUNT=$(printf '%s\n' "$ARCHS" | awk '{ print NF }')
[ "$ARCH_COUNT" = '2' ] \
    || fail "found ${ARCHS:-no architecture slices}"

lipo "$EXECUTABLE" -verify_arch arm64 x86_64 >/dev/null 2>&1 \
    || fail "found $ARCHS"

echo "universal executable checks passed: $ARCHS"
