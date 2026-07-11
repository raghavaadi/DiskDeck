# Moved Items and Restore Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every verified SSD offload a durable local record, show moved-item health in the app, and restore one item to the Mac without removing the external copy until the internal copy is installed and verified.

**Architecture:** `transfer.rs` owns shared copy/identity primitives; `offload.rs` keeps outbound policy; new `moves.rs` owns records, reconciliation, restore policy, and the restore worker. `app.rs` presents one rail state at a time and talks to move/restore workers through channels.

**Tech Stack:** Rust standard library, egui/eframe 0.29, libc filesystem statistics, `/usr/bin/ditto`, existing mpsc worker pattern, tempfile for fixture tests.

## Global Constraints

- Scan root remains `/System/Volumes/Data`.
- Nothing is deleted without an explicit selection or acknowledgement plus the 900 ms hold.
- Restore must copy, verify, atomically install, recheck target identity, and only then remove the external target.
- The origin may be absent or the exact DiskDeck-created symlink; any other occupant blocks restore.
- Paths remain local and lossless; no telemetry, account, cloud service, or AI advisor.
- No new crate dependency.
- Tests use only temporary fixture directories; never restore, delete, or run commands on owner data.
- `CFBundleIdentifier` remains `com.buddyhq.headroom-rs`.

---

### Task 1: Extract shared verified-transfer primitives

**Files:**
- Create: `src/transfer.rs`
- Modify: `src/main.rs`
- Modify: `src/offload.rs`
- Test: `src/transfer.rs`

**Interfaces:**
- Produces: `PathIdentity`, `path_identity`, `ensure_same_identity`, `ensure_absent`, `apparent_size`, and `verified_ditto_copy` for offload and restore.
- Preserves: existing `perform_move` behavior and all offload tests.

- [ ] **Step 1: Write failing transfer tests**

Create `src/transfer.rs` with the fixture tests below and add `mod transfer;` to
`src/main.rs` so the missing production interfaces fail compilation:

```rust
#[test]
fn identity_recheck_detects_path_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("item");
    std::fs::write(&path, b"first").unwrap();
    let identity = path_identity(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"second").unwrap();
    assert!(ensure_same_identity(&path, identity).is_err());
}

#[test]
fn verified_copy_preserves_source_and_matches_apparent_size() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dest = tmp.path().join("nested/dest");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("data"), b"payload").unwrap();
    let copied = verified_ditto_copy(&src, &dest).unwrap();
    assert_eq!(copied, apparent_size(&src));
    assert!(src.exists());
    assert_eq!(std::fs::read(dest.join("data")).unwrap(), b"payload");
}
```

- [ ] **Step 2: Run the focused tests and verify red**

Run: `cargo test transfer::tests --locked`

Expected: compile failure because the transfer interfaces do not exist.

- [ ] **Step 3: Implement the shared primitives**

Create `src/transfer.rs` with these exact signatures:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PathIdentity { pub dev: u64, pub ino: u64 }

