# Regrowth History — design

**Date:** 2026-07-11
**Status:** approved as DiskDeck v2 slice 1

## Problem

A completed DiskDeck scan explains current usage but cannot answer the more
valuable recurring question: **what grew since the last scan?** Without a
baseline, users repeatedly rediscover the same large folders and cannot tell
which application or workflow is consuming space over time.

## User experience

After the first successful scan, the capacity card says **Baseline saved —
compare after the next scan**. Later completed scans show:

- total mapped change since the previous compatible scan;
- the largest positive grower and its byte delta;
- an Activity entry listing up to the five largest growers.

Negative total change is shown as space reduced, not as growth. Small movement
below 10 MB is omitted from the grower list. The live treemap remains unchanged;
history appears only after a completed, compacted scan and never during live
growth. Aborted scans do not become baselines.

## Snapshot model

Add `src/history.rs` with:

```rust
pub struct Growth {
    pub path: PathBuf,
    pub bytes_delta: i64,
}

pub struct GrowthSummary {
    pub previous_at_ms: i64,
    pub total_delta: i64,
    pub growers: Vec<Growth>,
}

pub enum HistoryEvent {
    BaselineSaved,
    Compared(GrowthSummary),
    Failed(String),
}

pub fn default_history_dir() -> Option<PathBuf>;
pub fn record_scan(root: Arc<Node>, dir: PathBuf, tx: Sender<HistoryEvent>);
```

An internal `Snapshot` contains the capture time, scan-root path, total mapped
bytes, and compact-tree entries. Each entry stores a root-relative path, byte
count, file count, and directory flag. Paths are encoded from raw macOS path
bytes, not lossy UTF-8.

The snapshot walk runs only after `scan::compact` and keeps the same materialized
nodes the user can navigate. Bytes folded into “smaller items” remain represented
in their nearest retained ancestor, so the comparison never claims a precision
the map does not possess.

## Local storage

Snapshots live under:

`~/Library/Application Support/DiskDeck/History`

This directory is local, protected by the SSD-offload policy, and never
uploaded. Files use a small versioned length-prefixed binary format with a
fixed magic header. Each write goes to a sibling temporary file, calls
`sync_all`, and atomically renames into place. Filenames use millisecond capture
time plus process id to avoid collisions.

Keep the 12 newest snapshot files. Before saving a new snapshot, load the newest
valid snapshot with the same scan root. A corrupt or unknown-version file is
skipped; if no valid compatible snapshot remains, the new scan becomes a fresh
baseline. Retention cleanup never touches files that do not match DiskDeck's
snapshot filename pattern.

## Comparison

Comparison indexes previous entries by relative path. For every current entry:

- missing previously: delta equals current bytes;
- present previously: delta equals current minus previous bytes;
- positive delta of at least 10 MB: candidate grower;
- zero or negative delta: not a grower.

Growers sort by descending delta, then path, and truncate to five. Total delta
comes from the two snapshot totals so removals remain visible even though the
grower list is positive-only. Snapshots with different roots are incompatible.

## Threading and app integration

`App::on_scan_finished` starts one named history worker only for `ScanState::Done`.
The worker captures, loads, compares, atomically saves, prunes, and emits one
`HistoryEvent`. `App::poll_history` consumes the event on the UI thread, updates
the capacity-card state, and writes the Activity summary. The UI requests
repaint while a history worker is pending but performs no snapshot I/O itself.

A new scan clears the displayed comparison so an old delta is never presented
as belonging to a running scan.

## Error handling

- Missing `$HOME`: history is unavailable; scanning still succeeds.
- Directory creation, encoding, syncing, or rename failure: emit `Failed`, keep
  the scan result, and leave any previous snapshots intact.
- Corrupt snapshot: skip it and try the next newest file.
- Worker panic or channel disconnect: clear pending state and report one local
  Activity warning; do not affect scan, cleanup, or offload.

## Testing

Unit tests cover raw-path codec round trips, corrupt/truncated input, comparison
ordering and thresholds, incompatible roots, atomic record/load, retention of
exactly 12 matching snapshots, and preservation of unrelated files. An app
policy test proves aborted scans never start history recording. Full verification
includes formatting, Rust tests, repository guards, signed build, and visual
inspection of baseline and comparison copy without mutating user data.

## Non-goals

- No chart, calendar, per-day rate, or arbitrary snapshot picker in this slice.
- No history upload, sync, export, or telemetry.
- No file-content hashing.
- No comparison while a scan is running or after an aborted scan.
