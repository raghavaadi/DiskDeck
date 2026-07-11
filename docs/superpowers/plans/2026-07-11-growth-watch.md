# Growth Watch Implementation Plan

**Goal:** Turn the retained local scan snapshots into a useful timeline,
recurring-grower view, and user-controlled folder watchlist without adding an
always-on scan or service.

## Product contract

- Growth Watch reads only DiskDeck's retained `.ddhist` snapshots.
- The timeline advances only after an explicit normal scan completes.
- Paths and watch choices remain local under DiskDeck Application Support.
- Watchlist paths are relative to the fixed scan root and round-trip as raw
  macOS path bytes.
- Corrupt snapshots or watchlists are reported and never overwritten.
- Percentage change is shown only when a non-zero earlier measurement exists;
  otherwise the UI says `new` rather than inventing an infinite percentage.

## Task 1 — Timeline and recurring-growth model

- Add public timeline point, folder series, recurring grower, and Growth Watch
  summary types in `history.rs`.
- Load up to the 12 valid compatible snapshots oldest-to-newest.
- Calculate interval growth deterministically and rank recurring growers by
  number of positive intervals, then absolute growth.
- Test corrupt-snapshot skipping, root compatibility, ordering, missing/new
  entries, and percentage semantics.

## Task 2 — Lossless atomic watchlist

- Add a bounded binary watchlist beside the snapshots using raw Unix path
  bytes, a versioned magic header, and atomic temp-write/sync/rename.
- Reject absolute paths, parent traversal, oversized paths, truncation, wrong
  magic, trailing bytes, and corrupt-existing-file overwrite.
- Add toggle and load APIs with fixture-only tests.

## Task 3 — Background refresh and UI rail

- Add a mutually exclusive `Growth` rail state and a discoverable
  `Growth Watch` button on the summary.
- Load the timeline/watchlist on a named background worker at boot and after a
  completed history write.
- Show total trend, recurring growers, pinned folder series, absolute and
  percentage change, and explicit empty/baseline/error states.
- Let users pin or unpin only measured relative folders; persist through the
  worker and refresh the view.
- Escape and the visible Back control return to the summary without mutation.

## Task 4 — Documentation and proof

- Document local-only retention and the no-daemon behavior in README/AGENTS.
- Extend the non-destructive signed UI smoke to open Growth Watch, never pin or
  unpin anything.
- Run formatting, all Rust tests, UI-smoke tooling tests, privacy/pre-push
  guards, `make-app.sh`, signature proof, and minimum-window visual inspection.
