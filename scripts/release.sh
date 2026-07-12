#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
. scripts/release-lib.sh

REPOSITORY='raghavaadi/DiskDeck'
PUBLISH=0
MUTATION_STARTED=0
TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-release.XXXXXX")

cleanup() {
    status=${1:-0}
    trap - 0 1 2 15
    rm -rf "$TMP"
    if [ "$status" -ne 0 ] && [ "$MUTATION_STARTED" -eq 1 ]; then
        echo "Release stopped after tag creation. Inspect the remote tag and draft; nothing was deleted or force-moved." >&2
    fi
    exit "$status"
}
trap 'cleanup $?' 0
trap 'exit 130' 1 2 15

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

usage() {
    echo "usage: scripts/release.sh vMAJOR.MINOR.PATCH [--publish]" >&2
    exit 2
}

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
TAG=$1
if [ "$#" -eq 2 ]; then
    [ "$2" = '--publish' ] || usage
    PUBLISH=1
fi

diskdeck_validate_tag "$TAG" || fail "tag must be canonical SemVer (for example v1.0.0)"
VERSION=$(diskdeck_tag_version "$TAG")
PACKAGE_VERSION=$(diskdeck_package_version "$ROOT") \
    || fail "cannot read Cargo package version"
[ "$VERSION" = "$PACKAGE_VERSION" ] \
    || fail "$TAG does not match Cargo package version $PACKAGE_VERSION"

NOTES_FILE="$TMP/release-notes.md"
diskdeck_extract_release_notes "$ROOT/CHANGELOG.md" "$TAG" "$NOTES_FILE" \
    || fail "CHANGELOG.md has no non-empty section for $TAG"

[ "$(git branch --show-current)" = 'main' ] \
    || fail "release from main, not $(git branch --show-current)"
[ -z "$(git status --porcelain --untracked-files=normal)" ] \
    || fail "working tree must be clean before release"
[ "$(gh repo view --json nameWithOwner --jq .nameWithOwner)" = "$REPOSITORY" ] \
    || fail "GitHub CLI is not targeting $REPOSITORY"
gh auth status >/dev/null 2>&1 || fail "GitHub CLI is not authenticated"

git fetch --quiet origin main
HEAD_SHA=$(git rev-parse HEAD)
ORIGIN_SHA=$(git rev-parse origin/main)
GITHUB_SHA=$(gh api "repos/raghavaadi/DiskDeck/commits/main" --jq .sha)
[ "$HEAD_SHA" = "$ORIGIN_SHA" ] \
    || fail "local main is not synchronized with origin/main"
[ "$HEAD_SHA" = "$GITHUB_SHA" ] \
    || fail "local main does not match GitHub main"

CI_SHA=$(gh run list \
    --workflow CI \
    --branch main \
    --commit "$HEAD_SHA" \
    --limit 1 \
    --json conclusion,headSha,status \
    --jq 'map(select(.status == "completed" and .conclusion == "success"))[0].headSha // ""')
[ "$CI_SHA" = "$HEAD_SHA" ] \
    || fail "exact main commit $HEAD_SHA does not have successful completed CI"

if git show-ref --verify --quiet "refs/tags/$TAG"; then
    fail "local tag already exists: $TAG"
fi
if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
    fail "remote tag already exists: $TAG"
fi
if gh release view "$TAG" --repo "$REPOSITORY" >/dev/null 2>&1; then
    fail "GitHub Release already exists: $TAG"
fi

IDENTITY=${DISKDECK_SIGN_IDENTITY:-}
[ -n "$IDENTITY" ] \
    || fail "set DISKDECK_SIGN_IDENTITY to a Developer ID Application identity"
diskdeck_is_distribution_identity "$IDENTITY" \
    || fail "public releases require Developer ID Application signing"
security find-identity -v -p codesigning | grep -Fq "\"$IDENTITY\"" \
    || fail "Developer ID Application identity is not available in the keychain"

NOTARY_PROFILE=${DISKDECK_NOTARY_PROFILE:-DiskDeck-Notary}
xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
    || fail "notary keychain profile is unavailable: $NOTARY_PROFILE"

if [ "$PUBLISH" -eq 0 ]; then
    echo "release preflight passed for $TAG at $HEAD_SHA"
    exit 0
fi

cargo fmt -- --check
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-release.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
cargo test --locked

DISKDECK_DISTRIBUTION=1 \
DISKDECK_NO_OPEN=1 \
DISKDECK_SIGN_IDENTITY="$IDENTITY" \
DISKDECK_NOTARY_PROFILE="$NOTARY_PROFILE" \
    ./make-app.sh
scripts/check-release-artifact.sh "dist/DiskDeck.zip" "$VERSION"
(cd dist && shasum -a 256 DiskDeck.zip > SHA256SUMS.txt)

[ -z "$(git status --porcelain --untracked-files=normal)" ] \
    || fail "tracked source changed during the release build"
[ "$(git rev-parse HEAD)" = "$HEAD_SHA" ] \
    || fail "HEAD changed during the release build"

git tag -a "$TAG" -m "DiskDeck $TAG"
MUTATION_STARTED=1
git push origin "refs/tags/$TAG:refs/tags/$TAG"

gh release create "$TAG" \
    "dist/DiskDeck.zip" \
    "dist/SHA256SUMS.txt" \
    --repo "$REPOSITORY" \
    --title "DiskDeck $TAG" \
    --notes-file "$NOTES_FILE" \
    --draft \
    --verify-tag

ASSET_COUNT=$(gh release view "$TAG" \
    --repo "$REPOSITORY" \
    --json assets,isDraft,tagName \
    --jq ". | select(.isDraft == true and .tagName == \"$TAG\") | .assets | length")
[ "$ASSET_COUNT" = '2' ] \
    || fail "draft release does not contain exactly the two expected assets"

VERIFY_DIR="$TMP/downloaded"
mkdir -p "$VERIFY_DIR"
gh release download "$TAG" \
    --repo "$REPOSITORY" \
    --dir "$VERIFY_DIR" \
    --pattern 'DiskDeck.zip' \
    --pattern 'SHA256SUMS.txt'
(cd "$VERIFY_DIR" && shasum -a 256 -c SHA256SUMS.txt)
scripts/check-release-artifact.sh "$VERIFY_DIR/DiskDeck.zip" "$VERSION"

REMOTE_COMMIT=$(git ls-remote origin "refs/tags/$TAG^{}" | awk 'NR == 1 { print $1 }')
[ "$REMOTE_COMMIT" = "$HEAD_SHA" ] \
    || fail "remote annotated tag does not resolve to release commit"

gh release edit "$TAG" --repo "$REPOSITORY" --draft=false
IS_DRAFT=$(gh release view "$TAG" --repo "$REPOSITORY" --json isDraft --jq .isDraft)
[ "$IS_DRAFT" = 'false' ] || fail "release remained a draft after publication"

URL=$(gh release view "$TAG" --repo "$REPOSITORY" --json url --jq .url)
echo "published and verified $TAG: $URL"
