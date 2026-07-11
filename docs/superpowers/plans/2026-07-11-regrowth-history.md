# Regrowth History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist compact completed-scan snapshots locally and show the total change plus the five largest growers since the previous scan.

**Architecture:** A new `history.rs` module owns snapshot capture, raw-path binary encoding, comparison, atomic storage, retention, and its worker event. `app.rs` only starts the worker at the completed-scan boundary, polls one event, and paints the resulting baseline or comparison copy.

**Tech Stack:** Rust 2021 standard library, existing `Node` tree, mpsc, macOS raw path bytes, egui 0.29.

## Global Constraints

- No new crate dependency.
- Only `ScanState::Done` may record history; aborted and running scans never do.
- History is local under `~/Library/Application Support/DiskDeck/History`.
- Writes are sibling-temp + `sync_all` + atomic rename.
- Keep 12 matching snapshot files and never delete unrelated directory contents.
- Compare compacted nodes only; omit positive growers below 10 MB and keep five.
- Scan, cleanup, offload, bundle id, signing identity, and 900 ms hold invariants remain unchanged.

---

### Task 1: Snapshot codec and comparison

**Files:**
- Create: `src/history.rs`
- Modify: `src/main.rs`
- Test: inline `src/history.rs`

**Interfaces:**
- Produces: public `Growth`, `GrowthSummary`, `HistoryEvent`; internal `Entry`, `Snapshot`, `encode`, `decode`, and `compare`.

- [ ] **Step 1: Add failing comparison tests**

Construct snapshots with root `/System/Volumes/Data`, then assert a new 30 MB
entry and a 20 MB increase sort ahead of each other by delta, a 9 MB increase
is omitted, negative changes affect `total_delta` only, and different roots
return no comparison.

- [ ] **Step 2: Confirm comparison RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests::compare -- --nocapture`

Expected: compilation fails because the module and types do not exist.

- [ ] **Step 3: Implement the snapshot types and comparison**

Use these public shapes:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Growth { pub path: PathBuf, pub bytes_delta: i64 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowthSummary {
    pub previous_at_ms: i64,
    pub total_delta: i64,
    pub growers: Vec<Growth>,
}

pub enum HistoryEvent { BaselineSaved, Compared(GrowthSummary), Failed(String) }
```

`compare` returns `None` for different roots. Otherwise map previous entries by
raw relative `PathBuf`, retain positive deltas ≥ `10 << 20`, sort descending by
delta then path, truncate to five, and use snapshot totals for `total_delta`.

- [ ] **Step 4: Confirm comparison GREEN**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests::compare -- --nocapture`

Expected: comparison tests pass.

- [ ] **Step 5: Add failing raw-path codec tests**

On Unix, construct an entry path from bytes containing `0xFF`. Assert encode →
decode preserves all fields and raw path bytes. Also assert a truncated payload,
wrong magic, excessive entry count, and trailing garbage return errors.

- [ ] **Step 6: Confirm codec RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests::codec -- --nocapture`

Expected: tests fail until codec functions exist.

- [ ] **Step 7: Implement the bounded binary codec**

Use magic `DDHIST1\0`, little-endian integers, raw path length + bytes, bytes,
files, and a 0/1 directory byte. Reject path lengths above 1 MiB, counts above
1,000,000, invalid flags, short reads, and trailing bytes.

- [ ] **Step 8: Run Task 1 tests and commit**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests -- --nocapture
git add src/history.rs src/main.rs
git commit -m "Add regrowth snapshot model"
```

---

### Task 2: Tree capture, atomic storage, retention, and worker

**Files:**
- Modify: `src/history.rs`
- Test: inline `src/history.rs`

**Interfaces:**
- Consumes: `scan::Node`, Task 1 codec and comparison.
- Produces: `default_history_dir()`, private `capture`, `record`, and public `record_scan`.

- [ ] **Step 1: Add failing capture and record tests**

Use a tiny temporary scan tree and wait for `ScanState::Done`; assert capture
stores root-relative children and the root total. In a temporary history
directory, record 14 synthetic snapshots plus an unrelated `keep.txt`; assert
exactly 12 matching snapshot files remain and `keep.txt` remains.

- [ ] **Step 2: Confirm storage RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests::storage -- --nocapture`