pub(crate) fn path_identity(path: &Path) -> Result<PathIdentity, String>;
pub(crate) fn ensure_same_identity(path: &Path, expected: PathIdentity) -> Result<(), String>;
pub(crate) fn ensure_absent(path: &Path, label: &str) -> Result<(), String>;
pub(crate) fn apparent_size(path: &Path) -> i64;
pub(crate) fn verified_ditto_copy(src: &Path, dest: &Path) -> Result<i64, String>;
```

`path_identity` must reject symlinks. `verified_ditto_copy` must collision-check
the destination, create only its parent, call `/usr/bin/ditto`, compare apparent
sizes, and return the source apparent size. It never removes either path.

- [ ] **Step 4: Refactor offload to use the shared primitives**

Remove the duplicated identity, collision, apparent-size, and copy/verify
implementations from `offload.rs`; import the new functions. Keep
`perform_move` ordering unchanged:

```rust
ensure_absent(dest, "destination")?;
let source_identity = path_identity(src)?;
let total = quick_du(src);
verified_ditto_copy(src, dest)?;
ensure_same_identity(src, source_identity)?;
delete_path(src)?;
```

- [ ] **Step 5: Verify and commit**

Run: `cargo test transfer::tests --locked`

Run: `cargo test offload::tests --locked`

Expected: transfer and all existing offload tests pass.

Commit: `git add src/main.rs src/offload.rs src/transfer.rs && git commit -m "Share verified transfer primitives"`

---

### Task 2: Add the lossless local move registry

**Files:**
- Create: `src/moves.rs`
- Modify: `src/main.rs`
- Test: `src/moves.rs`

**Interfaces:**
- Produces: `MoveRecord`, `registry_path_for_home`, `load_records`, `upsert_record`, and `mark_restored`.
- Consumes: raw Unix path bytes via `OsStrExt` and `OsStringExt`.

- [ ] **Step 1: Write codec and atomic-storage tests**

Create `src/moves.rs` with the tests below and add `mod moves;` to `src/main.rs`:

```rust
#[test]
fn registry_round_trips_raw_paths_and_restore_state() {
    let origin = PathBuf::from(OsString::from_vec(b"/Users/<user>/clip-\xff".to_vec()));
    let record = MoveRecord {
        origin,
        dest: PathBuf::from("/Volumes/<external>/DiskDeck Offload/Users/<user>/clip"),
        moved_at: 42,
        bytes: 7,
        symlinked: true,
        restored_at: Some(84),
    };
    assert_eq!(decode_registry(&encode_registry(&[record.clone()]).unwrap()).unwrap(), vec![record]);
}

#[test]
fn upsert_is_atomic_deduplicated_and_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.ddmoves");
    for i in 0..520 {
        upsert_record(&path, MoveRecord {
            origin: PathBuf::from(format!("/Users/<user>/item-{i}")),
            dest: PathBuf::from(format!("/Volumes/<external>/DiskDeck Offload/item-{i}")),
            moved_at: i,
            bytes: i,
            symlinked: false,
            restored_at: None,
        }).unwrap();
    }
    upsert_record(&path, MoveRecord {
        origin: PathBuf::from("/Users/<user>/item-519"),
        dest: PathBuf::from("/Volumes/<external>/DiskDeck Offload/item-519"),
        moved_at: 999,
        bytes: 123,
        symlinked: true,
        restored_at: None,
    }).unwrap();
    let records = load_records(&path).unwrap();
    assert_eq!(records.len(), MAX_RECORDS);
    assert_eq!(records[0].moved_at, 999);
    assert_eq!(records[0].bytes, 123);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
}
```

Also cover wrong magic, truncation, trailing bytes, a path length above 1 MiB,
and a count above 4,096.

- [ ] **Step 2: Run focused tests and verify red**

Run: `cargo test moves::tests::registry --locked`

Expected: compile failure because the registry interfaces do not exist.

- [ ] **Step 3: Implement the record and codec**

```rust
const MAGIC: &[u8; 8] = b"DDMOVE1\0";
const MAX_RECORDS: usize = 512;
const MAX_PATH_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRecord {
    pub origin: PathBuf,
    pub dest: PathBuf,
    pub moved_at: i64,
    pub bytes: i64,
    pub symlinked: bool,
    pub restored_at: Option<i64>,
}

