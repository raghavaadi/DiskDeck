#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
HOOK="$ROOT/.githooks/pre-commit"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -x "$HOOK" ] || fail "$HOOK is missing or not executable"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-hook-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

new_repo() {
    repo="$TMP/$1"
    mkdir -p "$repo/.githooks"
    cp "$HOOK" "$repo/.githooks/pre-commit"
    chmod +x "$repo/.githooks/pre-commit"
    git -C "$repo" init -q
    git -C "$repo" config user.name "Hook Test"
    git -C "$repo" config user.email "hook-test@example.com"
    git -C "$repo" config core.hooksPath .githooks
}

expect_pass() {
    repo="$1"
    if ! git -C "$repo" commit -q -m test; then
        fail "expected commit to pass in $repo"
    fi
}

expect_block() {
    repo="$1"
    if git -C "$repo" commit -q -m test >/dev/null 2>&1; then
        fail "expected commit to be blocked in $repo"
    fi
}

new_repo clean
printf '%s\n' '# clean fixture' > "$repo/README.md"
git -C "$repo" add README.md
expect_pass "$repo"

new_repo system_data_root
printf '%s\n' 'scan_root=/System/Volumes/Data, not /' > "$repo/config.txt"
git -C "$repo" add config.txt
expect_pass "$repo"

new_repo credential
printf '%s%s\n' 'github_' 'pat_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA' > "$repo/config.txt"
git -C "$repo" add config.txt
expect_block "$repo"

new_repo private_path
printf '%s%s\n' '/Users/' 'realperson/private/file' > "$repo/notes.txt"
git -C "$repo" add notes.txt
expect_block "$repo"

new_repo dotenv
printf '%s\n' 'SAFE_FIXTURE=yes' > "$repo/.env"
git -C "$repo" add .env
expect_block "$repo"

new_repo appledouble
printf '%s\n' 'resource fork' > "$repo/._README.md"
git -C "$repo" add ._README.md
expect_block "$repo"

new_repo build_output
mkdir -p "$repo/target/debug"
printf '%s\n' 'binary' > "$repo/target/debug/diskdeck"
git -C "$repo" add -f target/debug/diskdeck
expect_block "$repo"

echo "pre-commit guard tests passed"
