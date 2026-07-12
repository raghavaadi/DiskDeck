# Reclaim History and Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist bounded local cleanup receipts and let users restore an unchanged exact Trash move to its original path through a verified 900 ms confirmation flow.

**Architecture:** Add a focused `reclaim_history.rs` module containing the `DDRH1` codec, atomic receipt store, read-only state classification, and exact-rename restore worker. `clean.rs` captures the actual direct Trash destination and appends successful receipts on its existing worker. `app.rs` loads and renders receipts only on demand or after a mutation, coordinates one mutating pipeline at a time, and never converts receipt data into cleanup or command authority.

**Tech Stack:** Rust standard library, egui 0.29 / eframe, Unix `MetadataExt`, existing mpsc worker patterns, AppleScript signed UI smoke, zsh release tooling.

## Global Constraints

- `CFBundleIdentifier` remains exactly `com.buddyhq.headroom-rs` and the signing identity default remains unchanged.
- Scan remains read-only and rooted at `/System/Volumes/Data`.
- Cleanup still requires a checked recommendation plus the existing 900 ms hold.
- Trash restore requires a separate acknowledgement plus a 900 ms hold.
- Restore never overwrites, deletes, copies, empties Trash, invokes Finder, escalates privileges, or accepts a text-entered path.
- Command receipts never become command authority; `clean::run_clean` still executes only the vetted command stored on the live `Rec`.
- Receipt and restore I/O runs off the egui thread. The menu-bar loop never loads history or probes receipt paths.
- `DDRH1` stores raw macOS path bytes, retains at most 200 receipts, and refuses to overwrite corrupt history.
- No new crate dependency.
- Any mutating fixture test operates only under a newly created temporary directory. Never test against a real recommendation or user item.

---

## Task 1: Versioned Receipt Model and Atomic Store

**Files:**
- Create: `src/reclaim_history.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces: `history_path_for_home(&Path) -> PathBuf`
- Produces: `FileKind`, `FileIdentity`, `TrashEvidence`, `ReceiptAction`, and `Receipt`
- Produces: `load_receipts(&Path) -> Result<Vec<Receipt>, String>`
- Produces: `append_receipt(&Path, Receipt) -> Result<(), String>`
- Produces: `mark_restored(&Path, u128, i64) -> Result<(), String>`
- Produces: `new_event_id() -> u128` and `now_ms() -> i64`

- [ ] **Step 1: Register the module and write failing codec/store tests**

Add `mod reclaim_history;` in `src/main.rs`. In `src/reclaim_history.rs`, first define tests for:

```rust
#[test]
fn ddrh1_round_trips_raw_paths_and_all_actions() {
    use std::os::unix::ffi::OsStringExt;
    let raw = std::ffi::OsString::from_vec(b"/tmp/cache-\xff".to_vec());
    let receipts = vec![fixture_receipt(PathBuf::from(raw), ReceiptAction::Trash)];
    assert_eq!(decode(&encode(&receipts).unwrap()).unwrap(), receipts);
}

#[test]
fn codec_rejects_wrong_truncated_invalid_and_trailing_payloads() {
    let bytes = encode(&[fixture_receipt(PathBuf::from("/tmp/a"), ReceiptAction::Delete)]).unwrap();
    assert!(decode(b"NOPE1").is_err());
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode(&trailing).is_err());
}

#[test]
fn append_is_bounded_atomic_and_refuses_corrupt_overwrite() {
    let fixture = TempFixture::new();
    for index in 0..205 {
        append_receipt(&fixture.path, fixture_receipt_with_id(index)).unwrap();
    }
    let stored = load_receipts(&fixture.path).unwrap();
    assert_eq!(stored.len(), 200);
    assert_eq!(stored.first().unwrap().event_id, 5);
    std::fs::write(&fixture.path, b"corrupt").unwrap();
    assert!(append_receipt(&fixture.path, fixture_receipt_with_id(999)).is_err());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"corrupt");
}

