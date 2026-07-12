# DiskDeck Trusted Release Pipeline — Design Specification

**Status:** Approved under the owner's standing AFK/repository-owner authorization on 2026-07-12

## Goal

Turn the signed local ship path into a repeatable, auditable public GitHub
Release workflow without committing credentials, weakening DiskDeck's stable
macOS identity, or presenting a development-signed build as public-ready.

The first public binary release is `v1.0.0`. It may be published only after the
owner's keychain contains a valid `Developer ID Application` identity and an
Apple notary-service keychain profile.

## Current state

- `main` is clean, synchronized with `origin/main`, and green in GitHub CI.
- The repository has no tags or GitHub Releases.
- `Cargo.toml` declares version `1.0.0`; `make-app.sh` duplicates that value in
  `Info.plist`.
- `make-app.sh` produces a structurally valid `dist/DiskDeck.zip` and installs
  a stable locally signed app for Full Disk Access and UI testing.
- The only Apple identity currently available is `Apple Development`. Apple
  requires `Developer ID Application`, hardened runtime, a secure timestamp,
  and notarization for trustworthy direct distribution outside the Mac App
  Store.
- GitHub-hosted runners do not have the owner's private signing identity or
  notary credentials. CI must not publish an unsigned or ad-hoc substitute.

## Approaches considered

### A. Owner-Mac signing and notarization, GitHub-hosted release (chosen)

Build, sign, notarize, staple, package, and validate on the owner's Mac. Use a
local release command to create an annotated tag, a draft GitHub Release, and
checksum assets, verify the uploaded state, then publish.

This preserves the private-key boundary, reuses the real ship path, and gives
users a normal GitHub download without overstating what CI can prove.

### B. Import signing credentials into GitHub Actions

This can automate the binary build, but requires exporting the private key and
storing signing and notarization credentials as repository secrets. That is a
larger security boundary than a single-maintainer v1 needs and is deferred.

### C. Source-only GitHub Releases

This is honest and easy, but does not give everyday Mac users an installable
app. Source archives remain available automatically from Git tags, but they are
not the primary distribution experience.

## Build modes

`make-app.sh` retains two explicit modes:

1. **Local QA mode (default).** It may use the existing stable development
   identity, install to `/Applications`, and open DiskDeck for signed UI work.
   Its ZIP is a test artifact, not a public release.
2. **Distribution mode.** Enabled only by the release orchestrator. It requires
   an identity whose name starts with `Developer ID Application:`, signs with
   DiskDeck's exact team identifier `65KMSM8WL8`, hardened runtime, and a secure
   timestamp; submits the app to Apple's notary service; staples and validates
   the ticket; packages the stapled app; and performs Gatekeeper assessment.
   It does not open the app or System Settings.

Both modes derive `CFBundleShortVersionString` from the validated Cargo package
version. `CFBundleVersion` comes from an optional positive integer build-number
override and defaults to `1`. The bundle identifier remains exactly
`com.buddyhq.headroom-rs`, and the local QA signing default remains unchanged.

Notary credentials are referenced by a keychain profile name. They are never
accepted as command-line secrets, environment values containing passwords, or
repository files.

## Release command

Add `scripts/release.sh TAG` with an explicit `--publish` switch.

Without `--publish`, the command is a non-mutating preflight. It verifies the
source/repository boundary and reports missing prerequisites. With `--publish`,
it executes the full release transaction.

Preflight must fail unless all of the following are true:

- `TAG` is canonical SemVer in the form `vMAJOR.MINOR.PATCH`.
- The tag version exactly matches the package version in `Cargo.toml`.
- The checkout is `main`, clean, and has no untracked files other than ignored
  output.
- Local `HEAD`, `origin/main`, and GitHub's `main` SHA are identical.
- The exact `HEAD` has a completed successful `CI` workflow run.
- The tag and GitHub Release do not already exist locally or remotely.
- `gh` is authenticated to the expected `raghavaadi/DiskDeck` repository.
- A `Developer ID Application` identity and named notary keychain profile are
  usable.
