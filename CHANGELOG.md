# Changelog

All notable DiskDeck changes are documented here. Versions follow Semantic
Versioning, and dates use ISO 8601.

## [1.0.0] - 2026-07-12

### Highlights

- Watch an on-disk, hardlink-aware terrain map grow while DiskDeck scans the
  real APFS data volume, then zoom, search, Quick Look, or reveal retained
  folders and large files without a hidden second scan.
- Build an explicit Safe-only reclaim plan for a space goal, review every
  target, and hold for 900 ms before any cleanup. Caution findings remain
  unchecked and command findings remain locked to vetted commands.
- Inspect an external local drive or one Finder-selected folder in isolated
  read-only maps. Network, whole-volume, missing, replaced, and symlinked
  targets fail closed and never enter cleanup.
- Restore exact unchanged direct Trash moves, inspect verified SSD offloads,
  and restore eligible moved items through staged copies and repeated identity
  checks without overwriting occupied paths.
- Compare compact scan history, recurring growers, watched folders, and honest
  storage forecasts whose confidence is tied to explicit evidence thresholds.
- Use Developer Deep Dive, APFS accounting, app-leftover review, duplicate and
  large-old file review, and the optional menu-bar monitor without converting
  display evidence into cleanup claims.
- Follow native macOS light/dark appearance with the adaptive DiskDeck icon,
  responsive layouts, visible Back navigation, and discoverable context menus.

### Distribution

- Supports Apple Silicon Macs running macOS 12 or later.
- The public `DiskDeck.zip` contains `DiskDeck.app` and the one-time
  `Install DiskDeck.command` helper.
- Public binaries are Developer ID-signed, hardened-runtime enabled,
  notarized, stapled, Gatekeeper-assessed, and accompanied by
  `SHA256SUMS.txt`. Development-signed local QA builds are never release
  assets.

### Known limitations

- Intel Macs are not supported in v1.0.0.
- Snapshot count is reported when available, but macOS does not expose a
  dependable exact reclaimable snapshot or purgeable-byte figure for DiskDeck
  to promise.
- Network volumes are intentionally excluded from External drives and Folder
  Lens.

[1.0.0]: https://github.com/raghavaadi/DiskDeck/releases/tag/v1.0.0
