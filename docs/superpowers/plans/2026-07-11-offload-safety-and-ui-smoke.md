# Offload Safety and Contributor UI Smoke Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent protected or changed sources from reaching SSD mutation boundaries and publish a non-destructive signed-app UI smoke workflow for contributors.

**Architecture:** `offload.rs` becomes the single source of truth for lexical eligibility, filesystem eligibility, target preflight, destination collision, and source identity. `app.rs` consumes the pure lexical decision for inexpensive UI enablement and the full decision before opening the dialog, while the worker repeats full validation. Tracked AppleScript, Swift, and shell tools exercise only Accessibility navigation and context-menu dismissal.

**Tech Stack:** Rust 2021, egui/eframe 0.29, macOS `symlink_metadata` and `MetadataExt`, AppleScript/System Events, Swift/CoreGraphics, POSIX shell.

## Global Constraints

- The scan remains read-only and rooted at `/System/Volumes/Data`.
- Cleanup still requires an explicit selection and the 900 ms hold.
- Offload uses copy, verify, then remove; no original is removed after a failed copy or verification.
- Do not add a crate dependency.
- Do not test delete, command, offload, or restore against the owner's real data.
- UI smoke tooling may navigate Back, open a context menu, and send Escape only; it must never select a menu action, recommendation, or reclaim control.
- Keep `CFBundleIdentifier` exactly `com.buddyhq.headroom-rs` and keep the signing identity default unchanged.

---

### Task 1: Central protected-path eligibility

**Files:**
- Modify: `src/offload.rs:1-55`
- Test: inline `src/offload.rs` test module

**Interfaces:**
- Consumes: `std::path::{Component, Path, PathBuf}` and `std::fs::symlink_metadata`.
- Produces: `pub enum OffloadBlock`, `OffloadBlock::message()`, `pub fn classify_movable(&Path, &Path)`, and `pub fn check_movable(&Path, &Path)`.

- [ ] **Step 1: Replace the broad movable tests with failing policy tests**

Add tests that assert the wished-for pure API:

```rust
#[test]
fn movable_policy_allows_normal_and_custom_home_folders() {
    let home = Path::new("/Users/<user>");
    assert_eq!(classify_movable(Path::new("/Users/<user>/Movies/Big.mov"), home), Ok(()));
    assert_eq!(classify_movable(Path::new("/Users/<user>/Projects/DiskDeck"), home), Ok(()));
}

#[test]
fn movable_policy_blocks_protected_home_roots() {
    let home = Path::new("/Users/<user>");
    for path in [
        "/Users/<user>",
        "/Users/<user>/Library/Caches/App",
        "/Users/<user>/.ssh",
        "/Users/<user>/Applications/Tool.app",
        "/Users/<user>/Public",
        "/Users/<user>/.Trash/file",
    ] {
        assert!(classify_movable(Path::new(path), home).is_err(), "{path}");
    }
}

#[test]
fn movable_policy_blocks_cloud_roots_and_managed_bundles() {
    let home = Path::new("/Users/<user>");
    for path in [
        "/Users/<user>/Dropbox/archive",
        "/Users/<user>/OneDrive - Example/archive",
        "/Users/<user>/Google Drive/archive",
        "/Users/<user>/Pictures/Library.photoslibrary",
        "/Users/<user>/Movies/Edit.fcpbundle",
    ] {
        assert!(classify_movable(Path::new(path), home).is_err(), "{path}");
    }
}

#[test]
fn movable_policy_blocks_non_normalized_external_and_outside_paths() {
    let home = Path::new("/Users/<user>");
    assert_eq!(classify_movable(Path::new("relative/file"), home), Err(OffloadBlock::NotAbsolute));
    assert_eq!(classify_movable(Path::new("/Users/<user>/Movies/../Library"), home), Err(OffloadBlock::NotNormalized));
    assert_eq!(classify_movable(Path::new("/Volumes/<external>/file"), home), Err(OffloadBlock::AlreadyExternal));
    assert_eq!(classify_movable(Path::new("/System/Library/file"), home), Err(OffloadBlock::OutsideHome));
}
```