- Curated notes for the version exist in `CHANGELOG.md`.

Publish then:

1. Runs the complete locked test and repository-guard suite.
2. Calls `make-app.sh` in distribution/no-open mode.
3. Re-verifies the exact app signature, hardened runtime, timestamp,
   notarization ticket, Gatekeeper assessment, archive layout, bundle ID, and
   version.
4. Writes `dist/SHA256SUMS.txt` for `DiskDeck.zip`.
5. Creates an annotated local tag at the already-verified `HEAD` and pushes
   that one tag.
6. Creates a draft GitHub Release using curated version notes and uploads
   `DiskDeck.zip` plus `SHA256SUMS.txt`.
7. Re-downloads or queries the release assets, verifies their names, sizes,
   and checksums, and confirms the release tag targets the expected SHA.
8. Publishes the draft only after every verification passes.

Remote mutations occur only after the distributable is fully built and
validated. If draft creation or asset verification fails after the tag push,
the script leaves the release as a draft and prints exact recovery guidance;
it never force-moves or deletes a tag automatically.

## CI boundary

GitHub CI remains read-only and does not build or upload the app. It gains a
release-contract test that validates:

- version parsing and tag matching;
- public-release rejection of development/ad-hoc identities;
- required distribution flags and validation commands;
- release scripts' shell syntax;
- the README and maintainer docs do not call a development-signed ZIP a public
  release.

The existing community-file guard continues to reject workflows that invoke
`make-app.sh`, `codesign`, or artifact upload.

## User-facing release content

- Add `CHANGELOG.md` with curated `v1.0.0` highlights, installation notes,
  platform support, safety boundaries, and honest known limitations.
- Update README installation to point to GitHub Releases and distinguish the
  notarized public download from local source builds.
- Replace the four-line release checklist with the tested release command and
  its Developer ID/notary prerequisites.
- Add a maintainer-only release section to `CONTRIBUTING.md` and reinforce in
  `AGENTS.md` that development signatures must never be published.

## Security and failure handling

- No private key, certificate export, Apple account password, app-specific
  password, API key, or notary credential enters Git, GitHub Actions, command
  history, or release notes.
- The public path fails closed on the certificate class and every validation
  boundary; an override may select a different Developer ID identity or
  keychain-profile name but may not bypass the checks.
- Release assets contain no AppleDouble files, unsafe paths, `.env` files,
  logs, source checkout, or machine-specific paths.
- The release command never deletes remote tags/releases, force-pushes, or
  overwrites an existing asset.
- Local QA remains available when public distribution prerequisites are
  missing, and its output is visibly labelled non-distributable.

## Verification

1. Run shell syntax and fixture tests for all release failure/success branches.
2. Run `cargo fmt -- --check`, all tracked shell guards, and
   `cargo test --locked`.
3. Run the release preflight with only the current Apple Development identity
   and prove it stops before build, tag, or GitHub mutation.
4. Push the automation changes and wait for exact-HEAD CI success.
5. Once a Developer ID identity and notary keychain profile exist, run the
   distribution build and verify `codesign`, `stapler`, `spctl`, archive
   layout, bundle metadata, and SHA-256 locally.
6. Publish `v1.0.0` through the scripted draft-first path.
7. Inspect the live GitHub Release, download both assets, compare checksums,
   verify the tag SHA, and test the installer on a clean macOS account or Mac
   before calling the release complete.

## Explicitly deferred

- Exporting signing material to GitHub Actions.
- Homebrew cask distribution and Sparkle auto-update feeds.
- Separate per-architecture assets; v1 uses one Universal 2/macOS 12+ ZIP.
- Mac App Store distribution.
- Publishing any binary before Developer ID signing and notarization are
  proven end to end.
