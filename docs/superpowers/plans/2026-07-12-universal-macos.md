# Universal macOS App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce and verify one signed DiskDeck app containing native `arm64` and `x86_64` slices, with native CI proof on both Mac architectures.

**Architecture:** Add one reusable exact-slice checker, exercise it with compiler-generated Mach-O fixtures, and call it before signing and after extracting a public release ZIP. Make `make-app.sh` build both explicit Rust targets with a macOS 12 deployment floor and merge them with `lipo`; keep CI read-only but add an actual Intel test job.

**Tech Stack:** Rust/Cargo/rustup, POSIX shell and zsh, Apple `clang`/`lipo`/`otool`/`codesign`, GitHub Actions.

## Global Constraints

- Supported platform is macOS 12+ on `arm64` and 64-bit `x86_64`.
- Every `make-app.sh` artifact contains exactly `arm64` and `x86_64`; no thin ship override exists.
- `cargo run` remains the fast native-only development loop.
- `CFBundleIdentifier` remains exactly `com.buddyhq.headroom-rs`.
- Local QA keeps the current Apple Development identity; public release keeps TeamIdentifier `65KMSM8WL8` and every existing notarization/release gate.
- `lipo` assembly and exact-slice validation happen before code signing.
- GitHub CI remains `contents: read` and never signs, notarizes, or uploads an app.
- No new Rust crate dependency.

---

### Task 1: Exact universal-binary checker with real Mach-O fixtures

**Files:**
- Create: `scripts/check-universal-binary.sh`
- Create: `scripts/test-universal.sh`
- Modify: `scripts/test-community-files.sh`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `scripts/check-universal-binary.sh EXECUTABLE`, exit 0 only for exactly two slices containing `arm64` and `x86_64`.
- Consumes: a Mach-O executable and system `lipo`.

- [ ] **Step 1: Write a failing fixture test**

Use system `clang` to compile the same minimal program for both targets without creating source files:

```sh
printf '%s\n' 'int main(void) { return 0; }' | \
  xcrun clang -x c -arch arm64 -mmacosx-version-min=12.0 - -o "$TMP/arm64"
printf '%s\n' 'int main(void) { return 0; }' | \
  xcrun clang -x c -arch x86_64 -mmacosx-version-min=12.0 - -o "$TMP/x86_64"
lipo -create "$TMP/arm64" "$TMP/x86_64" -output "$TMP/universal"
```

Assert the checker accepts `$TMP/universal`, rejects both thin binaries, rejects a missing file, and prints an actionable “exactly arm64 and x86_64” error. Also require `scripts/test-universal.sh` in the workflow/community contract.

- [ ] **Step 2: Run the red proof**

Run: `scripts/test-universal.sh`

Expected: non-zero with `missing scripts/check-universal-binary.sh`.

- [ ] **Step 3: Implement the exact-slice checker**

Read `lipo -archs`, normalize it as whitespace-separated words, require word count `2`, and use:

```sh
lipo -verify_arch arm64 x86_64 "$EXECUTABLE"
```

Fail on a missing file, `lipo` inspection failure, either missing slice, or any third slice. Print the observed architecture list in the error without dumping binary content.

- [ ] **Step 4: Wire and verify the fixture test**

Add this read-only CI step after package artifact tests:

```yaml
      - name: Test universal binary contract
        run: scripts/test-universal.sh
```

Run: `scripts/test-universal.sh && scripts/test-community-files.sh`

Expected: `universal binary checks passed` and `community file checks passed`.

- [ ] **Step 5: Commit**

```sh
git add scripts/check-universal-binary.sh scripts/test-universal.sh \
  scripts/test-community-files.sh .github/workflows/ci.yml
git commit -m "Test universal binary contract"
```

### Task 2: Universal ship build and downloaded-artifact verification

**Files:**
- Modify: `make-app.sh`
- Modify: `scripts/check-release-artifact.sh`
- Modify: `scripts/test-universal.sh`
- Modify: `scripts/test-release.sh`

**Interfaces:**
- Consumes: installed Rust targets `aarch64-apple-darwin` and `x86_64-apple-darwin`.
- Produces: `DiskDeck.app/Contents/MacOS/diskdeck` with exactly `arm64 x86_64`, minimum macOS 12.0 in both slices.