Expected: capture and record helpers do not compile.

- [ ] **Step 3: Implement capture and local path**

Walk `Node::kids()` recursively after scan completion, store every materialized
child relative to `root.path`, and copy atomic byte/file counts. Return
`$HOME/Library/Application Support/DiskDeck/History` from
`default_history_dir`, or `None` without `$HOME`.

- [ ] **Step 4: Implement atomic record and safe retention**

List only names matching `snapshot-<digits>-<digits>.ddhist`. Load newest-first
until a valid compatible snapshot is found. Write the new snapshot to a sibling
hidden temp, `sync_all`, rename, then keep the 12 newest matching files. Skip
corrupt versions and never remove unrelated files.

- [ ] **Step 5: Implement the named worker**

`record_scan(root, dir, tx)` spawns `scan-history`. It captures current time in
milliseconds, calls record, sends `BaselineSaved` without a previous compatible
snapshot, `Compared(summary)` otherwise, or `Failed(error)` once.

- [ ] **Step 6: Confirm storage GREEN and commit**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
PATH="$HOME/.cargo/bin:$PATH" cargo test history::tests -- --nocapture
git add src/history.rs
git commit -m "Persist bounded scan history"
```

---

### Task 3: Completed-scan integration and capacity-card UX

**Files:**
- Modify: `src/app.rs`
- Test: inline `src/app.rs`

**Interfaces:**
- Consumes: `default_history_dir`, `record_scan`, `GrowthSummary`, `HistoryEvent`.
- Produces: `history_rx`, `regrowth`, `history_baseline`, `poll_history`, and capacity-card comparison copy.

- [ ] **Step 1: Add a failing completed-only policy test**

Extract `should_record_history(state: ScanState) -> bool` and assert true only
for `Done`, false for `Idle`, `Running`, and `Aborted`.

- [ ] **Step 2: Confirm app policy RED**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::history_records -- --nocapture`

Expected: helper does not exist.

- [ ] **Step 3: Add app state and worker lifecycle**

Initialize the three history fields in `App::new`. Clear visible history in
`begin_scan`. At the end of `on_scan_finished`, when the helper allows and the
default directory exists, start one worker and store its receiver. Poll it in
`update`; clear the receiver on every terminal event.

Activity copy:

- baseline: `scan baseline saved — future scans will show what grew`;
- compared: `since last scan: <signed total>; largest growth: <display path> <delta>`;
- failed: `scan history unavailable — <error>`.

- [ ] **Step 4: Paint baseline and comparison copy**

In `draw_capacity`, add one quiet line below the capacity subtitle. Baseline is
muted. Positive total delta uses Caution, zero/negative uses Safe. Show
`Since last scan: <signed bytes>` and append the largest grower's display name
and positive delta when present. Do not resize or restructure the card.

- [ ] **Step 5: Confirm app and full suite GREEN, then commit**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
git add src/app.rs
git commit -m "Show growth since the previous scan"
```

---

### Task 4: Documentation, signed proof, and publication

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Verify: `/Applications/DiskDeck.app`

- [ ] **Step 1: Document local history and privacy**

Add regrowth tracking to README features and tests, state that 12 compact
snapshots stay only in Application Support, and record in AGENTS that aborted
or live scans must never become baselines.

- [ ] **Step 2: Run the complete gate**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-community-files.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
```

- [ ] **Step 3: Build and inspect the signed app**

Run `PATH="$HOME/.cargo/bin:$PATH" ./make-app.sh`, verify codesign and bundle id,
complete two non-destructive scans, and confirm baseline copy after the first
and comparison copy after the second. Run `scripts/test-signed-ui.sh`.

- [ ] **Step 4: Commit docs, merge, push, and prove remote**

Commit with `Document local regrowth history`, integrate to `main`, rerun tests,
push, and assert local HEAD equals `git ls-remote origin refs/heads/main`.
