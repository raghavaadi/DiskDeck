# Trusted Release Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a fail-closed owner-Mac workflow that publishes a Developer ID-signed, notarized, checksummed DiskDeck GitHub Release and refuses the current development identity.

**Architecture:** Keep parsing and policy in a small tested POSIX shell library. Extend the existing bundler with an explicit distribution mode, validate the final ZIP independently, and let a draft-first release orchestrator mutate Git/GitHub only after local proof passes.

**Tech Stack:** POSIX shell/zsh, Cargo, macOS code-signing and notarization tools, Git, GitHub CLI, GitHub Actions.

## Global Constraints

- `CFBundleIdentifier` stays exactly `com.buddyhq.headroom-rs`.
- Local QA keeps `Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)`.
- Public assets require `Developer ID Application:`, hardened runtime, secure timestamp, notarization, and stapling.
- Notary credentials are referenced only by a macOS Keychain profile name.
- GitHub CI remains read-only and never builds, signs, notarizes, or uploads the app.
- Release automation never force-pushes, deletes remote state, or overwrites assets.
- No new Rust crate dependency.

---

### Task 1: Release policy library and contract tests

**Files:**
- Create: `scripts/release-lib.sh`
- Create: `scripts/test-release.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `scripts/test-community-files.sh`

**Interfaces:**
- Produces: `diskdeck_package_version ROOT`, `diskdeck_validate_tag TAG`, `diskdeck_tag_version TAG`, `diskdeck_is_distribution_identity NAME`, `diskdeck_extract_release_notes CHANGELOG TAG OUTPUT`.
- Consumes: the `[package]` section in `Cargo.toml` and `## [X.Y.Z] - DATE` changelog headings.

- [ ] **Step 1: Write failing policy tests**

Test `v1.0.0` and `v0.2.3` as valid; reject `1.0.0`, `v1.0`, leading-zero components, prereleases, and four components. Accept only identities beginning `Developer ID Application: `. Create a two-version changelog fixture and assert exact single-section extraction.

- [ ] **Step 2: Run the missing-library proof**

Run: `scripts/test-release.sh`

Expected: non-zero because `scripts/release-lib.sh` is absent.

- [ ] **Step 3: Implement the pure library**

Use this exact SemVer predicate:

```sh
diskdeck_validate_tag() {
    printf '%s\n' "${1-}" | LC_ALL=C grep -Eq \
        '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
}
```

Use `awk` limited to `[package]` for the Cargo version. Use another `awk` program that begins after the exact version heading and stops at the next `## [` heading; fail on a missing or empty section. Do not use `eval`.

- [ ] **Step 4: Add release tests to read-only CI**

Add `scripts/test-release.sh` after `scripts/test-package-artifact.sh`. Extend `scripts/test-community-files.sh` to require the step while retaining its existing ban on `make-app.sh`, artifact upload, `codesign`, and signing identities in workflow YAML.

Run: `scripts/test-release.sh && scripts/test-community-files.sh`

Expected: both print their passing summaries.

- [ ] **Step 5: Commit**

```sh
git add scripts/release-lib.sh scripts/test-release.sh scripts/test-community-files.sh .github/workflows/ci.yml
git commit -m "Test public release policy"
```

### Task 2: Distribution bundling and artifact proof

**Files:**
- Modify: `make-app.sh`
- Create: `scripts/check-release-artifact.sh`
- Modify: `scripts/test-package-artifact.sh`
- Modify: `scripts/test-release.sh`

**Interfaces:**
- Consumes: library functions plus `DISKDECK_DISTRIBUTION`, `DISKDECK_SIGN_IDENTITY`, `DISKDECK_NOTARY_PROFILE`, `DISKDECK_BUILD_NUMBER`, and `DISKDECK_NO_OPEN`.
- Produces: a local QA ZIP or a Developer ID-signed, stapled public ZIP.

- [ ] **Step 1: Add failing build-mode contracts**

Require Cargo-derived bundle version, positive integer build number, the fixed bundle ID, release rejection of development/ad-hoc identities, `--options runtime`, `--timestamp`, `notarytool submit`, `stapler staple`, `stapler validate`, and `spctl`. Make the release artifact checker reject the existing development-signed ZIP.

- [ ] **Step 2: Run the failure proof**

Run: `scripts/test-release.sh && scripts/test-package-artifact.sh`

Expected: non-zero on the first absent distribution contract.

- [ ] **Step 3: Split local QA and distribution modes**

Source `release-lib.sh`, validate Cargo version/build number, and set both Info.plist version keys through `PlistBuddy`. In distribution mode, verify the identity name/class before building, sign using:

```sh
codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
```

ZIP the signed app to `$BUILD/notary-upload.zip`, then run:

```sh
xcrun notarytool submit "$BUILD/notary-upload.zip" \
  --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
```

Distribution mode skips `/Applications` installation and all GUI opens. Default mode preserves current install/sign behavior; `DISKDECK_NO_OPEN=1` suppresses its opens.

- [ ] **Step 4: Implement independent release ZIP validation**

`scripts/check-release-artifact.sh ZIP VERSION` first calls `check-dist.sh`, extracts on APFS, and checks strict/deep code signature, `Authority=Developer ID Application:`, runtime flag, timestamp, exact bundle ID/version, `stapler validate`, and `spctl --assess --type execute --verbose=4`.

- [ ] **Step 5: Verify both boundaries**

```sh
scripts/test-release.sh
scripts/test-package-artifact.sh
DISKDECK_NO_OPEN=1 ./make-app.sh
scripts/check-dist.sh dist/DiskDeck.zip
```

