# Storage Search — design

**Date:** 2026-07-12
**Status:** delivered and verified locally under the owner's standing direction

## Why this is next

DiskDeck can visualize, explain, reclaim, forecast, and restore storage, but a
user who already knows part of a name—`Downloads`, `node_modules`, `.dmg`, or a
project name—still has to drill the map manually. Search is a primary analysis
gesture in mature disk visualizers and serves both intended audiences: everyday
Mac users locating a large download and developers locating a build tree.

The feature is **Storage Search**: an instant, read-only search over the exact
folder and large-file nodes retained by the completed DiskDeck map. It does not
start another filesystem walk, query Spotlight, upload paths, or expand the
scanner's materialization thresholds.

## Approaches considered

1. **Search the completed in-memory map — selected.** Results agree with the
   map's sparse-aware on-disk measurements, arrive without I/O, need no new
   permission, and can navigate directly into retained folder nodes. The limit
   is explicit: small items already folded into aggregates are not individually
   searchable.
2. **Use Spotlight metadata.** This can find more names quickly, but results can
   be stale, excluded, cloud-only, or based on logical rather than on-disk size.
   It would create two conflicting notions of what DiskDeck mapped.
3. **Run a second full-name traversal.** This can be exhaustive, but repeats
   expensive filesystem work, complicates cancellation and permissions, and
   violates the product rule that heavy work must be visible and user initiated.

## Product experience

### Entry points and availability

The Storage map breadcrumb row gains a compact **Find** control with a `⌘F`
hint. Command-F opens the same surface from anywhere in the main window unless
another confirmation dialog is active. Search becomes available only after a
foreground scan completes and compaction is finished. During a scan the control
explains “Available when this map completes” rather than starting hidden work.

Opening search presents one focused, adaptive-native dialog over the existing
workspace. The text field receives focus immediately. Escape closes search
before it performs rail or map Back navigation. Closing or clearing search does
not change the current map location.

### Honest scope and query behavior

The dialog states: “Searches folders and large files retained in this completed
map. Small items remain grouped.” This is the user-facing form of the existing
10 MB directory and 100 MB file materialization thresholds; the UI does not
claim to search every filename on disk.

A query is trimmed, case-insensitive, and split into whitespace-delimited terms.
Every term must occur somewhere in the display path. One-character queries show
guidance rather than traversing the tree. Results update locally after each edit
and are capped at 80 rows while still reporting the full match count.

Ranking is deterministic:

1. exact basename match;
2. basename prefix match;
3. basename substring match;
4. path-only match;
5. larger on-disk usage;
6. raw path bytes as the final stable tie-breaker.

Each result shows its name, shortened display path, sparse-aware on-disk size,
kind, and a visible **No access** state where applicable. Search never sums
results or describes them as additional reclaimable space because ancestors and
descendants overlap.

### Result actions

- A readable folder exposes **Open map**. It closes search, rebuilds the
  breadcrumb chain from the retained parent links, and opens that folder as the
  current map view.
- A retained large file exposes **Quick Look** and **Reveal in Finder**.
- A folder also exposes **Reveal in Finder**.
- A denied folder exposes only **Reveal in Finder** and never pretends its
  contents were scanned.
- Enter activates the selected result's primary action. Up/Down change the selected
  row without moving focus out of the search surface.

No result exposes Trash, permanent erase, cleanup selection, a command, SSD
offload, or restore. Those actions retain their existing vetted entry points.

## Architecture

Create a focused `search.rs` module with no persistence and no worker:

- `search_tree(root, query, limit)` snapshots child lists through `Node::kids`,
  visits each retained node once, computes deterministic match ranks, and
  returns a `SearchSummary { total_matches, results }`;
- `SearchResult` holds the retained `Arc<Node>` plus its rank and display path;
- `crumbs_for(node)` walks weak parent links to produce the exact root-to-node
  breadcrumb chain and fails closed if the chain is detached or cyclic;
- path matching uses the existing lossy display representation because search
  is a convenience view, while the action continues to carry the original
  `PathBuf` and therefore preserves raw macOS path bytes.

The application owns ephemeral `SearchDialog` state: query, current results,
selected index, and a boolean requesting initial field focus. Results are
recomputed only when the query changes or a new scan replaces the root. They
are dropped at the start of every scan so no stale node can be opened after the
map changes.

The completed compact tree is bounded enough for synchronous in-memory search;
the un-compacted live tree is never searched. Search records no timing, query,
telemetry, or history. If real measurements later disprove the synchronous
assumption, the pure module can move behind a cancellable worker without
changing the UI or result contract.

