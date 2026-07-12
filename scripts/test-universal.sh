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

WORKFLOW="$ROOT/.github/workflows/ci.yml"
for required in \
    'apple-silicon-test:' \
    'name: Apple Silicon checks' \
    'runs-on: macos-14' \
    'test "$(uname -m)" = arm64' \
    'intel-test:' \
    'name: Intel checks' \
    'runs-on: macos-15-intel' \
    'test "$(uname -m)" = x86_64'
do
    grep -Fq -- "$required" "$WORKFLOW" \
        || fail "CI is missing native architecture witness: $required"
done

[ "$(grep -Fc 'cargo test --locked' "$WORKFLOW")" -ge 2 ] \
    || fail "both native CI jobs must run locked Rust tests"
[ "$(grep -Fc 'actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd' "$WORKFLOW")" -ge 2 ] \
    || fail "both native CI jobs must use the pinned checkout action"

grep -Fq 'Apple Silicon or a 64-bit Intel processor' "$ROOT/README.md" \
    || fail "README does not describe both supported Mac families"
grep -Fq 'Universal 2 (arm64 + x86_64)' "$ROOT/CHANGELOG.md" \
    || fail "changelog does not identify the universal v1 artifact"
grep -Fq 'rustup target add aarch64-apple-darwin x86_64-apple-darwin' \
    "$ROOT/CONTRIBUTING.md" \
    || fail "contributor setup does not install both Rust targets"
for instructions in "$ROOT/AGENTS.md" "$ROOT/CLAUDE.md"; do
    grep -Fq 'exactly arm64 + x86_64' "$instructions" \
        || fail "$(basename "$instructions") does not lock the universal ship contract"
done
if grep -Fq 'Intel Macs are not supported' "$ROOT/README.md" "$ROOT/CHANGELOG.md"; then
    fail "public copy still claims Intel Macs are unsupported"
fi

echo "universal binary checks passed"
