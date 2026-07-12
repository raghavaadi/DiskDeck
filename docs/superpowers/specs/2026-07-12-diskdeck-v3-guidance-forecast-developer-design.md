# DiskDeck v3: Guided Reclaim, Forecasting, and Developer Deep Dive

**Status:** Phases 1–3 shipped

**Date:** 2026-07-12

**Delivery order:** Guided Reclaim, then Storage Forecasting, then Developer Deep Dive

## Product intent

DiskDeck v3 serves two audiences without splitting into two applications. The
default experience helps an everyday Mac user answer plain questions such as
“How can I safely free 20 GB?” and “What keeps filling my Mac?” Developer Lens
adds technical evidence and developer-specific storage analysis without making
the default interface feel like an operations console.

The release remains local-first, deterministic, and conservative. DiskDeck does
not upload paths or scan history, silently clean files, run background full-disk
scans, or turn UI-provided paths into destructive commands.

## Product principles

1. **A goal before a list.** Everyday users choose an outcome; DiskDeck explains
   a bounded plan rather than presenting an undifferentiated wall of folders.
2. **Recommendations are evidence, not authority.** Every recommendation shows
   what it is, what will happen, and whether recovery is possible.
3. **Two depths, one safety model.** Developer Lens exposes technical detail but
   cannot bypass selection, acknowledgement, the hold gesture, or vetted rules.
4. **Honest uncertainty.** Forecasts and estimated sizes carry visible
   confidence. Missing evidence produces “not enough history,” not a guess.
5. **No hidden heavy work.** Full scans remain user initiated. The optional
   menu-bar monitor continues to use low-frequency capacity checks only.

## Phase 1: Guided Reclaim — shipped

### User experience

The Summary gains a primary **Free up space** entry point. The user chooses a
suggested goal such as 10 GB, 20 GB, or 50 GB, or enters a custom goal bounded by
the volume's currently used space. DiskDeck may therefore receive a goal larger
than its Safe pool and must present the resulting shortfall honestly. It then
presents a proposed plan in priority order.

The initial plan includes only `Safe` findings and may preselect those findings
under the existing tier policy. `Caution` findings appear in a separate
“More options” section and remain unchecked. DiskDeck never includes user
documents, code, media, app leftovers, duplicate files, or large-old files in
the automatic plan.

Each row answers four questions:

- What is this storage?
- Why is it safe or why does it need review?
- How much space is measured or estimated?
- What recovery or regeneration cost should the user expect?

The running total makes the goal visible. If safe findings cannot meet the goal,
DiskDeck says so and offers optional Caution findings; it does not weaken the
safety policy to reach the number. Technical paths and commands stay behind a
**Show details** disclosure.

The final confirmation reuses the existing checked-selection, acknowledgement,
and 900 ms hold boundaries. Completion compares planned and actual outcomes,
reports partial failures per item, and links to Trash or Moved Items when a
recovery path exists.

### Architecture

A new pure planning module receives immutable cleanup findings, a requested byte
goal, and the scan generation that produced those findings. It returns ordered
finding identifiers plus a summary explaining whether the goal can be met. It
never creates commands or paths and never performs filesystem work.

Ordering is deterministic:

1. measured `Safe` findings before estimated findings;
2. larger recoveries before smaller recoveries within the same evidence class;
3. stable finding identifier as the final tie-breaker.

The application resolves identifiers back to the original `Rec` values. Cleanup
continues exclusively through `clean::run_clean`, preserving the invariant that
command recommendations execute only the vetted command string stored in
`rules.rs`.

### Failure behavior

- A stale or missing scan disables plan creation and offers **Scan now**.
- If findings change before confirmation, the plan is recomputed and the user
  must confirm again.
- If measured space is below the requested goal, the shortfall is explicit.
- Individual cleanup failures do not mark the entire goal as achieved.
- Estimated bytes are never reported as exact recovered bytes.

### Phase 1 acceptance

- A user can request a target and understand the proposed plan without knowing
  filesystem terminology.
- Only Safe findings are selected automatically.
- The planner cannot introduce new paths, commands, or destructive actions.
- Existing checkbox, acknowledgement, and hold requirements remain intact.
- Minimum-window and Activity-drawer layouts have no overlap or hidden action.
- Unit tests cover deterministic ordering, shortfalls, estimates, stale plans,
  and tier enforcement; signed-app smoke testing covers the complete flow
  without cleaning owner data.

## Phase 2: Storage Forecasting — shipped

### User experience

Growth Watch gains a plain-language forecast answering when the configured
low-space threshold may be reached. Forecasts appear only after enough compatible
completed scans exist. Until then, DiskDeck shows exactly what evidence is
missing and offers **Scan now**.

The view separates:

- recurring growers with sustained net growth;
- temporary spikes that later shrank;
- newly large folders without enough history for a trend;
- the whole-volume free-space trajectory.

Confidence uses words rather than false precision:

- **Early estimate:** at least three compatible scans spanning seven days;
- **Developing estimate:** at least five scans spanning fourteen days;
- **Reliable estimate:** at least eight scans spanning thirty days.

The headline uses a range such as “about 5–7 weeks” rather than a precise date.
If recent behavior is flat, volatile, or improving, DiskDeck says that instead
of predicting exhaustion.

### Architecture and data flow