- [ ] **Step 1: Add failing ship-script contracts**

Require `make-app.sh` to contain both target triples, `MACOSX_DEPLOYMENT_TARGET=12.0`, two `cargo build --release --locked --target` invocations through a fixed loop, `lipo -create`, and `scripts/check-universal-binary.sh` before the first `codesign`. Require `check-release-artifact.sh` to call the same checker on the extracted executable.

- [ ] **Step 2: Run the red proof**

Run: `scripts/test-universal.sh`

Expected: non-zero on `make-app.sh is missing universal contract`.

- [ ] **Step 3: Replace the native build with two explicit locked builds**

Before any build, compare the installed target list with the two fixed targets. On failure print exactly:

```text
Install universal Rust targets once:
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

Then export `MACOSX_DEPLOYMENT_TARGET=12.0` and run:

```sh
for TARGET in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked --target "$TARGET"
done
```

Copy no thin binary into the app. Merge the two fixed paths directly to the bundle executable, then invoke `scripts/check-universal-binary.sh` before signing.

- [ ] **Step 4: Extend public artifact proof**

After extracting `DiskDeck.zip`, call:

```sh
"$ROOT/scripts/check-universal-binary.sh" \
  "$APP/Contents/MacOS/diskdeck"
```

Keep Developer ID, team ID, runtime, timestamp, bundle metadata, stapler, and Gatekeeper checks unchanged.

- [ ] **Step 5: Run contract and per-target build proof**

```sh
scripts/test-universal.sh
MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked --target aarch64-apple-darwin
MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked --target x86_64-apple-darwin
```

Expected: test passes and both builds finish successfully.

- [ ] **Step 6: Commit**

```sh
git add make-app.sh scripts/check-release-artifact.sh \
  scripts/test-universal.sh scripts/test-release.sh
git commit -m "Build one universal DiskDeck app"
```

### Task 3: Native Apple Silicon and Intel CI witnesses

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `scripts/test-community-files.sh`
- Modify: `scripts/test-universal.sh`

**Interfaces:**
- Consumes: GitHub public standard runners `macos-14` and `macos-15-intel`.
- Produces: one workflow conclusion that is successful only when native arm64 and native x86_64 tests pass.

- [ ] **Step 1: Add failing CI-shape tests**

Require two job IDs, the exact runner labels, expected architecture assertions, and `cargo test --locked` in each job. Continue rejecting any workflow occurrence of `make-app.sh`, `codesign`, notarization, signing identities, or artifact upload.

- [ ] **Step 2: Run the red proof**

Run: `scripts/test-universal.sh && scripts/test-community-files.sh`

Expected: non-zero because `macos-15-intel` and the Intel job are absent.

- [ ] **Step 3: Split the workflow into native jobs**

Keep the full existing job on `macos-14`, rename it `Apple Silicon checks`, and add before its tests:

```sh
test "$(uname -m)" = arm64
```

Add an `intel-test` job on `macos-15-intel`, reuse the pinned checkout SHA, install stable Rust with rustfmt, assert `uname -m = x86_64`, then run:

```sh
cargo fmt -- --check
cargo test --locked
```

Do not add caches, third-party actions, signing, or artifacts.

- [ ] **Step 4: Verify YAML and contracts**

Run:

```sh
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'
scripts/test-universal.sh
scripts/test-community-files.sh
```

Expected: YAML parses and both contract suites pass.

- [ ] **Step 5: Commit**

```sh
git add .github/workflows/ci.yml scripts/test-community-files.sh scripts/test-universal.sh
git commit -m "Test DiskDeck on both Mac architectures"
```

### Task 4: Platform documentation and release notes

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `CONTRIBUTING.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `scripts/test-universal.sh`

**Interfaces:**
- Consumes: exact build and support policy from Tasks 1–3.
- Produces: one-download Apple Silicon/Intel guidance with no stale Apple-Silicon-only statement.

- [ ] **Step 1: Add failing copy assertions**

Require README to contain `Apple Silicon or a 64-bit Intel processor`, changelog to contain `Universal 2`, contributor docs to show the two-target `rustup target add` command, and both agent files to require exactly `arm64` + `x86_64`. Reject `Intel Macs are not supported` across README and changelog.

