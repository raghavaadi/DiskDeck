# DiskDeck Safety & Quick Start

**Date:** 2026-07-12
**Status:** approved under the owner's standing autonomous-maintainer direction

## Purpose

DiskDeck now has a broad set of safe storage tools, but the application does
not explain its overall workflow in one place. A new user must infer the
relationship between the live map, search, recommendations, the 900 ms hold,
Trash recovery, external-drive exploration, and the advanced Insights views.

Add a persistent, non-blocking **Safety & Quick Start** guide. It should make
the first useful path obvious to an everyday Mac user while giving developers
a direct route to the deeper evidence views. It must teach the real product,
not add a second onboarding-only workflow.

## Chosen approach

Use a permanent guide view instead of a first-run wizard or scattered
tooltips.

- A wizard is visible, but blocks the user and becomes stale as features grow.
- Tooltips help locally, but cannot explain the safety model or the end-to-end
  flow.
- A permanent guide stays discoverable, is useful after upgrades, and does not
  require persisted completion state.

The guide is the first entry in Insights and is also reachable from a small
**Guide** action in the reclaim summary. The toolbar remains unchanged so its
minimum-width layout is not made more crowded.

## Information architecture

Add `RailView::Guide`. It is a read-only rail workspace whose back target is
Insights. It never changes scan, cleanup, move, restore, monitor, or review
state merely by opening.

The view contains four sections in a vertically scrolling surface:

1. **The safe path** — Scan, explore, review, then hold to reclaim. It states
   that scanning and map navigation are read-only.
2. **Know what DiskDeck can change** — Safe versus Caution, Trash as the
   default recoverable action, and the checked-item plus 900 ms hold boundary.
3. **Everyday shortcuts** — Find storage, open/reveal through the right-click
   menu, Guided Reclaim, Reclaim History, and External drives.
4. **Developer path** — Developer Lens, Growth Watch, APFS accounting, and
   evidence/rebuild-cost language.

The screen ends with two context-sensitive actions:

- **Explore the map** returns to the reclaim summary. The map is already
  visible beside the rail; this action only restores the default rail.
- **Free up space** opens Guided Reclaim when recommendations are ready. While
  scanning, it is visibly disabled with honest copy.

The everyday and developer paths are labels, not persistent modes. DiskDeck
continues to be one application with one safety model.

## Visual design

Follow the existing Adaptive Native system:

- use `panel_chrome`, existing palette semantics, Inter display/body styles,
  and the current card radius/stroke language;
- cyan marks navigation and scan explanation, mint marks Safe/Trash recovery,
  amber marks Caution/re-download cost, and no red is used because the guide
  itself performs no dangerous action;
- keep the body scrollable and reserve a fixed action area so controls cannot
  overlap at the supported 1180 × 740 minimum;
- use numbered steps and short sentences rather than shortcut-heavy prose;
- render identically in structure under native light and dark appearances.

## Components and data flow

The implementation stays in `app.rs`; this is presentation and routing, not a
new subsystem.

- `RailView::Guide` participates in `rail_back_target`, Escape routing, and
  the main rail draw dispatch.
- A pure `guide_primary_action(recs_ready, scanning)` helper describes whether
  Guided Reclaim is enabled and supplies its visible label. Tests cover the
  ready and scanning states without requiring GUI input.
- `draw_safety_guide` renders the scrollable content and produces at most one
  navigation request after drawing. It does not start background work.
- The Insights entry routes to Guide. The reclaim-summary Guide action routes
  to the same view.
- Existing `begin_*` operations remain owned by their destination views; the
  guide only changes `rail_view`.

No settings, local files, telemetry, or migration are introduced.

## Safety and failure behavior

- Opening or leaving the guide never starts a scan or mutation.
- The reclaim action remains disabled until the existing recommendation set is
  built and the internal scan is not running.
- The guide does not select recommendations, bypass acknowledgement, or touch
  the 900 ms hold boundary.
- External-drive language remains read-only and does not expose Move to SSD
  for an external map.
- If the window is short, content scrolls; the fixed action area stays usable.
- Escape and the visible Back control return to Insights.

## Documentation

Update README features and Controls with the new persistent guide and its two
entry points. Update AGENTS architecture/conventions so future features keep
the guide accurate when user-visible workflows change. Extend the signed UI
smoke script to navigate Summary → Guide and Insights → Guide using labels,
then return without invoking a mutation.

## Verification

1. Unit tests prove Guide routing, Escape/back behavior, and primary-action
   readiness.
2. `cargo test --locked` passes.
3. `./make-app.sh` produces and installs the signed application and package.
4. The signed UI smoke test opens and leaves the guide without starting a
   cleanup.
5. Visual inspection covers 1480 × 920 and 1180 × 740 in both light and dark
   appearances. Text, cards, and action controls must not clip or overlap.
6. Package, privacy, and pre-push guards pass before publication.

## Non-goals

- No forced or automatic first-run modal.
- No persisted “completed onboarding” state.
- No tutorial animation, coach marks, or added crate dependency.
- No change to cleanup rules, default selection, Trash behavior, offload,
  restore, or Full Disk Access policy.
- No external documentation browser or network request.
