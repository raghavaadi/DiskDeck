# DiskDeck Universal macOS App — Design Specification

**Status:** Approved under the owner's standing AFK/repository-owner authorization on 2026-07-12

## Goal

Ship one DiskDeck app that runs natively on Apple Silicon and 64-bit Intel Macs
without separate downloads, architecture choices, reduced safety checks, or a
different permission identity.

The supported platform becomes macOS 12 or later on `arm64` and `x86_64`.

## Evidence before design

- The current signed app is a thin `arm64` Mach-O executable.
- `x86_64-apple-darwin` was not initially installed on the owner Mac.
- After installing the Rust standard-library target, the unchanged DiskDeck
  source built successfully with:

  ```sh
  MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked \
    --target x86_64-apple-darwin
  ```

- The resulting executable is a valid `x86_64` Mach-O and its load command
  reports minimum macOS `12.0`.
- GitHub's current standard `macos-14` runner is Apple Silicon. An explicit
  `macos-15-intel` runner is therefore required for native Intel test proof.

## Approaches considered

### A. One universal app for QA and release (chosen)

Build both Rust targets, merge the two executables with `lipo`, then sign and
package the universal app through the existing stable QA or trusted release
path. The locally installed app and public artifact have the same architecture
shape.

This gives ordinary users one download and keeps visual/TCC testing aligned
with the artifact that will ship.

### B. Universal public releases, native-only local QA

This shortens local build time but makes the owner's most common signed-app
checks exercise a different executable layout from the public artifact.

### C. Separate Apple Silicon and Intel release assets

This produces smaller downloads but asks ordinary users to understand CPU
architecture and doubles release/upload/checksum opportunities for drift.

## Universal build contract

`make-app.sh` always produces a universal executable. `cargo run` remains the
fast native development loop.

The ship script:

1. Requires both Rust targets to be installed:
   - `aarch64-apple-darwin`
   - `x86_64-apple-darwin`
2. Fails before building with one actionable `rustup target add` command when
   either target is missing. It never installs toolchains implicitly.
3. Exports `MACOSX_DEPLOYMENT_TARGET=12.0` so the executable slices agree with
   `LSMinimumSystemVersion`.
4. Runs locked release builds for each explicit target.
5. Merges only those two resulting executables into the bundle executable:

   ```sh
   lipo -create \
     target/aarch64-apple-darwin/release/diskdeck \
     target/x86_64-apple-darwin/release/diskdeck \
     -output DiskDeck.app/Contents/MacOS/diskdeck
   ```

6. Verifies exactly two slices exist and that both `arm64` and `x86_64` are
   present before any code signing, installation, notarization, or packaging.

There is no environment override for a thin ship artifact. A developer who
wants a fast native loop uses `cargo run`; a ZIP from `make-app.sh` is always
universal.

## Signature, identity, and artifact proof

- The bundle identifier stays `com.buddyhq.headroom-rs`.
- Local QA keeps the current Apple Development identity.
- Public distribution keeps the exact Developer ID team `65KMSM8WL8` and all
  existing hardened-runtime, timestamp, notarization, stapling, Gatekeeper,
  checksum, draft, and downloaded-asset gates.
- Code signing happens after `lipo` assembly. No slice is modified after
  signing.
- `scripts/check-release-artifact.sh` proves the downloaded app executable has
  exactly the two expected slices in addition to its current trust checks.
- The structural package test still rejects unsigned fixtures before it needs
  to reason about architectures.

## CI proof

Keep GitHub Actions read-only and free of app signing or artifact upload.

Use two native jobs:

1. **Apple Silicon checks** on `macos-14`:
   - assert `uname -m` is `arm64`;
   - run formatting, shell/repository guards, and `cargo test --locked`.
2. **Intel checks** on `macos-15-intel`:
   - assert `uname -m` is `x86_64`;
   - install stable Rust with `rustfmt`;
   - run `cargo test --locked`.

The workflow succeeds only when both native architecture jobs pass. The
release preflight already requires the complete exact-commit workflow result,
so it automatically gains both witnesses without changing its GitHub query.

## User and contributor experience

- README installation requirements become “Mac with Apple Silicon or a 64-bit
  Intel processor, macOS 12+.” Users still download one `DiskDeck.zip`.
- Remove the v1.0.0 Intel limitation from the changelog and state the universal
  binary support explicitly.
- Contributor setup documents the two one-time `rustup target add` targets for
  signed QA builds; ordinary native development remains `cargo run`.
- AGENTS and CLAUDE require exact two-slice verification for every ship build.

## Verification

1. Add failing static/fixture tests for the two targets, `lipo` assembly, exact
   slice check, deployment target, README copy, and two CI runner labels.
2. Implement the minimal universal build and artifact checks.
3. Run both per-target locked release builds locally.
4. Run `make-app.sh`, then prove the installed executable and the executable
   extracted from `dist/DiskDeck.zip` each report exactly `arm64 x86_64`.
5. Verify the installed app's stable bundle ID/signing authority, launch it,
   run the non-destructive signed UI smoke test, and inspect the live window.
6. Run every repository guard plus the full Rust suite.
7. Push a PR and require both native GitHub jobs for its exact head.
8. Merge, then require both jobs again for the exact merged commit.

## Explicitly deferred

- 32-bit Intel Macs; macOS 12 cannot run on them.
- Separate per-architecture assets.
- Architecture-specific feature flags or UI.
- Publishing v1.0.0 before the separate Developer ID/notary prerequisite is
  available.
