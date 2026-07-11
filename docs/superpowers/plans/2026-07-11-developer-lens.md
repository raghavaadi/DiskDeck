# Developer Lens Implementation Plan

**Goal:** Add a read-only, opt-in explanation of developer storage using the
same deterministic findings already produced by DiskDeck's safety knowledge
base.

## Contract

- The default summary is unchanged; Developer Lens opens only on request.
- No path, project name, or measurement leaves the Mac.
- The lens does not rescan, change tiers, preselect Caution findings, or create
  new cleanup commands.
- Totals come only from existing non-overlapping recommendation records, so the
  lens cannot imply more reclaimable space than the plan contains.

## Work

1. Add a pure classifier that groups findings into Containers, Apple
   Development, JavaScript Projects, Package Stores, and Build Tooling.
2. Test group membership, totals, stable ordering, Caution counts, and
   exclusion of ordinary browser/log/trash findings.
3. Add a mutually exclusive Developer Lens rail with bounded group cards,
   plain-language rebuild cost, target counts, and paths.
4. Add non-destructive signed-smoke navigation; forbid the smoke script from
   clicking any reclaim, move, restore, or selection action.
5. Document, run all local CI/privacy gates, build/sign/install, and verify the
   minimum-window rail visually before merging.