- [ ] **Step 2: Run the policy tests and confirm RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests::movable_policy -- --nocapture`

Expected: compilation fails because `classify_movable` and `OffloadBlock` do not exist.

- [ ] **Step 3: Add the structured lexical policy**

Implement this public shape above `check_movable`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffloadBlock {
    NotAbsolute,
    NotNormalized,
    AlreadyExternal,
    OutsideHome,
    HomeRoot,
    HiddenRoot,
    ProtectedRoot,
    CloudSyncRoot,
    ManagedBundle,
    Missing,
    Symlink,
    SymlinkAncestor,
}

impl OffloadBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAbsolute => "Only absolute paths can be offloaded.",
            Self::NotNormalized => "This path must be normalized before it can be offloaded.",
            Self::AlreadyExternal => "This item is already on an external volume.",
            Self::OutsideHome => "Only items inside your home folder can be offloaded.",
            Self::HomeRoot => "Your entire home folder cannot be offloaded.",
            Self::HiddenRoot => "Hidden home-folder data stays on this Mac.",
            Self::ProtectedRoot => "App-managed home data stays on this Mac.",
            Self::CloudSyncRoot => "Cloud-synced folders cannot be offloaded safely.",
            Self::ManagedBundle => "Application and managed-library bundles cannot be offloaded safely.",
            Self::Missing => "This item is no longer available.",
            Self::Symlink => "Symlinks cannot be offloaded.",
            Self::SymlinkAncestor => "Items reached through a symlink cannot be offloaded.",
        }
    }
}
```

Implement `classify_movable` by rejecting non-absolute or dot-component paths, `/Volumes`, paths outside `home`, the home root, hidden first components, case-insensitive protected first components (`Library`, `Applications`, `Public`, `Trash`), cloud-root prefixes (`dropbox`, `onedrive`, `google drive`, `icloud drive (archive`), and any case-insensitive component suffix in `.app`, `.photoslibrary`, `.musiclibrary`, `.imovielibrary`, `.fcpbundle`.

- [ ] **Step 4: Run policy tests and confirm GREEN**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests::movable_policy -- --nocapture`

Expected: all four policy tests pass.

- [ ] **Step 5: Add failing filesystem policy tests**

Add these tests:

```rust
#[test]
fn movable_filesystem_accepts_a_regular_home_file_and_rejects_a_source_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let regular = home.join("Movies/clip.mov");
    let link = home.join("Movies/link.mov");
    fs::create_dir_all(regular.parent().unwrap()).unwrap();
    fs::write(&regular, b"clip").unwrap();
    std::os::unix::fs::symlink(&regular, &link).unwrap();
    assert_eq!(check_movable(&regular, &home), Ok(()));
    assert_eq!(check_movable(&link, &home), Err(OffloadBlock::Symlink));
}

#[test]
fn movable_filesystem_rejects_a_symlinked_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let real = tmp.path().join("real");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("clip.mov"), b"clip").unwrap();
    std::os::unix::fs::symlink(&real, home.join("Movies")).unwrap();
    assert_eq!(
        check_movable(&home.join("Movies/clip.mov"), &home),
        Err(OffloadBlock::SymlinkAncestor)
    );
}
```

- [ ] **Step 6: Run filesystem policy tests and confirm RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests::movable_filesystem -- --nocapture`

Expected: the new tests fail because `check_movable` does not inspect metadata or ancestors.

- [ ] **Step 7: Implement full filesystem eligibility**

Make `check_movable` return `Result<(), OffloadBlock>`. Call `classify_movable` first, then walk each home-relative component from `home` to `src` with `symlink_metadata`. Return `Missing` for an unreadable component, `Symlink` when the final component is a symlink, and `SymlinkAncestor` for an earlier symlink.

- [ ] **Step 8: Run Task 1 tests and the complete offload module**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests -- --nocapture`

Expected: all offload tests pass after existing broad tests are updated to use `classify_movable` or real temp fixtures.

- [ ] **Step 9: Commit the policy boundary**

```bash
git add src/offload.rs
git commit -m "Harden SSD source eligibility"
```

---

### Task 2: Destination, source-identity, target, and worker preflight

**Files:**
- Modify: `src/offload.rs:120-295`
- Modify: `src/app.rs:2120-2150`
- Test: inline `src/offload.rs` test module

**Interfaces:**
- Consumes: Task 1 `check_movable`, `OffloadBlock::message`, existing `eligible_volume`, `statfs_info`, `has_room`, `perform_move`, and `OffloadJob`.
- Produces: private `SourceIdentity`, `source_identity`, `ensure_source_unchanged`, `check_destination_absent`, `check_target`, plus `OffloadJob { home: PathBuf, .. }`.

- [ ] **Step 1: Write failing collision and identity tests**

Add tests proving:

```rust
#[test]
fn move_refuses_an_existing_destination_without_touching_source() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src.txt");
    let dest = tmp.path().join("dest.txt");
    let ledger = tmp.path().join("ledger.json");
    fs::write(&src, b"source").unwrap();
    fs::write(&dest, b"existing").unwrap();
    let error = perform_move(&src, &dest, &ledger, false).unwrap_err();
    assert!(error.contains("destination already exists"));
    assert_eq!(fs::read(&src).unwrap(), b"source");
    assert_eq!(fs::read(&dest).unwrap(), b"existing");
}

