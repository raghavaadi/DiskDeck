#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
HOOK="$ROOT/.githooks/pre-push"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -x "$HOOK" ] || fail "$HOOK is missing or not executable"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-pre-push-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

repo="$TMP/repo"
git init -q "$repo"
git -C "$repo" config user.name "Raghav"
git -C "$repo" config user.email "aadithyaraghav@gmail.com"

printf '%s\n' '# clean root' > "$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "Clean root"
clean_sha=$(git -C "$repo" rev-parse HEAD)
zero=0000000000000000000000000000000000000000

run_hook() {
    input="$1"
    remote_url=${2:-}
    if [ -z "$remote_url" ]; then
        remote_url=git@"github.com":raghavaadi/DiskDeck.git
    fi
    printf '%s\n' "$input" | (cd "$repo" && "$HOOK" origin "$remote_url")
}

run_hook "refs/heads/main $clean_sha refs/heads/main $zero" \
    || fail "clean personal-history push should pass"

printf '%s\n' 'private work note' > "$repo/work.txt"
git -C "$repo" add work.txt
git -C "$repo" -c user.name=raghav -c user.email=raghav@"buddyhq."ai \
    commit -q -m "Work identity commit"
work_sha=$(git -C "$repo" rev-parse HEAD)

if run_hook "refs/heads/archive $work_sha refs/heads/archive $zero" >/dev/null 2>&1; then
    fail "work-email history should be blocked from the personal GitHub repository"
fi

run_hook "refs/heads/archive $work_sha refs/heads/archive $zero" \
    "git@bitbucket.org:buddyhq/headroom-rs.git" \
    || fail "unrelated remotes should not be governed by the personal GitHub guard"

run_hook "(delete) $zero refs/heads/archive $work_sha" \
    || fail "deleting a remote ref should pass"

echo "pre-push identity guard tests passed"