Expected: tests pass; local output is labelled QA-only and the public checker rejects its Apple Development identity.

- [ ] **Step 6: Commit**

```sh
git add make-app.sh scripts/check-release-artifact.sh scripts/test-package-artifact.sh scripts/test-release.sh
git commit -m "Require notarized release artifacts"
```

### Task 3: Draft-first GitHub release orchestration

**Files:**
- Create: `scripts/release.sh`
- Modify: `scripts/test-release.sh`
- Create: `CHANGELOG.md`

**Interfaces:**
- Consumes: the release library/checker, `make-app.sh`, exact main CI, Developer ID identity, and notary profile.
- Produces: annotated tag, draft-verified published Release, `DiskDeck.zip`, and `SHA256SUMS.txt`.

- [ ] **Step 1: Write failing orchestration tests and curated v1 notes**

Add `CHANGELOG.md` v1.0.0 notes for the live map, guided reclaim, Search, External drives, Folder Lens, history/restore, growth/forecast/developer evidence, Apple Silicon/macOS 12+, and notarized installation. Require the release script to contain clean/synced-main proof, exact-HEAD CI proof, existing-tag/release refusal, `--draft`, `--verify-tag`, asset re-download, `shasum -a 256 -c`, and `--draft=false`. Reject `--clobber`, `--force`, `tag -f`, `release delete`, and `git push --tags`.

- [ ] **Step 2: Run the missing-orchestrator proof**

Run: `scripts/test-release.sh`

Expected: non-zero because `scripts/release.sh` is absent.

- [ ] **Step 3: Implement non-mutating preflight**

Accept only `TAG [--publish]`. Require canonical tag/package version match, clean `main`, local `HEAD == origin/main == GitHub main`, exact completed/successful CI for `HEAD`, expected `raghavaadi/DiskDeck` repo, absent local/remote tag and Release, available Developer ID identity, usable notary Keychain profile, and non-empty version notes. Without `--publish`, print readiness and exit before building/tagging.

- [ ] **Step 4: Implement build-before-mutation publication**

Run format, shell guards, and locked Rust tests. Invoke distribution/no-open `make-app.sh`, independently validate the ZIP, then create `SHA256SUMS.txt`. Only afterward create/push one annotated tag, create a draft Release with `--verify-tag`, upload the two assets, download both into a new temp directory, verify the checksum and peeled tag SHA, prove the Release is still a two-asset draft, then publish with `gh release edit "$TAG" --draft=false`.

- [ ] **Step 5: Prove the current identity fails closed**

Run:

```sh
DISKDECK_SIGN_IDENTITY='Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)' \
  scripts/release.sh v1.0.0
git tag --list v1.0.0
git ls-remote --tags origin refs/tags/v1.0.0
gh release view v1.0.0
```

Expected: the first command fails with `public releases require Developer ID Application`; both tag queries are empty and GitHub reports no Release.

- [ ] **Step 6: Commit**

```sh
git add scripts/release.sh scripts/test-release.sh CHANGELOG.md
git commit -m "Add draft-first release command"
```

### Task 4: Public and maintainer release guidance

**Files:**
- Modify: `README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `scripts/test-release.sh`

**Interfaces:**
- Consumes: Tasks 1–3 command names and safety boundaries.
- Produces: accurate public installation copy and a credential-free maintainer runbook.

- [ ] **Step 1: Add failing doc assertions**

Require the latest-Releases URL, Developer ID/notarized wording, a local-QA-only warning for default `make-app.sh`, `scripts/release.sh v1.0.0`, and the ban on publishing Apple Development/ad-hoc output.

- [ ] **Step 2: Run the stale-copy proof**

Run: `scripts/test-release.sh`

Expected: non-zero on the missing Releases link or distribution warning.

- [ ] **Step 3: Update docs**

Make the README recommended path start at GitHub Releases and keep the installer/FDA steps. Explain source/local builds are not the notarized public app. Replace the manual release checklist with preflight and `--publish` commands. Document only Keychain profile/identity names, never secret values. Add the public-release invariant to both agent files.

- [ ] **Step 4: Verify and commit**

```sh
scripts/test-release.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
git add README.md CONTRIBUTING.md AGENTS.md CLAUDE.md scripts/test-release.sh
git commit -m "Document trusted macOS releases"
```

### Task 5: Full proof, PR integration, and conditional v1 publication

**Files:**
- Verify only: all Task 1–4 files.

**Interfaces:**
- Consumes: complete implementation.
- Produces: green merged-HEAD CI and, only with valid credentials, live `v1.0.0`.

- [ ] **Step 1: Run the full local suite**

Run:

```sh
cargo fmt -- --check
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-release.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
cargo test --locked
git diff --check
```

Expected: all shell guards pass; 194 Rust tests pass and one fixture is ignored.

- [ ] **Step 2: Push and integrate through review**

Push `codex/trusted-release`, open a draft PR, inspect the complete diff and checks, mark ready, merge to `main`, and update local main without rewriting history.

- [ ] **Step 3: Verify exact merged-HEAD CI**

Wait for the CI run whose `headSha` equals merged `main`; do not accept a green run for another commit.

- [ ] **Step 4: Run public preflight**

Run: `scripts/release.sh v1.0.0`

Expected today: precise Developer ID/notary prerequisite failure with no remote mutation. If credentials exist, expected: readiness with no mutation.

- [ ] **Step 5: Publish only when Step 4 passes**

Run `scripts/release.sh v1.0.0 --publish`, re-download both live assets, verify `SHA256SUMS.txt`, compare tag commit to merged main, and inspect the Release is public. If credentials are absent, report that single external prerequisite and do not fall back to development-signed or source-only publication.