- [ ] **Step 2: Run the red proof**

Run: `scripts/test-universal.sh`

Expected: non-zero on the Apple-Silicon-only requirement or stale Intel limitation.

- [ ] **Step 3: Update platform copy**

Change installation requirements to macOS 12+ on Apple Silicon or 64-bit Intel while preserving one `DiskDeck.zip`. In v1.0.0 Distribution notes state `Universal 2 (arm64 + x86_64)` and remove the Intel known limitation. Add the one-time target installation command to contributor/agent build guidance and keep `cargo run` documented as native-only.

- [ ] **Step 4: Verify and commit**

```sh
scripts/test-universal.sh
scripts/test-release.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
git add README.md CHANGELOG.md CONTRIBUTING.md AGENTS.md CLAUDE.md scripts/test-universal.sh
git commit -m "Document universal Mac support"
```

### Task 5: Signed universal proof and GitHub integration

**Files:**
- Verify only: all files changed in Tasks 1–4.

**Interfaces:**
- Consumes: complete universal implementation and stable local signing identity.
- Produces: signed universal installed/ZIP artifacts, native arm/Intel CI success, merged clean main.

- [ ] **Step 1: Run the complete local logic suite**

Run:

```sh
cargo fmt -- --check
zsh -n make-app.sh scripts/install.command
sh -n scripts/check-universal-binary.sh scripts/test-universal.sh \
  scripts/check-release-artifact.sh scripts/release.sh
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-universal.sh
scripts/test-release.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
cargo test --locked
git diff --check
```

Expected: every shell guard passes; 194 Rust tests pass and one fixture is ignored.

- [ ] **Step 2: Build and inspect the signed universal app**

```sh
DISKDECK_NO_OPEN=1 ./make-app.sh
scripts/check-universal-binary.sh /Applications/DiskDeck.app/Contents/MacOS/diskdeck
codesign --verify --deep --strict /Applications/DiskDeck.app
lipo -archs /Applications/DiskDeck.app/Contents/MacOS/diskdeck
```

Expected: build/ZIP checks pass, signature is valid, and architectures are exactly `x86_64 arm64` or `arm64 x86_64`.

- [ ] **Step 3: Inspect the packaged executable**

Run:

```sh
PKG_TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-universal-proof.XXXXXX")
COPYFILE_DISABLE=1 ditto -x -k dist/DiskDeck.zip "$PKG_TMP"
PACKAGED="$PKG_TMP/DiskDeck/DiskDeck.app/Contents/MacOS/diskdeck"
scripts/check-universal-binary.sh "$PACKAGED"
for ARCH in arm64 x86_64; do
  lipo "$PACKAGED" -thin "$ARCH" -output "$PKG_TMP/$ARCH"
  otool -l "$PKG_TMP/$ARCH" | awk \
    '/LC_BUILD_VERSION/{seen=1} seen && /minos/{print; exit}' | \
    grep -Fq 'minos 12.0'
done
rm -rf "$PKG_TMP"
```

Expected: packaged checker passes and both thin-slice load commands report `minos 12.0`.

- [ ] **Step 4: Run signed interaction and visual proof**

Run:

```sh
open /Applications/DiskDeck.app
scripts/test-signed-ui.sh
codesign -dv --verbose=4 /Applications/DiskDeck.app 2>&1 | \
  grep -E 'Identifier=com\.buddyhq\.headroom-rs|Authority=Apple Development:'
```

Then inspect the live window in both appearances at the supported minimum layout. The app must retain its bundle ID/signing authority and no tofu/overlap regression may appear.

- [ ] **Step 5: Push, review, and merge**

Push `codex/universal-macos`, open a draft PR, and require both `Apple Silicon checks` and `Intel checks` for the exact PR head. Merge only after both pass; then require both again for the exact merged `main` SHA.

- [ ] **Step 6: Clean up and re-run release preflight**

Remove the owned worktree/merged branch, confirm clean synchronized main, and run the v1.0.0 release preflight with the current Apple Development identity. Expected: it reaches and fails only at the Developer ID gate, with no tag or Release created.