pub fn registry_path_for_home(home: &Path) -> PathBuf;
fn encode_registry(records: &[MoveRecord]) -> Result<Vec<u8>, String>;
fn decode_registry(bytes: &[u8]) -> Result<Vec<MoveRecord>, String>;
pub fn load_records(path: &Path) -> Result<Vec<MoveRecord>, String>;
pub fn upsert_record(path: &Path, record: MoveRecord) -> Result<(), String>;
pub fn mark_restored(path: &Path, record: &MoveRecord, restored_at: i64) -> Result<(), String>;
```

Use little-endian fixed-width integers and raw path bytes. `upsert_record`
loads the existing registry, refuses to overwrite corrupt content, replaces a
matching `(origin, dest)` record, sorts newest first, truncates to 512, writes a
unique sibling, calls `sync_all`, and renames atomically.

- [ ] **Step 4: Verify and commit**

Run: `cargo test moves::tests::registry --locked`

Expected: all registry tests pass.

Commit: `git add src/main.rs src/moves.rs && git commit -m "Persist lossless move records"`

---

### Task 3: Reconcile ledgers and classify move health

**Files:**
- Modify: `src/moves.rs`
- Test: `src/moves.rs`

**Interfaces:**
- Produces: `MoveState`, `MovedItem`, `refresh_records`, and `state_reason`.
- Consumes: local records plus legacy `.diskdeck-offload.json` files on attached volumes.

- [ ] **Step 1: Write state and legacy-import tests**

Cover the exact state table with fixture `home` and `volumes_root` directories:

```rust
#[test]
fn exact_origin_symlink_is_ready_but_a_different_link_blocks_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let volumes = tmp.path().join("Volumes");
    let origin = home.join("Movies/clip.mov");
    let dest = volumes.join("<external>/DiskDeck Offload/Users/<user>/Movies/clip.mov");
    std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"clip").unwrap();
    std::os::unix::fs::symlink(&dest, &origin).unwrap();
    let record = MoveRecord::new(origin.clone(), dest.clone(), 42, 4, true);
    assert_eq!(inspect_record(&record, &home, &volumes), MoveState::Ready);
    std::fs::remove_file(&origin).unwrap();
    std::os::unix::fs::symlink(dest.with_file_name("other.mov"), &origin).unwrap();
    assert_eq!(inspect_record(&record, &home, &volumes), MoveState::OriginChanged);
}

#[test]
fn detached_drive_is_not_misreported_as_a_missing_target() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let volumes = tmp.path().join("Volumes");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&volumes).unwrap();
    let record = MoveRecord::new(
        home.join("Movies/clip.mov"),
        volumes.join("<external>/DiskDeck Offload/Users/<user>/Movies/clip.mov"),
        42, 4, false,
    );
    assert_eq!(inspect_record(&record, &home, &volumes), MoveState::DriveDisconnected);
}

#[test]
fn legacy_import_accepts_only_normalized_paths_under_diskdeck_offload() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let volumes = tmp.path().join("Volumes");
    let drive = volumes.join("SSD");
    let offload = drive.join("DiskDeck Offload");
    std::fs::create_dir_all(&offload).unwrap();
    let valid_origin = home.join("Movies/clip.mov");
    let valid_dest = offload.join("Users/test/Movies/clip.mov");
    let ledger = format!(
        "{{\"origin\":\"{}\",\"dest\":\"{}\",\"moved_at\":42,\"symlinked\":false}}\n\
         {{\"origin\":\"{}\",\"dest\":\"{}\",\"moved_at\":43,\"symlinked\":false}}\n",
        valid_origin.display(), valid_dest.display(),
        valid_origin.display(), offload.join("../escape").display(),
    );
    std::fs::write(offload.join(".diskdeck-offload.json"), ledger).unwrap();
    let registry = tmp.path().join("index.ddmoves");
    let items = refresh_records(&registry, &home, &volumes).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].record.dest, valid_dest);
}
```

- [ ] **Step 2: Run focused tests and verify red**

Run: `cargo test moves::tests --locked`

Expected: compile failure for missing state/reconciliation interfaces.

- [ ] **Step 3: Implement classification and reconciliation**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveState { Ready, DriveDisconnected, OriginChanged, TargetMissing, Restored }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedItem { pub record: MoveRecord, pub state: MoveState }

impl MoveRecord {
    pub fn new(origin: PathBuf, dest: PathBuf, moved_at: i64, bytes: i64, symlinked: bool) -> Self;
}

pub fn inspect_record(record: &MoveRecord, home: &Path, volumes_root: &Path) -> MoveState;
pub fn state_reason(state: MoveState) -> &'static str;
pub fn refresh_records(registry: &Path, home: &Path, volumes_root: &Path)
    -> Result<Vec<MovedItem>, String>;
```

Resolve `Restored` only from `restored_at`. Check the volume root before the
destination so an unplugged drive is `DriveDisconnected`. A present origin is
ready only when it is a symlink whose `read_link` result exactly equals `dest`.
The legacy parser must decode only the JSON string/number/bool fields emitted by
DiskDeck, skip malformed lines, validate both paths, deduplicate against local
records, and persist accepted imports atomically.

- [ ] **Step 4: Verify and commit**

Run: `cargo test moves::tests --locked`

