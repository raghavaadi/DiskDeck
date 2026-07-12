#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
CHECKER="$ROOT/scripts/check-universal-binary.sh"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -f "$CHECKER" ] || fail "missing scripts/check-universal-binary.sh"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-universal-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

printf '%s\n' 'int main(void) { return 0; }' | \
    xcrun clang -x c -arch arm64 -mmacosx-version-min=12.0 \
        - -o "$TMP/arm64"
printf '%s\n' 'int main(void) { return 0; }' | \
    xcrun clang -x c -arch x86_64 -mmacosx-version-min=12.0 \
        - -o "$TMP/x86_64"
lipo -create "$TMP/arm64" "$TMP/x86_64" -output "$TMP/universal"

"$CHECKER" "$TMP/universal" >/dev/null \
    || fail "two-slice fixture was rejected"

for invalid in "$TMP/arm64" "$TMP/x86_64" "$TMP/missing"; do
    if "$CHECKER" "$invalid" > "$TMP/rejection.out" 2>&1; then
        fail "non-universal fixture was accepted: $invalid"
    fi
    grep -Fq 'exactly arm64 and x86_64' "$TMP/rejection.out" \
        || fail "universal rejection was not actionable: $invalid"
done

BUNDLER="$ROOT/make-app.sh"
RELEASE_CHECKER="$ROOT/scripts/check-release-artifact.sh"
for required in \
    'aarch64-apple-darwin' \
    'x86_64-apple-darwin' \
    'MACOSX_DEPLOYMENT_TARGET=12.0' \
    'cargo build --release --locked --target "$TARGET"' \
    'lipo -create' \
    'scripts/check-universal-binary.sh'
do
    grep -Fq -- "$required" "$BUNDLER" \
        || fail "make-app.sh is missing universal contract: $required"
done

grep -Fq 'scripts/check-universal-binary.sh' "$RELEASE_CHECKER" \
    || fail "public release checker does not verify universal slices"

UNIVERSAL_CHECK_LINE=$(grep -n -m 1 'scripts/check-universal-binary.sh' "$BUNDLER" | cut -d: -f1)
FIRST_SIGN_LINE=$(grep -n -m 1 'codesign --force' "$BUNDLER" | cut -d: -f1)
[ "$UNIVERSAL_CHECK_LINE" -lt "$FIRST_SIGN_LINE" ] \
    || fail "universal slices must be verified before code signing"

echo "universal binary checks passed"
