# APFS Accounting Implementation Plan

**Goal:** Explain APFS container capacity and snapshots without presenting
approximate system-managed space as immediately reclaimable.

- Parse only bounded `diskutil` plist fields from a fixed command invocation:
  APFS container size/free and the local snapshot list.
- Never accept a device or command from UI input.
- Treat unavailable, malformed, or timed-out values as unavailable rather than
  zero.
- Show file usage separately from container free, snapshot count separately
  from bytes, and purgeable/snapshot byte sizes as “not reliably reported”
  when macOS does not provide exact values.
- Add a read-only APFS rail, deterministic parser tests, non-destructive signed
  smoke navigation, documentation, full gates, signed build, and visual proof.