Commit: `git add src/moves.rs && git commit -m "Classify moved item health"`

---

### Task 4: Implement the verified restore worker

**Files:**
- Modify: `src/moves.rs`
- Test: `src/moves.rs`

**Interfaces:**
- Consumes: Task 1 transfer primitives and Task 2 records.
- Produces: `RestoreRoots`, `RestoreBlock`, `RestoreJob`, `RestoreEvent`, `can_confirm_restore`, and `run_restore`.

- [ ] **Step 1: Write preflight and mutation-boundary tests**

```rust
#[test]
fn restore_refuses_an_occupied_origin_without_touching_either_copy() {
    let fixture = RestoreFixture::new(false);
    std::fs::write(&fixture.origin, b"new local file").unwrap();
    let error = preflight_restore(&fixture.record, &fixture.roots).unwrap_err();
    assert_eq!(error, RestoreBlock::OriginChanged);
    assert_eq!(std::fs::read(&fixture.origin).unwrap(), b"new local file");
    assert_eq!(std::fs::read(&fixture.dest).unwrap(), b"external copy");
}

#[test]
fn restore_replaces_only_the_exact_origin_link_after_verified_copy() {
    let fixture = RestoreFixture::new(true);
    let outcome = perform_restore(&fixture.record, &fixture.registry, &fixture.roots).unwrap();
    assert_eq!(outcome.restored, b"external copy".len() as i64);
    assert!(!std::fs::symlink_metadata(&fixture.origin).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&fixture.origin).unwrap(), b"external copy");
    assert!(!fixture.dest.exists());
}

#[test]
fn restore_detects_destination_replacement_before_external_delete() {
    let fixture = RestoreFixture::new(false);
    std::fs::write(&fixture.origin, b"installed copy").unwrap();
    let identity = path_identity(&fixture.dest).unwrap();
    std::fs::remove_file(&fixture.dest).unwrap();
    std::fs::write(&fixture.dest, b"replacement").unwrap();
    assert!(remove_verified_target(&fixture.dest, identity).is_err());
    assert_eq!(std::fs::read(&fixture.dest).unwrap(), b"replacement");
    assert_eq!(std::fs::read(&fixture.origin).unwrap(), b"installed copy");
}

#[test]
fn failed_install_rename_restores_the_backed_up_symlink() {
    let fixture = RestoreFixture::new(true);
    let missing_staging = fixture.origin.with_extension("missing-stage");
    let backup = fixture.origin.with_extension("diskdeck-link-backup");
    assert!(install_staged_origin(&fixture.origin, &missing_staging, &backup, &fixture.dest).is_err());
    assert_eq!(std::fs::read_link(&fixture.origin).unwrap(), fixture.dest);
    assert!(!backup.exists());
}
```

Define `RestoreFixture::new(exact_link: bool)` inside the test module. It creates
temporary `home`, `Volumes/SSD/DiskDeck Offload`, registry, origin, destination,
record, and roots paths; writes `b"external copy"` to the destination; and adds
the exact origin symlink only when requested. The fixture never references the
real home or `/Volumes`.

- [ ] **Step 2: Run focused tests and verify red**

Run: `cargo test moves::tests::restore --locked`

Expected: compile failure for missing restore interfaces.

- [ ] **Step 3: Implement preflight and worker types**