A forecasting module consumes the bounded snapshots already owned by
`history.rs`. It operates on completed, compatible snapshots only and stores no
second copy of history. A robust rate is derived from interval changes, using a
median rather than a single first-to-last slope so one cleanup or download spike
cannot dominate the result.

Forecasting is read-only. The optional menu monitor may display an already
computed forecast summary, but its five-minute `statfs` loop does not open scan
history, traverse the disk, or start the application scanner. A fresh forecast
is produced only when DiskDeck records a completed foreground scan.

### Failure behavior

- Corrupt and incompatible snapshots remain skipped under existing retention
  rules.
- Clock reversal, zero-length intervals, and impossible capacity values exclude
  the affected interval.
- Too little history, excessive volatility, or non-positive growth produces no
  time-to-low estimate.
- Forecasts never classify predicted bytes as currently reclaimable.

### Phase 2 acceptance

- No forecast appears from one scan or from less than seven days of evidence.
- Temporary spikes do not become recurring-growth warnings.
- Forecast confidence and data span are visible.
- No background full scan is introduced.
- Tests cover confidence thresholds, median-rate behavior, cleanup spikes,
  volatility, incompatible history, and non-growing disks.
- The signed application exposes both forecast surfaces through accessibility
  and keeps them readable and non-overlapping at minimum and typical window
  sizes in light and dark appearances.

## Phase 3: Developer Deep Dive — shipped

### User experience

Developer Lens becomes an opt-in workspace organized by source rather than a
flat list of folders.

**Docker** separates the locally measurable VM footprint from Docker-reported
images, containers, volumes, and build cache when fixed read-only commands are
available. DiskDeck distinguishes “inside Docker” values from the on-disk VM
size and does not add overlapping numbers together.

**Xcode** separates DerivedData, archives, device support, simulator runtimes,
simulator devices, and caches. Active or ambiguous simulator data remains
review-only.

**Projects** groups `node_modules`, Rust `target`, Python environments, build
outputs, and related findings beneath a bounded project root. Project roots are
identified only from findings already measured by DiskDeck and nearby standard
project markers; the feature does not begin another unbounded filesystem walk.

Each finding carries a rebuild-cost label:

- **Quick regeneration** for local derived output;
- **Large download** for toolchains, runtimes, or dependency caches;
- **Manual setup** when recreation is not automatic or evidence is incomplete.

An evidence disclosure shows measured paths, sizes, overlap handling, commands,
and the reason for the assigned safety tier. Everyday Summary continues to show
only a compact Developer Lens entry point.

### Safety and command boundaries

Developer grouping is read-only presentation logic. It cannot create a cleanup
rule from a discovered path. Any cleanup action must map to an existing vetted
`Rec`; command findings remain locked to `Action::Command` and use only their
fixed `rules.rs` command. Unknown project output is reveal-only until a separate
review adds a conservative rule and tests.

No plugin or third-party rule API is included in v3. An extension system should
follow only after real contributor use cases establish a capability and trust
model.

### Failure behavior

- Missing Docker or Xcode tools omit their command-derived details without
  hiding filesystem measurements.
- Command timeouts display unavailable sections and never block the main UI.
- A project with ambiguous ownership remains an ungrouped finding.
- Overlapping measurements name their source and are never summed twice.

### Phase 3 acceptance

- Developer Lens remains opt-in and does not complicate first-run Summary.
- Docker, Xcode, and project totals have explicit overlap semantics.
- No discovered path becomes executable cleanup input.
- Tests cover project grouping, ambiguity, overlap removal, rebuild-cost labels,
  fixed-command failures, and deterministic ordering.
- Signed-app visual verification covers loading plus a populated mixed report
  with a partial Docker measurement at minimum and typical window sizes in
  light and dark appearances. Deterministic tests cover empty, partial,
  unavailable, and worker-error state copy; visual error injection stops at the
  host privacy boundary rather than changing the owner's macOS permissions.
- The shipped worker starts only after the user opens Developer Lens, caps
  candidate/marker inspection, and keeps every discovered path reveal-only.
  The signed populated proof includes an unavailable on-disk Docker footprint
  alongside live uncounted inside-VM detail.

## Shared interface decisions

The interface keeps the existing adaptive native visual direction and semantic
colors. Everyday text uses plain verbs and outcomes; technical nomenclature is
progressively disclosed. Every new detail view uses the established Insights
navigation and visible Back behavior. Empty states explain why no result exists
and what the user can safely do next.

Accessibility work is part of each phase: keyboard navigation, readable focus,
VoiceOver labels for controls and confidence, non-color status indicators, and
layout checks at minimum window size.

## Privacy and persistence

All planning and forecasting data stays on the Mac. The reclaim goal is
ephemeral UI state unless the user explicitly repeats it; it is not telemetry.
Forecasting reuses the existing bounded local snapshot retention. Developer
grouping is recomputed from local findings and does not create a permanent
project-name database. No phase introduces analytics, cloud sync, or path
uploading.

## Delivery strategy

Each phase ships as an independent, reversible slice on `main`:

1. committed implementation plan and focused tests;
2. local format, privacy, identity, and Rust gates;
3. signed app build and eyes-on-screen verification;
4. green GitHub CI on the pushed revision;
5. README and maintainer documentation updated to match shipped behavior.

Phase 2 begins only after Phase 1 is usable in the signed app. Phase 3 begins
only after forecasts are honest under sparse and volatile history. This keeps
DiskDeck useful after every release and prevents a broad v3 rewrite from
weakening its safety model.
