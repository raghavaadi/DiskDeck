# App Leftovers Implementation Plan

**Goal:** Surface conservative, evidence-backed large app sandbox leftovers
without proposing unrelated support data for deletion.

- Inspect only immediate children of the scanned user's `Library/Containers`.
- Require a bundle-identifier-shaped directory and at least 250 MB on disk.
- Search standard app roots plus Spotlight for the exact bundle identifier;
  uncertain or failed lookups are excluded, not treated as absent.
- Findings are Caution, never selected, and read-only in this slice. The only
  action is Reveal in Finder.
- Show the exact evidence: container bundle identifier, measured size, and no
  matching installed bundle found.
- Run analysis after the normal scan on a named worker; never add a background
  filesystem watcher or upload paths.
- Add fixture/parser tests, signed smoke, documentation, full gates, and
  minimum-window visual proof.