```rust
#[derive(Clone, Debug)]
pub struct RestoreRoots { pub home: PathBuf, pub volumes: PathBuf }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreBlock {
    UnsafeRecord, DriveDisconnected, TargetMissing, TargetSymlink,
    OriginChanged, SymlinkAncestor, NotEnoughSpace, StagingCollision,
}

pub struct RestoreJob {
    pub record: MoveRecord,
    pub registry_path: PathBuf,
    pub roots: RestoreRoots,
}

#[derive(Debug)]
struct RestorePlan {
    staging: PathBuf,
    backup: PathBuf,
    target_identity: PathIdentity,
    total: i64,
}

pub struct RestoreOutcome {
    pub restored: i64,
    pub origin: PathBuf,
    pub registry_warning: Option<String>,
}

pub enum RestoreEvent {
    Started { name: String, total: i64 },
    Done { restored: i64, origin: PathBuf, registry_warning: Option<String> },
    Failed { error: String },
}

impl RestoreBlock { pub fn message(self) -> &'static str; }
pub fn can_confirm_restore(acknowledged: bool, block: Option<RestoreBlock>) -> bool;
pub fn restore_block(record: &MoveRecord, roots: &RestoreRoots) -> Option<RestoreBlock>;
fn preflight_restore(record: &MoveRecord, roots: &RestoreRoots) -> Result<RestorePlan, RestoreBlock>;
fn install_staged_origin(origin: &Path, staging: &Path, backup: &Path, expected_dest: &Path) -> Result<(), String>;
fn remove_verified_target(dest: &Path, expected: PathIdentity) -> Result<(), String>;
fn perform_restore(record: &MoveRecord, registry: &Path, roots: &RestoreRoots) -> Result<RestoreOutcome, String>;
pub fn run_restore(job: RestoreJob, tx: Sender<RestoreEvent>) -> Result<(), String>;
```

The worker executes the nine-step safety contract from the design. Generate
staging and link-backup siblings from the current PID and a collision counter.
After staging verification, recheck destination identity, rename the exact
origin link to backup, rename staging into origin, restore the backup on install
failure, verify origin, recheck destination again, then call `delete_path` on
the external destination. Mark the record restored; return registry failure as
a warning in `Done`, not a false move failure.

- [ ] **Step 4: Verify and commit**

Run: `cargo test moves::tests::restore --locked`

Expected: all fixture restore tests pass and never access `/Volumes` or the real home.

Commit: `git add src/moves.rs && git commit -m "Restore moved items safely"`

---

### Task 5: Persist every future offload locally

**Files:**
- Modify: `src/offload.rs`
- Modify: `src/app.rs`
- Test: `src/offload.rs`

**Interfaces:**
- Consumes: `MoveRecord`, `registry_path_for_home`, and `upsert_record`.
- Extends: `MoveOutcome` with `moved_at`; `OffloadEvent::Done` with `registry_warning`.

- [ ] **Step 1: Write a failing offload-registry test**

```rust
#[test]
fn successful_offload_persists_the_exact_local_record() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let src = home.join("Movies/clip.mov");
    let dest = tmp.path().join("volume/DiskDeck Offload/clip.mov");
    let external_ledger = tmp.path().join("volume/DiskDeck Offload/.diskdeck-offload.json");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, b"payload").unwrap();
    let outcome = perform_move(&src, &dest, &external_ledger, true).unwrap();
    let registry_warning = persist_move_record(&home, &src, &outcome);
    assert!(registry_warning.is_none());
    let records = load_records(&registry_path_for_home(&home)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].origin, src);
    assert_eq!(records[0].dest, dest);
    assert_eq!(records[0].moved_at, outcome.moved_at);
    assert!(records[0].symlinked);
    assert_eq!(records[0].restored_at, None);
}
```

- [ ] **Step 2: Run the test and verify red**

Run: `cargo test offload::tests::successful_offload_persists --locked`

- [ ] **Step 3: Persist after the verified outbound move**

Generate `moved_at` once in `perform_move`, pass it to the external ledger, and
return it in `MoveOutcome`. In `run_offload`, build the exact `MoveRecord` and
call `upsert_record(registry_path_for_home(&job.home), record)`. Extend the done
event:

```rust
fn persist_move_record(home: &Path, origin: &Path, outcome: &MoveOutcome) -> Option<String>;
```

```rust
Done {
    reclaimed: i64,
    dest: PathBuf,
    symlinked: bool,
    registry_warning: Option<String>,
}
```

Update `poll_offload` to log an amber warning without changing the successful
move result.

- [ ] **Step 4: Verify and commit**

Run: `cargo test offload::tests --locked`

Run: `cargo test moves::tests --locked`

Commit: `git add src/offload.rs src/app.rs && git commit -m "Record verified SSD moves locally"`

---

### Task 6: Add the Moved items rail and restore confirmation

