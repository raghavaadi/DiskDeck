# DiskDeck Folder Lens

**Date:** 2026-07-12
**Status:** approved under the owner's standing autonomous-maintainer direction

## Purpose

DiskDeck can map Macintosh HD and mounted local volumes, but a user cannot yet
answer a narrower question such as “what is taking space inside this project,
Downloads folder, or local cloud cache?” without waiting for a whole-volume
scan and navigating from its root.

Add **Folder Lens**, an explicit read-only scan of one user-chosen local
folder. It reuses DiskDeck's live map, breadcrumbs, right-click inspection,
and completed-map Search while remaining completely outside cleanup,
recommendations, history, forecasting, developer analysis, and SSD offload.

## Chosen approach

Provide both familiar macOS entry paths:

1. **Choose a folder…** opens a native system folder chooser through one
   fixed AppleScript executed on a worker thread.
2. A single Finder folder can be dropped onto the Folder Lens rail.

The picker makes the feature discoverable for everyday users; drag-and-drop
keeps it fast for experienced users. No new crate dependency or expansion of
the narrowly configured AppKit dependency is required.

Alternatives were rejected for this slice:

- scan exclusions can make the main map and totals silently incomplete;
- persisted favorites add storage, migration, and stale-path behavior before
  the basic focused scan is proven;
- a text path field encourages typing mistakes and is not native Mac UX;
- cleanup actions in a custom scan would create a second destructive authority
  and are explicitly out of scope.

## Safety boundary

`scan::DATA_ROOT` remains `/System/Volumes/Data` and remains the only source
for rules, recommendations, Guided Reclaim, cleanup, Reclaim History, Growth
Watch, forecasting, app leftovers, Developer Lens, and offload-source actions.

A Folder Lens tree is a separate in-memory session with read-only
capabilities:

- map primary click may open a real directory;
- context menu may Open a directory or Reveal a real item in Finder;
- completed-map Search may open a mapped folder, Quick Look a file, or reveal
  it;
- Move to SSD, selection checkboxes, Trash, erase, cleanup commands, restore,
  watchlist, and developer actions are absent;
- no Folder Lens node or path is converted into a `Rec`, `CleanJob`,
  `OffloadJob`, history snapshot, receipt, or move record.

Opening the rail, opening the picker, or hovering a dropped folder never
starts a scan. Choosing or dropping one valid folder is the explicit scan
action.

## Folder target model

Create `folder_lens.rs` with a pure target policy and the fixed picker runner.

```rust
pub struct FolderTarget {
    pub name: String,
    pub path: PathBuf,
    pub fs_type: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub read_only: bool,
    pub device_id: u64,
    pub inode: u64,
}
```

`inspect_folder(path)` accepts only a direct, absolute, existing directory on
a local filesystem. It rejects:

- relative paths and paths containing `.` or `..` components;
- an exact symlink or any symlinked ancestor;
- missing paths and non-directories;
- network or synthetic filesystems rejected by the existing mounted-volume
  policy;
- `/`, `/System/Volumes/Data`, and a direct `/Volumes/<name>` root, because
  those already have dedicated whole-volume workflows.

User folders, hidden folders, packages, and subfolders of mounted local media
are valid because scanning is read-only and explicitly requested. Read-only
media is valid.

Expose the existing `statfs` result from `volumes.rs` as one reusable
`LocalFilesystem` descriptor so mounted-volume and folder policy share the
same local/network, type, capacity, and read-only interpretation.

The target records path, filesystem type, capacity, read-only state, device,
and inode. Before every Open, Reveal, Quick Look, or rescan—and during the
existing five-second status
refresh—DiskDeck repeats inspection and requires all identity fields to match.
A capacity-only change refreshes the displayed totals without invalidating the
folder identity. A missing, replaced, re-mounted, networked, or newly
symlinked root fails
closed, cancels an active scan, invalidates Search, and disables path actions.

## Folder picker

`folder_lens::choose_folder()` runs `/usr/bin/osascript` with one compile-time
AppleScript string. The script catches standard user cancellation and returns
one of two tagged records:

- `CANCEL`
- `PATH:` followed by the selected POSIX path

The parser removes exactly the one line terminator added by `osascript`; it
does not trim or split the path, so embedded whitespace and newline bytes are
not silently changed. An untagged, empty, non-zero, or malformed response is
an error. UI input is never interpolated into the script or any shell command.

The picker runs on a named worker and sends `Result<Option<PathBuf>, String>`
back to the app. Cancellation is a normal `None`, not an error or Activity
warning. While the picker is open, a second picker or auxiliary scan cannot
start.

## Auxiliary scan ownership

DiskDeck retains at most one auxiliary map in addition to Macintosh HD:

- starting Folder Lens drops a completed External drives session;
- starting External drives drops a completed Folder Lens session;
- neither may start while the other is scanning;
- neither may start during the internal scan, a mutation, or duplicate/file
  review;