#[test]
fn source_identity_detects_a_path_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src.txt");
    let parked = tmp.path().join("parked.txt");
    fs::write(&src, b"first").unwrap();
    let identity = source_identity(&src).unwrap();
    fs::rename(&src, &parked).unwrap();
    fs::write(&src, b"second").unwrap();
    assert!(ensure_source_unchanged(&src, identity).is_err());
}
```

- [ ] **Step 2: Run the two tests and confirm RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests -- --nocapture`

Expected: collision test fails because `ditto` merges/overwrites, and identity helpers do not compile.

- [ ] **Step 3: Add destination and identity guards**

Import `std::os::unix::fs::MetadataExt`. Add a private copyable `SourceIdentity { dev: u64, ino: u64 }`. `source_identity` must use `symlink_metadata`, reject symlinks, and record `dev()` and `ino()`. `ensure_source_unchanged` must recapture and compare. `check_destination_absent` must distinguish `NotFound` from other metadata errors and reject existing files, directories, and broken symlinks.

At the start of `perform_move`, call `check_destination_absent(dest)` and capture the identity. After destination-size verification and immediately before `delete_path(src)`, call `ensure_source_unchanged(src, identity)`.

- [ ] **Step 4: Run collision, identity, and move tests and confirm GREEN**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests -- --nocapture`

Expected: collision and identity tests pass; clean move and symlink move remain green.

- [ ] **Step 5: Write failing worker-preflight tests**

Replace the old async-success test with this protected-source test; the new
`home` field intentionally makes the first run fail to compile:

```rust
#[test]
fn run_offload_rejects_a_protected_source_before_copying() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let src = home.join("Library/Caches/data.bin");
    let mount = tmp.path().join("target");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(&mount).unwrap();
    fs::write(&src, b"keep me").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    run_offload(
        OffloadJob {
            src: src.clone(),
            mount_path: mount,
            leave_symlink: false,
            home,
        },
        tx,
    );
    assert!(matches!(
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
        OffloadEvent::Failed { .. }
    ));
    assert_eq!(fs::read(src).unwrap(), b"keep me");
}
```

Add a second test using `home/Movies/data.bin` with the same temporary target.
Assert `Failed` and an intact source because the target is outside `/Volumes`.

- [ ] **Step 6: Run worker tests and confirm RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test offload::tests::run_offload -- --nocapture`

Expected: compilation fails because `OffloadJob` has no `home`, then behavior fails until worker preflight exists.

- [ ] **Step 7: Implement target and worker preflight**

Add `home: PathBuf` to `OffloadJob`. Add `check_target(mount_path, item_size)` that requires a normalized path below `/Volumes`, calls `statfs_info`, reuses `eligible_volume`, and rechecks the 100 MB free-space margin.

Inside the worker, before emitting `Started` or invoking `perform_move`:

1. call `check_movable(&job.src, &job.home)` and map the block to its message;
2. calculate the current source size;
3. call `check_target(&job.mount_path, total)`;
4. calculate `dest` and call `check_destination_absent(&dest)`.

Any failure emits one `OffloadEvent::Failed` and returns without copying.

- [ ] **Step 8: Pass the current home from the UI**

When constructing `OffloadJob` in `app.rs`, set:

```rust
home: std::env::var_os("HOME")
    .map(std::path::PathBuf::from)
    .unwrap_or_default(),
```

