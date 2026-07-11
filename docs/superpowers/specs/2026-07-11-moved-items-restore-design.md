# DiskDeck Moved Items and Restore Center — design

**Date:** 2026-07-11
**Status:** approved for autonomous phased delivery

## Goal

Make every successful SSD offload visible and safely reversible. DiskDeck shows
where an item moved, whether its drive and origin link are healthy, and restores
it to the Mac through the same explicit, verified safety boundary used for an
offload.

## Product behavior

The Reclaim summary gains a **Moved items** entry. It opens a rail view listing
the newest moves first. Each row shows the item name, original path, target
drive, on-disk size, move date, and one of these deterministic states:

- **Ready to restore:** target exists and the origin is absent or is the exact
  symlink DiskDeck created.
- **Drive disconnected:** the recorded target volume is not mounted.
- **Origin changed:** the original path now contains a real item or a different
  symlink. Restore is blocked rather than overwriting it.
- **Target missing:** the drive is mounted but the recorded destination is gone.
- **Restored:** the origin is a real item and the target no longer exists after
  a completed DiskDeck restore.

Unavailable actions stay visible but disabled with a plain-language reason.
There is no automatic restore, bulk restore, or "fix all" action in this slice.

Selecting **Restore to Mac…** opens a confirmation sheet that names the source
drive, destination path, required space, and whether an origin symlink will be
replaced. The user must tick an acknowledgement and hold for 900 ms. The sheet
never offers a destructive alternative.

## Record model and discovery

Future offloads write a lossless local move registry under:

`~/Library/Application Support/DiskDeck/Moves/index.ddmoves`

The codec stores raw macOS path bytes, moved time, measured bytes, and whether a
symlink was created. It is versioned, bounded, atomically replaced, and skips a
corrupt registry without touching moved data. No path is uploaded or logged to
analytics.

DiskDeck retains the existing append-only JSON ledger on each target drive as
an on-drive recovery trail. When a drive is attached, Restore Center reconciles
its ledger with the local registry. Exact local binary records win; legacy JSON
records may be imported only when both paths are absolute, normalized, and the
destination is beneath that drive's `DiskDeck Offload` directory.

The registry is refreshed after launch, after an offload, after a restore, and
when the user presses **Refresh**. It does not require a full disk scan.

## Restore safety contract

Restore runs on a background worker and repeats every check at execution time.
UI eligibility is advisory only.

1. Validate that the record is structurally safe: the origin is absolute and
   inside the current home folder; the destination is absolute, normalized,
   mounted beneath `/Volumes/<name>/DiskDeck Offload`, and exists as a real
   file or directory rather than a symlink.
2. Accept the origin only when it is absent or is a symlink whose resolved
   target exactly matches the recorded destination. Any real item, dangling
   unrelated link, or symlinked ancestor blocks restore.
3. Measure the destination and require its apparent size plus the existing
   100 MB margin on the internal data volume.
4. Capture the destination device/inode identity.
5. Copy with `/usr/bin/ditto` to a unique staging sibling beside the origin.
6. Verify the staging copy by apparent size. A failed copy or verification
   leaves both the destination and origin unchanged.
7. Recheck the destination identity. If an origin link exists, rename it to a
   unique backup sibling; atomically rename staging into the origin. If the
   install rename fails, restore the backed-up link.
8. Verify the installed origin, recheck destination identity again, and only
   then delete the external destination. Remove the backed-up link after the
   origin is installed.
9. Atomically mark the local record restored. A registry-write failure is
   reported as a warning but never rolls back or misrepresents the completed
   file move; the next refresh derives state from the filesystem.

Temporary staging and backup names are collision-checked. Failure cleanup may
remove only a staging path created by that restore attempt. Tests never restore
the owner's real data.

## Architecture

Add `moves.rs` as the focused owner of move records, the binary codec, local
registry I/O, record reconciliation, filesystem state classification, restore
preflight, and the restore worker. Add `transfer.rs` for the neutral primitives
shared by offload and restore: apparent-size measurement, path identity,
identity rechecks, collision checks, and verified `ditto` copy. `offload.rs`
continues to own outbound policy and ordering; `moves.rs` owns restore policy
and ordering. `app.rs` owns the Moved items rail and confirmation sheet and
consumes worker events through channels, matching scan, history, clean, and
offload patterns.

No new crate dependency is introduced. Standard library filesystem APIs,
existing `libc` disk statistics, and `/usr/bin/ditto` are sufficient.

## Error handling

- Corrupt local registry: show "Move history unavailable" and preserve the
  file for manual recovery; do not truncate it silently.
- Legacy ledger line malformed: skip that line and continue importing others.
- Drive disconnects during restore: the copy fails and the origin remains
  unchanged; if it disconnects after origin install while target removal is in
  progress, report that the restored copy is safe and that external cleanup may
  be incomplete.
- Insufficient internal space: disable confirmation and show required versus
  available space.
- Origin collision or destination identity change: abort with both real copies
  untouched.
- App closes mid-copy: staging remains clearly named; the next refresh ignores
  it. A later maintenance slice may offer stale-staging cleanup, but this slice
  never deletes it automatically.

## UI constraints

- Preserve the Adaptive Native palette and fixed review-rail geometry.
- Long names and paths use the same bounded text/utility-column contract as
  reclaim rows.
- Status color is semantic: mint ready/restored, amber disconnected or missing,
  red origin collision, cyan active copy.
- Keyboard Escape closes the sheet without changing data.
- VoiceOver-accessible button labels must be supplied for Moved items, Refresh,
  Restore to Mac, acknowledgement, and Cancel.

## Verification

- Codec round trip includes non-UTF-8 path bytes and rejects corrupt/trailing
  payloads.
- Reconciliation deduplicates local and legacy records without losing newer
  metadata.
- State tests cover attached/detached, exact/different symlink, occupied origin,
  missing target, and restored state.
- Fixture restore tests prove copy verification, collision refusal, destination
  identity checks, rollback of a backed-up link, and successful move-back.
- Worker tests use only temporary fixture directories and a test-only target
  policy seam; no `delete` or command action runs on owner data.
- Full `cargo test --locked`, privacy/community gates, `make-app.sh`, strict
  code-signature verification, and signed-app visual checks must pass before
  merge.

## Explicit non-goals

- Bulk restore or automatic restore when a drive reconnects.
- Moving cloud-sync roots, application bundles, or hidden/protected home data.
- Editing, deleting, or compacting external ledgers.
- Cloud sync, accounts, telemetry, or AI recommendations.
- Treating a missing target as proof that restoring or deleting anything else
  is safe.