## State and failure behavior

- No scan: Find explains that a scan is required and offers no implicit scan.
- Scan running or aborted: Find stays disabled with honest copy.
- Empty or one-character query: guidance, no traversal.
- No matches: “No mapped folder or large file matches this search.”
- More than 80 matches: render the best 80 and state the total.
- Detached/cyclic ancestry: keep the result revealable but disable Open map.
- A result disappears after the completed scan due to external filesystem
  activity: Finder/Quick Look handles the missing path normally; DiskDeck does
  not mutate or rescan automatically.
- Starting a new scan closes search and drops every retained result before the
  scan root changes.

## Accessibility and layout

The Find control, dialog heading, search field, count, row names, kinds, sizes,
selection, and action buttons use semantic widgets rather than painted-only
text. Keyboard focus remains inside the dialog while it is open. Row layout
reserves a fixed action column so long project names and paths elide before the
buttons, never underneath them. The dialog fits the app's 1180 × 740 minimum window
and follows the existing light/dark palette and Inter/Hack font roles.

## Verification

### Pure and application tests

- exact, prefix, basename-substring, and path-only ranking;
- multi-term AND matching, whitespace trimming, case folding, one-character
  rejection, deterministic raw-path tie-break, result cap, and total count;
- sparse-aware bytes and denied state pass through unchanged;
- breadcrumb reconstruction covers root, nested folder, detached parent, and a
  defensive cycle bound;
- new scan invalidation clears the query, results, selection, and dialog;
- Escape closes search before rail/map navigation;
- Command-F is blocked by confirmation dialogs and available only for a
  completed map;
- minimum-width result geometry keeps content and action columns disjoint;
- no search result action maps to cleanup, erase, command, offload, or restore.

### Signed application proof

Build only with `./make-app.sh`. Inspect empty, no-match, populated, capped, and
long-path search states in light and dark appearance at minimum and typical
window sizes. Verify Command-F focus, Up/Down selection, Enter folder opening,
Escape priority, Quick Look, and Finder reveal using only read-only paths.

Extend signed AppleScript smoke to open Storage Search and verify its heading,
field, scope copy, and Escape closure. The checked-in smoke must not type a path
that triggers an action, press Enter, click a result action, start a scan, or
touch any cleanup/move/restore control.

Run formatting, every shell guard, `cargo test --locked`, distribution ZIP
validation, signature validation, signed UI smoke, and the exact GitHub CI run
before declaring the slice shipped.

### Delivered proof

- `163` active Rust tests pass; the sole ignored test remains the explicit
  reclaim-history signed-visual fixture seeder.
- Pure tests cover exact/prefix/name/path ranking, multi-term matching, the
  80-row cap with full count, raw-path tie-breaks, denied nodes, attached
  breadcrumbs, detached roots, and defensive cycle refusal. Application tests
  cover availability, complete invalidation, Escape priority, read-only action
  classification, fixed row geometry, exact folder navigation, and modal map
  interaction blocking.
- The signed app was inspected at 1180 × 740 and 1480 × 952 in light and dark
  appearance. Empty, one-character, no-match, populated, capped (`80 of 607`),
  and long-path states were inspected without overlap, tofu, hidden controls,
  or background tooltip leakage.
- Command-F focus, Up/Down selection, Enter folder opening, Escape closure,
  Quick Look of an existing retained large file, and Finder reveal were proven
  on the signed build. Those actions were read-only; no path was created,
  moved, renamed, deleted, or selected for cleanup.
- Signed smoke opens the empty search surface, verifies its semantic heading,
  field, and scope copy, then closes it without typing or activating a result.
  The harness polls the opened heading because egui AccessKit exposes the
  button but omits a Boolean `AXEnabled` value.
- QA created no fixture or persisted search data. The owner's dark appearance,
  1480 × 952 window size, original window position, Data-root map, and Summary
  rail were restored afterward.

## Documentation and non-goals

README explains the mapped-node scope, Command-F, ranking, and read-only result
actions. AGENTS records the no-second-walk rule, threshold honesty, stale-root
invalidation, raw-path action boundary, and non-mutating smoke contract.

This slice adds no account, network request, Spotlight query, dependency,
background scan, persisted search history, arbitrary command, new cleanup rule,
or destructive action. Search does not change scanner thresholds, APFS
accounting, cloud behavior, or external-volume scope.