- [ ] **Step 9: Run Task 2 verification**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check && PATH="$HOME/.cargo/bin:$PATH" cargo test --locked`

Expected: formatting is clean and the full Rust suite passes.

- [ ] **Step 10: Commit mutation-boundary enforcement**

```bash
git add src/offload.rs src/app.rs
git commit -m "Revalidate SSD moves at execution"
```

---

### Task 3: Disable protected SSD actions in both UI surfaces

**Files:**
- Modify: `src/app.rs:1-15,430-620,360-390,1280-1365,1670-1820`
- Test: inline `src/app.rs` test module

**Interfaces:**
- Consumes: Task 1 `classify_movable`, `check_movable`, and `OffloadBlock::message`.
- Produces: `map_item_actions(..., offload_allowed: bool)`, disabled-hover reasons in the map context menu and recommendation row, and a full dialog fallback reason.

- [ ] **Step 1: Write a failing map-action policy test**

Extend `map_actions_match_real_synthetic_and_denied_items` with this assertion,
then add the `offload_allowed` argument to every existing call:

```rust
assert_eq!(
    map_item_actions(true, false, false, true, false),
    MapItemActions {
        open: true,
        reveal: true,
        move_to_ssd: false,
    }
);
```

- [ ] **Step 2: Run the app policy test and confirm RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::map_actions_match -- --nocapture`

Expected: compilation fails because `map_item_actions` has no `offload_allowed` argument.

- [ ] **Step 3: Implement inexpensive UI eligibility**

Import `classify_movable`. Extend `map_item_actions` with `offload_allowed` and set `move_to_ssd: real && offload_allowed`.

In `draw_map`, read `$HOME` once before the item loop. For each real node, strip the data root and call `classify_movable`; keep the returned `OffloadBlock` as `offload_block`. Pass `offload_block.is_none()` to `map_item_actions`.

Build the Move response in a variable. If blocked, apply `response.on_disabled_hover_text(block.message())`; otherwise preserve the current click dispatch. This path performs no metadata reads per frame.

- [ ] **Step 4: Add the same disabled state to recommendation rows**

In `rec_card`, derive the pure lexical decision for `rec_real`. Replace the always-clickable `→ SSD` label with `ui.add_enabled(offload_block.is_none(), Label::new(...).sense(Sense::click()))`, attach `on_disabled_hover_text`, and only populate `offload_out` on an enabled click.

- [ ] **Step 5: Keep full dialog fallback validation**

Change `open_offload_dialog` to map the new enum:

```rust
let reason = check_movable(&src, &home)
    .err()
    .map(|block| block.message().to_owned());
```