#[test]
fn mark_restored_updates_only_the_matching_receipt() {
    let fixture = TempFixture::new();
    append_receipt(&fixture.path, fixture_receipt_with_id(7)).unwrap();
    mark_restored(&fixture.path, 7, 42).unwrap();
    assert_eq!(load_receipts(&fixture.path).unwrap()[0].restored_at_ms, Some(42));
    assert!(mark_restored(&fixture.path, 999, 84).is_err());
}
```

- [ ] **Step 2: Run the focused tests and prove RED**

Run: `cargo test reclaim_history::tests --locked`

Expected: compilation fails because the receipt types and codec functions do not exist yet.

- [ ] **Step 3: Implement strict `DDRH1` encoding and atomic storage**

Define the public model exactly as:

```rust
const MAGIC: &[u8; 5] = b"DDRH1";
const MAX_RECEIPTS: usize = 200;
const MAX_PATH_BYTES: usize = 4096;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind { File, Directory }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity { pub dev: u64, pub ino: u64, pub kind: FileKind }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrashEvidence { pub path: PathBuf, pub identity: FileIdentity }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptAction { Trash, Delete, Empty, Command }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub event_id: u128,
    pub completed_at_ms: i64,
    pub rec_id: String,
    pub title: String,
    pub origin: PathBuf,
    pub action: ReceiptAction,
    pub freed_bytes: i64,
    pub pending_bytes: i64,
    pub trash: Option<TrashEvidence>,
    pub finder_managed: bool,
    pub restored_at_ms: Option<i64>,
}
```

Encode integers little-endian, strings and Unix path bytes with bounded `u32`
length prefixes, action and optional fields with validated one-byte tags, and a
bounded `u32` record count. Clamp negative byte inputs when constructing a
receipt and reject negative byte values when decoding. Missing files load as an
empty vector; every other read error is surfaced.

Implement `write_receipts` with `create_dir_all(parent)`, a unique sibling temp
file, `write_all`, `sync_all`, and `rename`. Remove the temp file on failure.
`append_receipt` must load successfully before writing, append, discard only the
oldest records beyond 200, and preserve chronological order.

- [ ] **Step 4: Run the focused tests and full formatter**

Run: `cargo fmt -- --check && cargo test reclaim_history::tests --locked`

Expected: all receipt codec/store tests pass.

- [ ] **Step 5: Commit the receipt foundation**

```sh
git add src/main.rs src/reclaim_history.rs
git commit -m "Persist bounded reclaim receipts" -m "Add a strict raw-path DDRH1 codec and atomic local store that refuses corrupt overwrite and retains only the newest 200 successful cleanup receipts."
```

---

## Task 2: Capture Exact Trash Outcomes Without Weakening Cleanup

**Files:**
- Modify: `src/clean.rs`
- Modify: `src/reclaim_history.rs`
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `Receipt`, `ReceiptAction`, `TrashEvidence`, `append_receipt`, `new_event_id`, `now_ms`
- Produces: `TrashOutcome::{Exact(TrashEvidence), FinderManaged}`
- Changes: `trash_path(&Path) -> Result<TrashOutcome, String>`
- Changes: `run_clean(Vec<CleanJob>, PathBuf, Sender<CleanEvent>)`
- Changes: `CleanEvent::Result` adds `history_warning: Option<String>`

- [ ] **Step 1: Write failing exact-path and receipt-warning tests**

Add fixture tests proving:

```rust
#[test]
fn trash_path_returns_collision_safe_exact_identity() {
    let fixture = TrashFixture::new();
    std::fs::create_dir_all(fixture.trash.join("cache")).unwrap();
    let outcome = trash_path_with_home(&fixture.origin, &fixture.home, finder_must_not_run).unwrap();
    let TrashOutcome::Exact(evidence) = outcome else { panic!("expected exact rename") };
    assert!(evidence.path.starts_with(&fixture.trash));
    assert_ne!(evidence.path, fixture.trash.join("cache"));
    assert_eq!(identity_at(&evidence.path).unwrap(), evidence.identity);
    assert!(!fixture.origin.exists());
}