**Files:**
- Modify: `src/app.rs`
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-ui-smoke.sh`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: `refresh_records`, `MovedItem`, restore preflight/block messages, and restore events.
- Produces: discoverable, accessible Moved items navigation and one-item restore sheet.

- [ ] **Step 1: Write failing view-state and layout tests**

Replace `review_open: bool` with:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RailView { Summary, Reclaim, Moved }
```

Test that the app opens on `Summary`, only one rail view can be active, Escape
returns `Moved` or `Reclaim` to `Summary`, and moved rows reuse the bounded
text/utility geometry at 320- and 344-point rails.

- [ ] **Step 2: Run focused tests and verify red**

Run: `cargo test app::tests --locked`

- [ ] **Step 3: Implement refresh state and navigation**

Add `moves_rx`, `moved_items`, `moves_error`, `restore_rx`, `restoring`,
`restore_dialog`, and `restore_hold` to `App`. Start a named `move-refresh`
worker on first frame, after offload, after restore, and from the Refresh
button. Add an actual egui `Button` labeled `Moved items` to the Reclaim summary
so VoiceOver and UI smoke can discover it.

- [ ] **Step 4: Implement the Moved items rail**

Use `panel_chrome` and the existing scroll/footer pattern. Rows show name/path
left and size/status right, with `Restore to Mac…` always visible. Disable it
with `state_reason` unless state is `Ready`. Restored rows remain visible and
dimmed. Drive-disconnected and target-missing states are amber; origin collision
is red.

- [ ] **Step 5: Implement the confirmation sheet and event handling**

The sheet shows drive, origin, bytes, free capacity, acknowledgement checkbox,
Cancel, and the 900 ms `Hold to restore` control. Opening or closing it never
mutates data. On completion, update disk stats, add an activity line, refresh
moves, and keep the Moved items rail open. On failure, show the exact worker
error and keep both records visible.

- [ ] **Step 6: Extend non-destructive UI tooling**

Add a `moved-items-visible` AppleScript command that verifies the named button
and opens the view but never presses Restore, acknowledgement, or any hold
control. Extend `test-ui-smoke.sh`'s forbidden-action grep with `Restore to Mac`
and `Hold to restore`.

- [ ] **Step 7: Verify and commit**

Run: `cargo test --locked`

Run: `scripts/test-ui-smoke.sh`

Commit: `git add src/app.rs scripts && git commit -m "Add Moved items Restore Center"`

---

### Task 7: Document and prove the signed slice

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `.github/workflows/ci.yml` only if the UI-tooling command changes

**Interfaces:**
- Documents: local registry location, restore states, safety ordering, and contributor fixture limits.

- [ ] **Step 1: Update public and maintainer documentation**

Document Moved items under features and privacy. Add `moves.rs` and
`transfer.rs` to the architecture table. Add invariants forbidding origin
overwrite and external deletion before verified install. State explicitly that
Restore Center never auto-restores or bulk-restores.

- [ ] **Step 2: Run all local gates**

Run in order:

```sh
cargo fmt --all -- --check
cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
scripts/test-icon-pipeline.sh
git diff --check
```

Expected: zero failures; existing dead-code warnings may remain but no new warning.

- [ ] **Step 3: Build and verify the signed app**

Run: `./make-app.sh`

Then verify:

```sh
codesign --verify --deep --strict /Applications/DiskDeck.app
defaults read /Applications/DiskDeck.app/Contents/Info CFBundleIdentifier
scripts/test-signed-ui.sh
```

Expected bundle id: `com.buddyhq.headroom-rs`.

- [ ] **Step 4: Perform live non-destructive visual proof**

With no real restore action selected, capture Summary → Moved items with a
disconnected fixture/import state if available. Confirm no title/path/status
overlap at the 344-point rail and that disabled Restore explains why. Do not
acknowledge or hold a real restore.

- [ ] **Step 5: Commit the documentation**

Commit: `git add README.md AGENTS.md .github/workflows/ci.yml && git commit -m "Document Restore Center safety"`

- [ ] **Step 6: Merge and publish only after fresh verification**

Fast-forward the feature branch into `main`, rerun `cargo test --locked`, push
through the personal identity hook, verify `origin/main` equals local `HEAD`,
and remove the merged worktree/branch. If GitHub Actions again reports zero
steps because of the account billing lock, report it as external and do not
change code or workflow YAML.