- stopping a folder scan keeps its partial map visibly incomplete until it is
  replaced or scanned again.

This prevents two potentially large live trees from accumulating and keeps
the read-only capability boundary explicit.

## App architecture and routing

Add `RailView::Folder` beneath Insights and `MapSource::Folder` beside Internal
and External. `FolderSession` owns:

- `FolderTarget`;
- `ScanHandle`;
- its own `MapNavigation` breadcrumbs/view/zoom;
- a revision for Search invalidation;
- disconnected and completion-reported flags.

All map-source helpers become exhaustive over three sources. Folder uses the
same read-only `MapCapabilities` as External, but its identity revalidation is
independent. Internal navigation state remains untouched when a folder opens,
and leaving Folder Lens makes the internal map active again.

The top bar shows the selected folder name and Stop scan / Scan again while a
Folder session is active. Without a session, it continues to show Macintosh
HD; the Folder rail supplies the chooser.

The capacity card remains volume-honest: it labels the selected folder and
shows the containing local volume's total/free usage, while a supporting line
reports only the bytes and items currently mapped inside the folder. Folder
bytes never pretend to be the volume's used capacity.

## Folder Lens rail

Add the entry immediately after External drives in Insights. The rail uses the
existing Adaptive Native panel language and has three clear states:

1. **No folder:** a visible drop zone, “Choose a folder…” button, and plain
   explanation that the scan is read-only and local.
2. **Choosing:** button disabled with “Choosing…”; dropping is ignored until
   the system chooser finishes or is cancelled.
3. **Selected:** target name/path, Local/read-only badge, state and mapped
   counts, Stop or Scan again, and “Choose another folder…”.

Invalid drops and policy failures show one actionable local error in the rail.
Dropping multiple items says to drop exactly one folder. The app does not
silently select the first item.

When an internal or external scan, mutation, or file review makes an auxiliary
scan unavailable, Choose is disabled with “Finish current task”; a drop shows
the same explanation without opening a picker or retaining the path.

The rail's bottom safety note states: “Folder Lens can inspect, open, reveal,
and search. It cannot reclaim or move anything.”

Update Safety & Quick Start's everyday path to mention Folder Lens for focused
questions.

## Error handling

- Picker cancellation: return to the idle state without an error.
- Picker process failure or malformed output: show a retryable chooser error;
  do not start a scan.
- Missing, relative, symlinked, network, whole-volume, or non-directory target:
  show the exact policy reason; do not retain a session.
- Target changes after selection: mark “Folder unavailable,” cancel the scan,
  invalidate Search, and disable path actions.
- Scan denial inside the chosen root: retain normal per-node No access evidence
  without escalating privileges.
- Worker/channel disconnect: clear picker-busy state and show a retryable
  error.

## Testing and verification

Pure tests cover:

- local-directory acceptance plus exact path/filesystem/device/inode capture;
- missing, file, relative, dot-component, exact-symlink, symlink-ancestor,
  network, `/`, data-root, and direct-volume-root rejection;
- read-only local target acceptance;
- identity mismatch on path, filesystem, device, inode, and file kind;
- picker parsing for cancel, spaces, Unicode, embedded newlines, a single
  output terminator, malformed tags, and empty paths;
- one-auxiliary-session start gates and source routing;
- Folder navigation/search isolation and read-only map capabilities;
- dropped-file cardinality and missing-path behavior;
- capacity copy distinguishing containing-volume usage from mapped folder
  bytes.

Release verification requires:

1. `cargo test --locked` and all repository guards pass.
2. `./make-app.sh`, signature verification, and package verification pass.
3. Signed AccessKit smoke opens Folder Lens from Insights and verifies the
   chooser/drop/safety copy without opening the picker or scanning a real
   folder.
4. A fixture folder created under a temporary directory is selected through
   the system chooser for local visual QA; only that fixture is scanned.
5. Empty, scanning, complete, and invalidated states are inspected at 1480 ×
   920 and 1180 × 740 under both light and dark appearances.
6. The fixture is deleted, the owner's appearance/window are restored, the
   exact pushed commit receives green GitHub CI, and the worktree is clean.

## Documentation

Update README features, Controls, module/test tables, and privacy boundary.
Update AGENTS architecture, auxiliary-memory rule, cleanup authority, and
visual QA instructions. Extend the tracked signed UI AppleScript and its
static safety contract without clicking Choose, Open, Reveal, Quick Look,
Move, scan, or cleanup controls.

## Non-goals

- No cleanup, offload, restore, recommendation, or watch action from Folder
  Lens.
- No network, cloud-account, iCloud-quota, or remote scan.
- No persisted favorite folders, recents, exclusions, or bookmarks.
- No simultaneous external and folder maps.
- No administrator/root scan.
- No arbitrary typed path or AppleScript supplied by the user.
- No new crate dependency or AppKit feature expansion.