#[test]
fn successful_clean_stays_successful_when_receipt_write_fails() {
    let fixture = CleanFixture::new();
    let events = run_fixture_clean(fixture.job(), fixture.corrupt_history_path());
    let result = result_event(events);
    assert!(result.ok);
    assert!(result.history_warning.unwrap().contains("history"));
}

#[test]
fn command_action_is_still_locked_to_the_vetted_rec_command() {
    let events = run_fixture_command(Action::Trash);
    assert!(events.iter().any(|event| matches!(event, CleanEvent::Result { ok: true, .. })));
    assert_eq!(fixture_command_log(), "vetted-command");
}
```

- [ ] **Step 2: Run the tests and prove RED**

Run: `cargo test clean::tests --locked`

Expected: failures show `trash_path` has no typed destination and `run_clean`
does not accept or report receipt persistence.

- [ ] **Step 3: Implement typed Trash evidence and worker-side receipt append**

Move `FileIdentity::at(path)` into `reclaim_history.rs` using
`symlink_metadata`, `MetadataExt::dev`, `MetadataExt::ino`, and only regular
file/directory kinds. Add:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrashOutcome { Exact(TrashEvidence), FinderManaged }
```

Keep the existing same-volume rename first and Finder AppleScript fallback. On
rename success, read identity from the final target and return `Exact`; if that
identity read fails, attempt to rename the item back to its original path and
return an error. Finder success returns `FinderManaged`.

Pass `history_path_for_home(&App::home_dir())` into `run_clean`. After each
successful job, construct a receipt from the live vetted `Rec`, effective
action, measured result, and Trash outcome, then call `append_receipt` on the
clean worker before sending `CleanEvent::Result`. Store no command text/output.
Attach any append error to `history_warning` without changing `ok`.

In `App::poll_clean_events`, add an amber activity line for the warning and
start a history refresh after `CleanEvent::Done`.

- [ ] **Step 4: Run cleanup, receipt, and full tests**

Run: `cargo fmt -- --check && cargo test clean::tests --locked && cargo test reclaim_history::tests --locked && cargo test --locked`

Expected: all tests pass; real-data cleanup is never invoked.

- [ ] **Step 5: Commit exact cleanup receipts**

```sh
git add src/clean.rs src/reclaim_history.rs src/app.rs
git commit -m "Record exact cleanup outcomes" -m "Capture the final direct Trash path and identity on the clean worker, persist successful action receipts, and surface history failures as warnings without lying about cleanup success."
```

---

## Task 3: Read-only Recovery Classification and Exact Restore Worker

**Files:**
- Modify: `src/reclaim_history.rs`

**Interfaces:**
- Produces: `ReceiptState`, `ReceiptItem`, `ReclaimHistory`, and `RestoreBlock`
- Produces: `refresh_history(&Path, &Path) -> Result<ReclaimHistory, String>`
- Produces: `can_confirm_restore(bool, &ReceiptState) -> bool`
- Produces: `RestoreJob`, `RestoreEvent`, and `run_restore(RestoreJob, Sender<RestoreEvent>)`

- [ ] **Step 1: Write failing classification and restore fixture tests**

Cover every state and mutation boundary:

```rust
#[test]
fn classification_distinguishes_ready_missing_occupied_changed_manual_and_restored() {
    let fixture = RestoreFixture::new();
    assert_eq!(classify(&fixture.receipt, &fixture.home), ReceiptState::Ready);
    std::fs::rename(&fixture.trash_item, &fixture.replacement).unwrap();
    assert_eq!(classify(&fixture.receipt, &fixture.home), ReceiptState::Missing);
    fixture.replace_with_different_inode();
    assert_eq!(classify(&fixture.receipt, &fixture.home), ReceiptState::Changed);
    fixture.occupy_origin();
    assert_eq!(classify(&fixture.current_receipt(), &fixture.home), ReceiptState::OriginOccupied);
    assert_eq!(classify(&fixture.finder_receipt(), &fixture.home), ReceiptState::ManualOnly);
    assert_eq!(classify(&fixture.restored_receipt(), &fixture.home), ReceiptState::Restored);
}

#[test]
fn restore_moves_exact_item_back_and_marks_only_its_receipt() {
    let fixture = RestoreFixture::new();
    let outcome = perform_restore(&fixture.job()).unwrap();
    assert_eq!(std::fs::read(&fixture.origin).unwrap(), b"fixture");
    assert!(!fixture.trash_item.exists());
    assert_eq!(load_receipts(&fixture.history).unwrap()[0].restored_at_ms, Some(outcome.restored_at_ms));
}

#[test]
fn restore_blocks_origin_collision_inode_replacement_symlink_ancestor_and_protected_roots() {
    assert_eq!(fixture_with_origin_collision().block(), RestoreBlock::OriginOccupied);
    assert_eq!(fixture_with_replaced_trash_item().block(), RestoreBlock::Changed);
    assert_eq!(fixture_with_symlink_parent().block(), RestoreBlock::SymlinkAncestor);
    assert_eq!(fixture_for_origin(Path::new("/System/cache")).block(), RestoreBlock::UnsafeOrigin);
}

#[test]
fn worker_rechecks_identity_after_dialog_preflight() {
    let fixture = RestoreFixture::new();
    assert_eq!(classify(&fixture.receipt, &fixture.home), ReceiptState::Ready);
    fixture.replace_with_different_inode();
    assert!(perform_restore(&fixture.job()).is_err());
    assert!(!fixture.origin.exists());
}
```

Also add one `#[ignore]` test named `seed_signed_visual_fixture`. It must run
only when `DISKDECK_QA_HISTORY` equals the normal current-user history path,
abort if any path named `DiskDeck-QA-Reclaim-History` already exists, and create
only that exact fixture name under `$HOME/.Trash` and
`$HOME/Library/Caches`. It writes display-only Ready, Missing, Changed,
ManualOnly, Permanent, and Restored receipts. Because the helper is inside
`#[cfg(test)]`, no seeding or arbitrary-path interface enters the release app.

- [ ] **Step 2: Run focused tests and prove RED**

Run: `cargo test reclaim_history::tests --locked`

Expected: compilation fails because classification and restore types are absent.

- [ ] **Step 3: Implement fail-closed classification**

Define:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptState {
    Ready,
    Missing,
    OriginOccupied,
    Changed,
    ManualOnly,
    UnsafeOrigin,
    SymlinkAncestor,
    CrossDevice,
    Restored,
    Permanent,
}

#[derive(Clone, Debug)]
pub struct ReceiptItem { pub receipt: Receipt, pub state: ReceiptState }

