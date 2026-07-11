#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

required_files='.github/workflows/ci.yml
.github/ISSUE_TEMPLATE/bug_report.yml
.github/ISSUE_TEMPLATE/feature_request.yml
.github/ISSUE_TEMPLATE/config.yml
.github/pull_request_template.md
CONTRIBUTING.md
SECURITY.md'

printf '%s\n' "$required_files" | while IFS= read -r path; do
    [ -n "$path" ] || continue
    [ -f "$ROOT/$path" ] || fail "missing $path"
done

ruby -e 'require "yaml"; ARGV.each { |path| YAML.load_file(path) }' \
    "$ROOT/.github/workflows/ci.yml" \
    "$ROOT/.github/ISSUE_TEMPLATE/bug_report.yml" \
    "$ROOT/.github/ISSUE_TEMPLATE/feature_request.yml" \
    "$ROOT/.github/ISSUE_TEMPLATE/config.yml"

workflow="$ROOT/.github/workflows/ci.yml"
for expected in \
    'permissions:' \
    'contents: read' \
    'workflow_dispatch:' \
    'runs-on: macos-14' \
    'cargo fmt -- --check' \
    'scripts/test-ui-smoke.sh' \
    'scripts/test-pre-commit.sh' \
    'scripts/test-pre-push.sh' \
    'cargo test --locked'
do
    grep -Fq "$expected" "$workflow" || fail "CI is missing: $expected"
done

if grep -Eiq 'make-app\.sh|upload-artifact|codesign|SIGN_IDENTITY' "$workflow"; then
    fail "CI must not build, sign, or upload application bundles"
fi

grep -Fq 'https://github.com/raghavaadi/DiskDeck/security/advisories/new' "$ROOT/SECURITY.md" \
    || fail "SECURITY.md must use private vulnerability reporting"
grep -Fq 'Full Disk Access' "$ROOT/.github/ISSUE_TEMPLATE/bug_report.yml" \
    || fail "bug reports must capture Full Disk Access state"
grep -Fq '900 ms' "$ROOT/.github/pull_request_template.md" \
    || fail "pull requests must preserve the reclaim hold"
grep -Fq 'com.buddyhq.headroom-rs' "$ROOT/.github/pull_request_template.md" \
    || fail "pull requests must preserve the bundle identifier"

echo "community file checks passed"
