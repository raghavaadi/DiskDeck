# Duplicate and Large-Old File Review Plan

**Goal:** Add an opt-in, read-only user-file review that proves exact duplicate
groups and surfaces very large files with old access metadata.

- Never run automatically. Scan only existing standard user roots: Desktop,
  Documents, Downloads, Movies, Music, and Pictures.
- Do not follow symlinks, cross filesystems, enter hidden directories,
  `node_modules`, app/media-library bundles, or Library/system roots.
- Duplicate floor: 10 MB logical size. Group by size, stream a deterministic
  fingerprint, then verify every reported copy byte-for-byte; fingerprint
  equality alone is never proof.
- Large-old floor: 1 GB on-disk and access time at least 180 days old. Explain
  that macOS access metadata can be coarse or disabled.
- Bound traversal/candidate/results, support cancellation, and perform all work
  on a named worker.
- Results are never selected and expose Quick Look/Finder reveal only. No
  delete/move action is added, so every duplicate group keeps all copies.
- Add fixture tests, UI/smoke/docs, full privacy gates, signed build, and live
  minimum-window proof without scanning owner data during automation.