#[derive(Clone, Debug, Default)]
pub struct ReclaimHistory {
    pub items: Vec<ReceiptItem>,
    pub freed_bytes: i64,
    pub pending_bytes: i64,
    pub recoverable_count: usize,
}
```

Validate normalized components without canonicalizing the missing origin.
Require the exact Trash path to have parent exactly `$HOME/.Trash`; reject
symlinks and identity mismatch. Reject `/`, `/System/**`, `/Applications/**`,
the exact `/Library`, `/Users`, `$HOME`, `$HOME/Library`, and `$HOME/.Trash/**`
as origins. Walk each existing origin ancestor with `symlink_metadata`; do not
follow a symlink. Compare the recorded Trash device with the original parent's
device. Sort refreshed items newest first and derive summary totals with
saturating non-negative arithmetic.

- [ ] **Step 4: Implement the one-rename restore worker**

Define:

```rust
pub struct RestoreJob { pub receipt: Receipt, pub history_path: PathBuf, pub home: PathBuf }

pub enum RestoreEvent {
    Started { title: String, bytes: i64 },
    Done { bytes: i64, origin: PathBuf, warning: Option<String> },
    Failed { error: String },
}
```

`perform_restore` repeats preflight, calls one `fs::rename(trash, origin)`,
verifies the recorded identity at the origin, and calls `mark_restored`. A
history-write failure returns success with a warning. If post-rename identity
verification fails, attempt one exact rename back only when the Trash path is
still absent and the origin is still the just-observed item, then return a
failure that states whether rollback succeeded. `run_restore` uses a named
background thread and streams `Started` then one terminal event.

- [ ] **Step 5: Run focused and full tests**

Run: `cargo fmt -- --check && cargo test reclaim_history::tests --locked && cargo test --locked`

Expected: all tests pass with mutation limited to test fixtures.

- [ ] **Step 6: Commit verified Trash restore**

```sh
git add src/reclaim_history.rs
git commit -m "Restore exact Trash receipts safely" -m "Classify current receipt state and require normalized paths, unchanged identity, a vacant origin, real ancestors, one device, and a repeated worker preflight before atomic move-back."
```

---

## Task 4: Reclaim History Workspace and Confirmation UX

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `refresh_history`, `ReclaimHistory`, `ReceiptItem`, `ReceiptState`, and aliased `run_restore`
- Produces: `RailView::ReclaimHistory`
- Produces: on-demand refresh/poll state and a dedicated `TrashRestoreDialog`
- Produces: `mutation_busy(&App) -> bool` shared by reclaim/offload/moved restore/Trash restore entry points

- [ ] **Step 1: Write failing UI state and concurrency tests**

Add pure app tests for:

```rust
#[test]
fn reclaim_history_is_an_insights_child_and_escape_returns_to_insights() {
    assert_eq!(rail_back_target(RailView::ReclaimHistory), Some(RailView::Insights));
}

#[test]
fn recovery_copy_is_plain_and_never_promises_permanent_undo() {
    assert_eq!(receipt_state_copy(&ReceiptState::Ready), "Ready to restore");
    assert_eq!(receipt_state_copy(&ReceiptState::Permanent), "Permanent — cannot restore");
    assert_eq!(receipt_state_copy(&ReceiptState::ManualOnly), "Open Trash to restore manually");
}

#[test]
fn mutation_gate_blocks_every_overlapping_pipeline() {
    let mut app = App::new_for_test();
    app.offloading = true;
    assert!(app.mutation_busy());
    app.offloading = false;
    app.restoring_trash = true;
    assert!(app.mutation_busy());
}

#[test]
fn trash_restore_requires_acknowledgement_ready_state_and_current_dialog() {
    assert!(can_start_trash_restore(true, &ReceiptState::Ready, false));
    assert!(!can_start_trash_restore(false, &ReceiptState::Ready, false));
    assert!(!can_start_trash_restore(true, &ReceiptState::Changed, false));
    assert!(!can_start_trash_restore(true, &ReceiptState::Ready, true));
}
```

- [ ] **Step 2: Run app tests and prove RED**

Run: `cargo test app::tests --locked`

Expected: compilation fails because the rail, state copy, and mutation gate are missing.

- [ ] **Step 3: Add on-demand history worker state**

Add application fields for the refresh receiver, `ReclaimHistory`, error,
Trash-restore receiver, `restoring_trash`, dialog, and hold progress. Opening
the rail calls `begin_reclaim_history_refresh`; cleanup completion and restore
completion refresh it again. No call is added to the five-minute menu monitor.

Poll refresh and restore receivers in `update`. Include only active receivers,
dialog, and workers in the existing 40 ms repaint condition. Escape closes the
Trash restore dialog before navigating Back.

Add `mutation_busy` and enforce it at the final entry point for reclaim,
offload, moved-item restore, and Trash restore. Scanning stays independent.

- [ ] **Step 4: Build the workspace**

Add a **Reclaim History** row to Insights with the detail “Cleanup receipts and
verified Trash recovery”. The workspace must render:

- header and `← Insights` Back control;
- loading, empty, corrupt/unavailable, and populated states;
- three non-overlapping summary cards for freed, pending in Trash, and
  recoverable receipt count;
- newest-first rows with title, time, action, original path, size, and exact
  `receipt_state_copy`;
- **Restore…** only for `Ready`;
- **Reveal in Trash** only for an existing exact Trash path;
- **Open Trash** for manual-only/missing cases;
- no cleanup, erase, command, or checkbox controls.

Use existing Adaptive Native colors: mint for Ready/Restored, amber for manual
or changed states, red only for permanent action semantics, cyan for navigation.
Long paths elide inside their content column; action controls have a reserved
right column at minimum window width.

- [ ] **Step 5: Build the confirmation surface and event handling**

Render a modal-like egui `Area` consistent with moved-item restore. Show exact
original and Trash paths, receipt age, size, preflight message, one acknowledgement
checkbox, and the hold control labelled **Hold to restore from Trash**. Reset
hold progress whenever acknowledgement/state/dialog changes. On success close
the dialog, mark the old scan visually stale, update stats/activity, refresh
history, and say “Rescan to refresh the terrain map”. On failure retain the
dialog only if a new refresh still classifies the receipt as Ready.

- [ ] **Step 6: Run app and full tests**

Run: `cargo fmt -- --check && cargo test app::tests --locked && cargo test --locked`

Expected: all tests pass and no UI-thread filesystem mutation is introduced.

- [ ] **Step 7: Commit the recovery workspace**

```sh
git add src/app.rs
git commit -m "Add the Reclaim History workspace" -m "Expose bounded cleanup receipts and a discoverable verified Trash restore flow while serializing all mutating pipelines and keeping receipt I/O off the UI thread."
```

---

## Task 5: Public Contract and Non-mutating Smoke Coverage

**Files:**
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-signed-ui.sh`
- Modify: `scripts/test-ui-smoke.sh`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-12-reclaim-history-recovery-design.md`

**Interfaces:**
- Produces: a signed smoke assertion for the Reclaim History rail and heading
- Preserves: smoke never clicks Restore, Reveal, Open Trash, reclaim, move, scan, or command controls

- [ ] **Step 1: Write the failing smoke-tooling assertion**

Extend `scripts/test-ui-smoke.sh` so it requires the AppleScript to contain the
visible `Reclaim History` entry/heading and requires `scripts/test-signed-ui.sh`
to list `Restore`, `Reveal`, `Open Trash`, and `Hold to restore` among forbidden
click labels.

Run: `scripts/test-ui-smoke.sh`

Expected: FAIL until the AppleScript and forbidden-label contract are updated.

- [ ] **Step 2: Add non-mutating signed navigation**

Have the AppleScript click **Reclaim History**, wait for the same heading, log a
PASS, then send Escape and confirm it returns to Insights. Do not inspect or
activate row action buttons. Keep the existing context-menu and rail checks.

Run: `scripts/test-ui-smoke.sh`

Expected: `UI smoke tooling checks passed`.

- [ ] **Step 3: Document the shipped boundary**

README must explain receipt retention, exact-versus-manual Trash recovery,
empty-Trash irreversibility, permanent/command irreversibility, local-only
storage, and fixture-only restore proof. Add `reclaim_history.rs` to the module
table and Reclaim History to the interaction table.

AGENTS must add invariants for `DDRH1`, corrupt-store refusal, exact Trash
identity, no overwrite/delete/copy/Finder restore, worker-only I/O, serialized
mutations, and fixture-only tests. Add the module to the flat-layout convention.

Update the design status to **shipped** only after Task 6 signed and CI proof.

- [ ] **Step 4: Run documentation and repository guards**

Run:

```sh
scripts/test-ui-smoke.sh
scripts/test-community-files.sh
scripts/test-package-artifact.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
```

Expected: every guard passes.

- [ ] **Step 5: Commit the public contract**

```sh
git add README.md AGENTS.md scripts/ui-smoke.applescript scripts/test-signed-ui.sh scripts/test-ui-smoke.sh docs/superpowers/specs/2026-07-12-reclaim-history-recovery-design.md
git commit -m "Ship the Reclaim History contract" -m "Document exact and manual Trash recovery boundaries and extend signed smoke navigation without activating any mutation or Finder controls."
```

---

## Task 6: Fixture Mutation Proof, Signed Visual QA, Release, and CI

**Files:**
- Modify only if proof finds a defect: files owned by Tasks 1–5

**Interfaces:**
- Consumes: complete Reclaim History slice
- Produces: verified signed app, clean `dist/DiskDeck.zip`, pushed `main`, green exact GitHub CI run

- [ ] **Step 1: Run the complete local gate**

Run:

```sh
cargo fmt -- --check
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
cargo test --locked
```

Expected: all shell guards pass and all Rust tests pass.

- [ ] **Step 2: Build and verify the signed shipping artifact**

Run:

```sh
./make-app.sh
codesign --verify --strict --verbose=2 /Applications/DiskDeck.app
scripts/check-dist.sh dist/DiskDeck.zip
scripts/test-signed-ui.sh
```

Expected: stable signature valid, distribution archive clean, every non-mutating
signed UI rail check passes.

- [ ] **Step 3: Repeat the fixture-only mutation proof**

Run:

```sh
cargo test reclaim_history::tests::restore_moves_exact_item_back_and_marks_only_its_receipt --locked -- --exact
cargo test reclaim_history::tests::worker_rechecks_identity_after_dialog_preflight --locked -- --exact
```

Expected: the first test proves exact fixture rename and receipt update; the
second proves a replaced item blocks before mutation. Both fixtures live only
under their test-owned temporary directories. The signed app is not given a
fake HOME and no real recommendation or Trash item is touched.

- [ ] **Step 4: Inspect the signed UI visually with reversible QA receipts**

First quit DiskDeck. Back up the existing history file byte-for-byte when it
exists and remember the absence case. Set `DISKDECK_QA_HISTORY` to
`$HOME/Library/Application Support/DiskDeck/reclaim-history.ddrh`, then run:

```sh
cargo test reclaim_history::tests::seed_signed_visual_fixture --locked -- --ignored --exact
```

Launch `/Applications/DiskDeck.app` and inspect Reclaim History at 1180×740 and
1480×920 in light and dark appearance. Open the Ready restore dialog but do not
hold or restore. Confirm Ready, Missing, Changed, ManualOnly, Permanent, and
Restored copy; non-overlapping cards/rows; reserved action width; visible Back;
path elision; and no tofu glyphs or misleading totals.

Quit the app, remove only the exact
`$HOME/.Trash/DiskDeck-QA-Reclaim-History` and
`$HOME/Library/Caches/DiskDeck-QA-Reclaim-History` fixture paths, then restore
the original history file byte-for-byte or restore its absence. Relaunch the
signed app and confirm the user's original/empty state. Never click Restore,
Reveal, Open Trash, cleanup, move, or scan during visual proof.

- [ ] **Step 5: Mark the design shipped and commit proof adjustments**

Change the design status from approved to shipped and state the exact visual and
fixture proof boundaries. Run `cargo test --locked`, then commit any final proof
or copy adjustment with an imperative subject and why-focused body.

- [ ] **Step 6: Fast-forward `main`, push, and watch exact CI**

From the root worktree:

```sh
git merge --ff-only codex/reclaim-history-recovery
git push origin main
gh run list --repo raghavaadi/DiskDeck --commit "$(git rev-parse HEAD)" --limit 1
gh run watch <exact-run-id> --repo raghavaadi/DiskDeck --exit-status
```

Expected: the exact pushed SHA completes with conclusion `success`, including
the Rust suite and every repository guard.

- [ ] **Step 7: Audit and clean the implementation worktree**

Verify `HEAD == origin/main`, clean status, valid signed app, clean ZIP, and no
unmet design requirement. Remove only the completed worktree and its ignored
`target` directory, delete the merged local feature branch, and prune worktree
metadata. Leave the installed signed app and `dist/DiskDeck.zip` intact.