- [ ] **Step 6: Run Task 3 verification**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check && PATH="$HOME/.cargo/bin:$PATH" cargo test --locked`

Expected: all tests pass, protected paths keep Open/Reveal where appropriate, and only Move to SSD is disabled.

- [ ] **Step 7: Commit the visible safety state**

```bash
git add src/app.rs
git commit -m "Explain protected SSD actions"
```

---

### Task 4: Publish contributor UI automation

**Files:**
- Create: `scripts/ui-smoke.applescript`
- Create: `scripts/right-click.swift`
- Create: `scripts/test-ui-smoke.sh`
- Create: `scripts/test-signed-ui.sh`
- Modify: `CONTRIBUTING.md`
- Modify: `.github/workflows/ci.yml`
- Modify: `scripts/test-community-files.sh`

**Interfaces:**
- Consumes: installed signed `/Applications/DiskDeck.app`, macOS Accessibility, System Events, CoreGraphics.
- Produces: `osascript scripts/ui-smoke.applescript check|signature|tile-center|menu-visible|escape|back`, static script verification, and a non-destructive signed UI runner.

- [ ] **Step 1: Write the failing static script test**

Create executable `scripts/test-ui-smoke.sh` that requires the other three files, compiles AppleScript with `osacompile` into a temporary output, typechecks Swift with `swiftc -typecheck`, runs `sh -n scripts/test-signed-ui.sh`, and rejects `click` statements whose target contains `Hold to reclaim`, `Review targets`, `Move to SSD`, or `Reveal in Finder`. Merely inspecting those fixed labels remains allowed.

- [ ] **Step 2: Run the static script test and confirm RED**

Run: `scripts/test-ui-smoke.sh`

Expected: failure reports missing `scripts/ui-smoke.applescript`.

- [ ] **Step 3: Implement the AppleScript command surface**

`ui-smoke.applescript` must:

- activate DiskDeck and require `UI elements enabled`;
- require window 1, group 1, the `DiskDeck` label, and the named `← Back` button;
- `signature`: return the ordered breadcrumb labels without printing filesystem paths in normal `check` output;
- `tile-center`: choose the largest `AXUnknown` element at least 200 × 200 points and return its center as `x y`;
- `menu-visible`: return `true` only when the accessibility names include Open,
  Reveal in Finder, and Move to SSD; it never clicks those elements;
- `escape`: send key code 53;
- `back`: click only the named Back button and only when a slash breadcrumb separator is present;
- `check`: verify controls and report `PASS: signed UI controls available` without clicking map items or actions.

Unknown commands exit non-zero with usage copy.

- [ ] **Step 4: Implement the right-click helper**

`right-click.swift` must parse exactly two finite coordinates, call `CGPreflightPostEventAccess()`, emit one `.rightMouseDown` and one `.rightMouseUp` through `.cghidEventTap`, and exit non-zero with Accessibility instructions when posting is unavailable.

- [ ] **Step 5: Implement the live signed UI runner**

`test-signed-ui.sh` must:

1. require `/Applications/DiskDeck.app` and its executable;
2. run AppleScript `check`;
3. capture `signature` and `tile-center`;
4. invoke `swift scripts/right-click.swift x y`;
5. wait no more than two seconds for fixed menu labels to appear through the accessibility tree;
6. call AppleScript `escape`;
7. assert the post-Escape signature equals the pre-click signature;
8. call AppleScript `back` only when nested and report that navigation-only check separately.

It must never click a context-menu entry or cleanup control.

- [ ] **Step 6: Run the static script test and confirm GREEN**

Run: `scripts/test-ui-smoke.sh`

Expected: AppleScript compiles, Swift typechecks, shell parses, and the destructive-click scan passes.

- [ ] **Step 7: Document and wire the contributor check**

Add a **Signed UI smoke check** section to `CONTRIBUTING.md` explaining the one-time Accessibility grant, `./make-app.sh`, `scripts/test-ui-smoke.sh`, and `scripts/test-signed-ui.sh`. State exactly that the live runner opens/dismisses a context menu and may navigate Back, but never selects a cleanup or SSD action.

Add `scripts/test-ui-smoke.sh` as a CI step after formatting. Add its filename to the expected CI strings in `scripts/test-community-files.sh`.

- [ ] **Step 8: Run documentation and automation verification**

Run: `scripts/test-ui-smoke.sh && scripts/test-community-files.sh && scripts/test-pre-commit.sh`

Expected: all three suites pass.

- [ ] **Step 9: Commit contributor automation**

```bash
git add scripts/ui-smoke.applescript scripts/right-click.swift scripts/test-ui-smoke.sh scripts/test-signed-ui.sh CONTRIBUTING.md .github/workflows/ci.yml scripts/test-community-files.sh
git commit -m "Publish signed UI smoke tooling"
```

---

### Task 5: Signed-app verification and publication

**Files:**
- Verify: `src/offload.rs`, `src/app.rs`, scripts, docs, CI
- Verify: `/Applications/DiskDeck.app`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: a locally and remotely verified safety-foundation slice on `main`.

- [ ] **Step 1: Run the complete local gate**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
```

Expected: every command exits zero; the Rust report has zero failed tests.

- [ ] **Step 2: Build and verify the signed application**

```bash
PATH="$HOME/.cargo/bin:$PATH" ./make-app.sh
codesign --verify --strict --verbose=2 /Applications/DiskDeck.app
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/DiskDeck.app/Contents/Info.plist
```

Expected: release build succeeds, the signature is valid, and the identifier is exactly `com.buddyhq.headroom-rs`.

- [ ] **Step 3: Run the live non-destructive smoke check**

Run: `scripts/test-signed-ui.sh`

Expected: context menu appears, Escape preserves the breadcrumb signature, Back navigation passes when nested, and no filesystem action is selected.

- [ ] **Step 4: Inspect the signed UI manually**

In the signed app, verify a protected path shows disabled Move to SSD with refusal copy, an eligible normal user folder still opens the existing dialog, and cancel the dialog before the hold. Do not run an offload against real data.

- [ ] **Step 5: Commit any verification-only documentation correction**

If live verification required a documentation-only correction, stage only that file and use `git commit -m "Clarify SSD safety verification"`. If no correction was required, create no empty commit.

- [ ] **Step 6: Push and prove the remote**

```bash
git push origin main
test "$(git rev-parse HEAD)" = "$(git ls-remote origin refs/heads/main | awk '{print $1}')"
git status --short --branch
```

Expected: the pre-push identity guard passes, remote and local SHAs match, and the worktree is clean.
