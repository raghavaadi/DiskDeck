# Folder Lens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an explicit local Folder Lens that maps one chosen or dropped folder with DiskDeck's live map and Search while never inheriting cleanup or offload authority.

**Architecture:** `volumes.rs` exposes one shared local-filesystem descriptor and a new `folder_lens.rs` owns target validation, identity, and the fixed system picker. `App` retains one isolated `FolderSession`, extends the existing map/search router to a third source, and ensures only one auxiliary External-or-Folder tree exists at a time.

**Tech Stack:** Rust 1.80+, egui 0.29 / eframe, rayon scanner, macOS `statfs`, fixed `/usr/bin/osascript`, AccessKit AppleScript smoke checks, POSIX shell verification.

## Global Constraints

- `scan::DATA_ROOT` remains `/System/Volumes/Data` and the only source for recommendations, cleanup, history, forecasting, leftovers, developer actions, and offload-source actions.
- Folder Lens is read-only: Open, Reveal, Quick Look, Search, and map navigation only; never Move to SSD, Trash, erase, restore, watch, or cleanup.
- Retain at most one auxiliary External-or-Folder tree in addition to Macintosh HD.
- Add no crate dependency, AppKit feature, background helper, telemetry, persisted path, favorite, exclusion, or network scan.
- The picker script is compile-time fixed; no UI path or script text enters a command argument.
- Preserve the 1180 × 740 minimum layout, native light/dark adaptation, and repaint only while real work is active.
- Use fixture-only filesystem mutation for tests and local visual QA; never scan or mutate an unapproved real folder.

---

## File map

- `src/volumes.rs` — shared local-filesystem `statfs` descriptor.
- `src/folder_lens.rs` — folder policy, identity revalidation, picker output parser, fixed picker process.
- `src/main.rs` — module registration only.
- `src/app.rs` — Folder session, auxiliary ownership, map/search/capacity routing, drop handling, and rail UI.
- `scripts/ui-smoke.applescript` — signed no-scan Folder Lens navigation proof.
- `scripts/test-ui-smoke.sh` — static safety contract preventing picker or storage action activation.
- `scripts/test-signed-ui.sh` — invokes the signed Folder Lens proof.
- `README.md`, `AGENTS.md` — public workflow and maintainer invariants.

---

### Task 1: Share local filesystem facts

**Files:**
- Modify: `src/volumes.rs:1-96`

**Interfaces:**
- Consumes: existing `statfs_info`, `eligible_mount`, and `MountedVolume` projection.
- Produces: `LocalFilesystem` and `inspect_local_filesystem(path: &Path) -> Option<LocalFilesystem>` for Folder Lens.

- [ ] **Step 1: Write failing descriptor tests**

Add tests proving the descriptor reports the temporary fixture's type,
capacity bounds, local status, and read-only flag consistently:

```rust
#[test]
fn local_filesystem_descriptor_reports_bounded_capacity() {
    let tmp = tempfile::tempdir().unwrap();
    let fs = inspect_local_filesystem(tmp.path()).unwrap();
    assert!(!fs.fs_type.is_empty());
    assert!(fs.total_bytes > 0);
    assert!(fs.free_bytes >= 0);
    assert!(fs.free_bytes <= fs.total_bytes);
    assert!(fs.local);
}

#[test]
fn mounted_volume_projection_uses_the_shared_descriptor() {
    let descriptor = LocalFilesystem {
        fs_type: "apfs".into(),
        total_bytes: 500,
        free_bytes: 125,
        read_only: false,
        local: true,
    };
    assert!(eligible_mount(
        &descriptor.fs_type,
        descriptor.local,
        false
    ));
}
```

- [ ] **Step 2: Run focused tests and verify failure**

```sh
~/.cargo/bin/cargo test volumes::tests::local_filesystem_ --locked
```

Expected: compilation fails because `LocalFilesystem` and
`inspect_local_filesystem` do not exist.

- [ ] **Step 3: Implement the shared descriptor**

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalFilesystem {
    pub fs_type: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub read_only: bool,
    pub local: bool,
}

pub fn inspect_local_filesystem(path: &Path) -> Option<LocalFilesystem> {
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let fs_type = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let block_size = stat.f_bsize as i64;
    Some(LocalFilesystem {
        fs_type,
        total_bytes: (stat.f_blocks as i64).saturating_mul(block_size),
        free_bytes: (stat.f_bavail as i64).saturating_mul(block_size),
        read_only: stat.f_flags & (libc::MNT_RDONLY as u32) != 0,
        local: stat.f_flags & (libc::MNT_LOCAL as u32) != 0,
    })
}
```

Remove `statfs_info` and have `inspect_mounted_volume` consume the descriptor.
Keep `eligible_mount` unchanged.

- [ ] **Step 4: Run volume and full tests**

```sh
~/.cargo/bin/cargo test volumes::tests --locked
~/.cargo/bin/cargo test --locked
```

Expected: all tests pass and mounted-volume behavior is unchanged.

- [ ] **Step 5: Commit the shared filesystem model**

```sh
git add src/volumes.rs
git commit -m "Share local filesystem inspection" -m "Give mounted drives and focused folder scans one interpretation of filesystem type, capacity, locality, and read-only state."
```

---

### Task 2: Validate folder targets and parse the fixed picker

**Files:**
- Create: `src/folder_lens.rs`
- Modify: `src/main.rs:1-24`

**Interfaces:**
- Consumes: Task 1 `LocalFilesystem`, `inspect_local_filesystem`, and `eligible_mount`.
- Produces: `FolderTarget`, `FolderBlock`, `inspect_folder`, `is_same_folder`, `parse_picker_output`, and `choose_folder`.

- [ ] **Step 1: Register the module and write failing policy/parser tests**

Add `mod folder_lens;` to `src/main.rs`, then create `src/folder_lens.rs` with
tests first. Use temporary real directories, files, and symlinks plus pure
injected descriptors:

```rust
#[test]
fn folder_policy_accepts_a_direct_local_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let target = inspect_folder(tmp.path()).unwrap();
    let metadata = std::fs::symlink_metadata(tmp.path()).unwrap();
    assert_eq!(target.path, tmp.path());
    assert_eq!(target.device_id, metadata.dev());
    assert_eq!(target.inode, metadata.ino());
    assert!(!target.fs_type.is_empty());
}

#[test]
fn folder_policy_rejects_missing_file_relative_symlink_and_whole_volume_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("file.txt");
    std::fs::write(&file, b"fixture").unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(tmp.path(), &link).unwrap();
    assert_eq!(inspect_folder(Path::new("relative")), Err(FolderBlock::NotAbsolute));
    assert_eq!(inspect_folder(&tmp.path().join("missing")), Err(FolderBlock::Missing));
    assert_eq!(inspect_folder(&file), Err(FolderBlock::NotDirectory));
    assert_eq!(inspect_folder(&link), Err(FolderBlock::Symlink));
    assert_eq!(classify_folder_shape(Path::new("/")), Err(FolderBlock::WholeVolume));
    assert_eq!(
        classify_folder_shape(Path::new("/System/Volumes/Data")),
        Err(FolderBlock::WholeVolume)
    );
    assert_eq!(
        classify_folder_shape(Path::new("/Volumes/Archive")),
        Err(FolderBlock::WholeVolume)
    );
}

#[test]
fn picker_parser_preserves_path_bytes_and_cancellation() {
    assert_eq!(parse_picker_output(b"CANCEL\n".to_vec()).unwrap(), None);
    assert_eq!(
        parse_picker_output(b"PATH:/tmp/Folder Name/\n".to_vec()).unwrap(),
        Some(PathBuf::from("/tmp/Folder Name/"))
    );
    assert_eq!(
        parse_picker_output(b"PATH:/tmp/line\nname/\n".to_vec()).unwrap(),
        Some(PathBuf::from("/tmp/line\nname/"))
    );
    assert!(parse_picker_output(b"PATH:\n".to_vec()).is_err());
    assert!(parse_picker_output(b"OTHER:/tmp\n".to_vec()).is_err());
}
```

Add separate tests for a symlink ancestor, dot segments using raw path bytes,
injected `smbfs`/non-local rejection, read-only local acceptance, and every
identity field mismatch.

- [ ] **Step 2: Run focused tests and verify failure**

```sh
~/.cargo/bin/cargo test folder_lens::tests --locked
```

Expected: compilation fails because the module API is absent.

- [ ] **Step 3: Implement target types and policy**

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderBlock {
    NotAbsolute,
    DotComponent,
    Missing,
    NotDirectory,
    Symlink,
    SymlinkAncestor,
    Network,
    WholeVolume,
    Unavailable,
}

pub fn is_same_folder(expected: &FolderTarget, current: &FolderTarget) -> bool {
    expected.path == current.path
        && expected.fs_type == current.fs_type
        && expected.device_id == current.device_id
        && expected.inode == current.inode
}
```

Implement `FolderBlock::message`, raw `/` segment validation, ancestor
`symlink_metadata`, `classify_folder_shape`, and `inspect_folder`. Project
capacity/read-only facts from `LocalFilesystem`; accept read-only local media
and reject `!eligible_mount(fs_type, local, false)`.

- [ ] **Step 4: Implement the fixed picker and byte parser**

```rust
const PICKER_SCRIPT: &str = r#"
try
    set chosenFolder to choose folder with prompt "Choose a folder for DiskDeck to inspect"
    return "PATH:" & POSIX path of chosenFolder
on error number -128
    return "CANCEL"
end try
"#;

pub fn parse_picker_output(mut bytes: Vec<u8>) -> Result<Option<PathBuf>, String> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes == b"CANCEL" {
        return Ok(None);
    }
    let path = bytes
        .strip_prefix(b"PATH:")
        .ok_or("folder picker returned an unsupported response")?;
    if path.is_empty() {
        return Err("folder picker returned an empty path".into());
    }
    Ok(Some(PathBuf::from(OsString::from_vec(path.to_vec()))))
}

pub fn choose_folder() -> Result<Option<PathBuf>, String> {
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", PICKER_SCRIPT])
        .output()
        .map_err(|error| format!("open folder chooser: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "folder chooser failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_picker_output(output.stdout)
}
```

- [ ] **Step 5: Run all tests**

Run:

```sh
~/.cargo/bin/cargo test folder_lens::tests --locked
~/.cargo/bin/cargo test --locked
```

Expected: all policy/parser tests and the full suite pass.

- [ ] **Step 6: Commit the folder boundary**

```sh
git add src/folder_lens.rs src/main.rs
git commit -m "Validate focused folder scans" -m "Accept only direct local folders, preserve picker path bytes, and revalidate filesystem and inode identity before any read-only action."
```

---

### Task 3: Isolate Folder sessions and auxiliary ownership

**Files:**
- Modify: `src/app.rs:175-340`
- Modify: `src/app.rs:430-520`
- Modify: `src/app.rs:780-1115`
- Modify: `src/app.rs:2390-3210`

**Interfaces:**
- Consumes: Task 2 folder policy/picker API and existing `ScanHandle`, `MapNavigation`, `ExternalSession`.
- Produces: `FolderSession`, auxiliary start gate, picker worker/polling, folder scan lifecycle, drop-cardinality helper, and identity invalidation. Map and rail variants arrive in Tasks 4–5 so every intermediate commit remains exhaustive and buildable.

- [ ] **Step 1: Write failing session and drop tests**

```rust
#[test]
fn auxiliary_scan_gate_allows_exactly_one_idle_read_only_session() {
    assert!(can_start_auxiliary_scan(false, false, false, false, false, false));
    assert!(!can_start_auxiliary_scan(true, false, false, false, false, false));
    assert!(!can_start_auxiliary_scan(false, true, false, false, false, false));
    assert!(!can_start_auxiliary_scan(false, false, true, false, false, false));
    assert!(!can_start_auxiliary_scan(false, false, false, true, false, false));
    assert!(!can_start_auxiliary_scan(false, false, false, false, true, false));
    assert!(!can_start_auxiliary_scan(false, false, false, false, false, true));
}

#[test]
fn folder_drop_requires_exactly_one_path() {
    assert!(folder_drop_path(&[]).is_err());
    assert!(folder_drop_path(&[Some(PathBuf::from("/a")), Some(PathBuf::from("/b"))]).is_err());
    assert!(folder_drop_path(&[None]).is_err());
    assert_eq!(
        folder_drop_path(&[Some(PathBuf::from("/fixture"))]).unwrap(),
        PathBuf::from("/fixture")
    );
}

```

Extend mutation and scan-start gates here; back-routing and map capabilities
are tested in Task 4 when the corresponding enum variants are introduced.

- [ ] **Step 2: Run focused tests and verify failure**

```sh
~/.cargo/bin/cargo test app::tests::folder_ --locked
~/.cargo/bin/cargo test app::tests::auxiliary_ --locked
```

Expected: compilation fails because Folder routing/session APIs do not exist.

- [ ] **Step 3: Add session state and the shared start gate**

```rust
struct FolderSession {
    target: FolderTarget,
    scan: ScanHandle,
    navigation: MapNavigation,
    revision: u64,
    disconnected: bool,
    completion_reported: bool,
}

fn can_start_auxiliary_scan(
    internal_scanning: bool,
    external_scanning: bool,
    folder_scanning: bool,
    picker_running: bool,
    mutation_busy: bool,
    file_review_running: bool,
) -> bool {
    !internal_scanning
        && !external_scanning
        && !folder_scanning
        && !picker_running
        && !mutation_busy
        && !file_review_running
}
```

Add fields `folder_picker_rx: Option<Receiver<Result<Option<PathBuf>, String>>>`,
`folder_session`, `folder_error`, and `folder_revision`, and update `App::new`.
Rename the external-only start gate to the auxiliary gate everywhere. Starting
External drops only a completed Folder session; starting Folder drops only a
completed External session. Do not add `MapSource::Folder` or
`RailView::Folder` in this task.

- [ ] **Step 4: Add picker and folder scan lifecycle**

Implement:

```rust
fn begin_folder_picker(&mut self)
fn poll_folder_picker(&mut self)
fn begin_folder_scan(&mut self, path: PathBuf)
fn folder_scanning(&self) -> bool
fn poll_folder_scan(&mut self)
fn refresh_folder_identity(&mut self) -> bool
```

The picker worker is named `folder-picker`; `begin_folder_scan` reinspects the
path immediately, increments revision, invalidates Search, clears a completed
External session, and calls only `start_scan(target.path.clone())`.
`refresh_folder_identity` updates capacity facts on a matching target and
fails closed on any path/filesystem/device/inode mismatch.

- [ ] **Step 5: Poll only explicit picker and scan work**

Call `poll_folder_picker` and `poll_folder_scan` from `update`. While
`folder_picker_rx.is_some()`, request a 250 ms repaint; while the folder scan
runs, use the existing 40 ms work repaint. Drag-and-drop is connected in Task
5 after the Folder rail exists.

- [ ] **Step 6: Run focused and full tests**

```sh
~/.cargo/bin/cargo test app::tests::folder_ --locked
~/.cargo/bin/cargo test app::tests::auxiliary_ --locked
~/.cargo/bin/cargo test --locked
```

Expected: all tests pass; no cleanup type consumes `FolderTarget`.

- [ ] **Step 7: Commit isolated session ownership**

```sh
git add src/app.rs
git commit -m "Isolate focused folder scan state" -m "Retain one bounded read-only Folder session, serialize it with external maps, and keep picker and target failures outside cleanup state."
```

---

### Task 4: Route Folder through map, Search, capacity, and top bar

**Files:**
- Modify: `src/app.rs:2050-2145`
- Modify: `src/app.rs:3500-4480`
- Modify: `src/app.rs:7820-8210`

**Interfaces:**
- Consumes: Task 3 `FolderSession`, lifecycle, and identity refresh.
- Produces: `RailView::Folder`, `MapSource::Folder`, Folder-aware top bar, capacity evidence, live map, navigation storage, Search root/actions, a minimal buildable rail shell, and fail-closed path action checks.

- [ ] **Step 1: Write failing routing and capacity tests**

```rust
#[test]
fn folder_map_capabilities_are_read_only() {
    assert!(!MapCapabilities::READ_ONLY.allow_offload);
}

#[test]
fn folder_capacity_copy_separates_volume_and_mapped_bytes() {
    assert_eq!(
        folder_capacity_detail(3_500_000_000, 42),
        "3.5 GB mapped in this folder · 42 items"
    );
}

#[test]
fn folder_navigation_never_changes_internal_breadcrumbs() {
    let root = storage_search_root();
    let folder = storage_search_child(&root, "Project", true, false);
    let tmp = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.folder_session = Some(FolderSession {
        target: inspect_folder(tmp.path()).unwrap(),
        scan: start_scan(tmp.path().to_path_buf()),
        navigation: MapNavigation::default(),
        revision: 1,
        disconnected: false,
        completion_reported: false,
    });
    assert!(app.open_search_folder(MapSource::Folder, &root, folder));
    assert!(app.crumbs.is_empty());
    assert_eq!(app.folder_session.as_ref().unwrap().navigation.crumbs.len(), 1);
}
```

- [ ] **Step 2: Run focused tests and verify failure**

```sh
~/.cargo/bin/cargo test app::tests::folder_map_ --locked
~/.cargo/bin/cargo test app::tests::folder_capacity_ --locked
~/.cargo/bin/cargo test app::tests::folder_navigation_ --locked
```

Expected: tests fail until Folder is exhaustively routed.

- [ ] **Step 3: Extend top bar and capacity evidence**

Add `RailView::Folder` and `MapSource::Folder`, route Folder back to Insights,
and add this minimal `draw_folder_lens` shell so the commit remains exhaustive:

```rust
fn draw_folder_lens(&mut self, ui: &mut egui::Ui, rect: Rect) {
    let palette = theme::palette(ui.ctx());
    let content = panel_chrome(
        ui,
        rect,
        "Folder Lens",
        Some(("local · read-only".into(), palette.faint)),
    );
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
        if ui.button("← Insights").clicked() {
            self.rail_view = RailView::Insights;
        }
    });
}
```

For an active Folder source, top
bar uses target name, scan state, Stop scan,
and Scan again. `draw_capacity` uses target `total_bytes/free_bytes` for the
gauge, title `<name> · containing volume`, and:

```rust
fn folder_capacity_detail(mapped_bytes: i64, files: i64) -> String {
    format!(
        "{} mapped in this folder · {} items",
        fmt_bytes(mapped_bytes),
        fmt_count(files)
    )
}
```

The capacity-only target fields refresh in the existing five-second tick.

- [ ] **Step 4: Extend live map and navigation storage**

Add a Folder arm in `draw_map` returning the Folder root, scan state, target
name, `MapCapabilities::READ_ONLY`, Folder navigation, and disconnected flag.
Add the same arm in `store_map_navigation` and `open_search_folder`. Folder
disconnect copy says `Folder unavailable\nChoose it again`.

Before map Reveal, use:

```rust
let path_actions_available = match source {
    MapSource::Internal => true,
    MapSource::External => self.external_path_actions_available(),
    MapSource::Folder => self.folder_path_actions_available(),
};
```

- [ ] **Step 5: Extend Search and Quick Look revalidation**

Add Folder root/revision checks to `open_storage_search` and
`draw_storage_search`. Before Open map, Quick Look, or Reveal from a Folder
dialog, call `folder_path_actions_available`; if it fails, close Search without
launching any process. Storage Search remains completed-tree-only.

- [ ] **Step 6: Run all map/search tests**

```sh
~/.cargo/bin/cargo test app::tests::folder_ --locked
~/.cargo/bin/cargo test search::tests --locked
~/.cargo/bin/cargo test --locked
```

Expected: every Folder action is read-only and all existing Internal/External
behavior remains green.

- [ ] **Step 7: Commit shared map behavior**

```sh
git add src/app.rs
git commit -m "Explore focused folders in the live map" -m "Route Folder Lens through map, Search, and capacity evidence while revalidating identity before every external process action."
```

---

### Task 5: Build the adaptive Folder Lens rail

**Files:**
- Modify: `src/app.rs:175-215`
- Modify: `src/app.rs:4450-5650`

**Interfaces:**
- Consumes: Tasks 3–4 lifecycle, routing, and target error copy.
- Produces: Insights entry, `draw_folder_lens`, idle/choosing/active/unavailable states, chooser button, drop zone, and Guide copy update.

- [ ] **Step 1: Write failing layout and action-copy tests**

```rust
#[test]
fn folder_lens_layout_keeps_navigation_body_and_safety_footer_separate() {
    let content = Rect::from_min_size(Pos2::ZERO, vec2(320.0, 600.0));
    let layout = folder_lens_layout(content);
    assert!(layout.nav.max.y < layout.body.min.y);
    assert!(layout.body.max.y < layout.footer.min.y);
    assert!(layout.body.height() > 400.0);
}

#[test]
fn folder_lens_action_copy_covers_idle_busy_running_complete_and_unavailable() {
    assert_eq!(folder_lens_action(None, false, true), ("Choose a folder…", true));
    assert_eq!(folder_lens_action(None, true, false), ("Choosing…", false));
    assert_eq!(folder_lens_action(Some(ScanState::Running), false, true), ("Stop scan", true));
    assert_eq!(folder_lens_action(Some(ScanState::Done), false, true), ("Scan again", true));
    assert_eq!(folder_lens_action(Some(ScanState::Done), false, false), ("Folder unavailable", false));
}
```

- [ ] **Step 2: Run focused tests and verify failure**

```sh
~/.cargo/bin/cargo test app::tests::folder_lens_ --locked
```

Expected: compilation fails because rail layout/action helpers are absent.

- [ ] **Step 3: Implement the rail**

Replace Task 4's minimal Folder renderer and add the first Insights group order:

```rust
(
    "External drives",
    external_detail,
    RailView::External,
),
(
    "Folder Lens",
    "Choose or drop one local folder for a read-only map".into(),
    RailView::Folder,
),
```

`draw_folder_lens` uses `panel_chrome("Folder Lens", "local · read-only")`,
a frameless `← Insights`, a visible `Choose a folder…` button, drop zone copy
`Drop one Finder folder here`, and selected-target state with path, mapped
items/bytes, Stop/Scan again, and `Choose another folder…`. Keep the safety
footer fixed:

`Folder Lens can inspect, open, reveal, and search. It cannot reclaim or move anything.`

Render errors inside the rail in caution color. Disable Choose with `Finish
current task` when the auxiliary gate is closed.

In `update`, clone `raw.dropped_files` only while `rail_view ==
RailView::Folder`, convert them to `Vec<Option<PathBuf>>`, call
`folder_drop_path`, and start only one valid target when the auxiliary gate is
open. Multiple or pathless drops set the exact local rail error and never
retain a target.

- [ ] **Step 4: Update Safety & Quick Start copy**

The Everyday Mac card becomes:

`Use Find or right-click the map to inspect storage. Folder Lens answers a focused folder question; Guided Reclaim builds a Safe-only plan.`

- [ ] **Step 5: Run focused and full tests**

```sh
~/.cargo/bin/cargo test app::tests::folder_lens_ --locked
~/.cargo/bin/cargo test --locked
```

Expected: all rail states fit the tested layout and no mutation action appears.

- [ ] **Step 6: Commit the user-facing rail**

```sh
git add src/app.rs
git commit -m "Add the Folder Lens workspace" -m "Make focused local scans discoverable through a chooser and Finder drop zone with explicit read-only capability copy."
```

---

### Task 6: Document and smoke-test Folder Lens

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-ui-smoke.sh`
- Modify: `scripts/test-signed-ui.sh`

**Interfaces:**
- Consumes: accessible names `Folder Lens`, `Choose a folder…`, `Drop one Finder folder here`, and the exact safety footer.
- Produces: `folder-lens-visible` no-scan signed smoke command and updated public/maintainer contract.

- [ ] **Step 1: Add a failing static smoke contract**

```sh
grep -q 'commandName is "folder-lens-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose folder-lens-visible"
grep -Fq 'button "Choose a folder…"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Folder Lens chooser"
grep -Fq 'static text "Drop one Finder folder here"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify drag-and-drop discoverability"
grep -Fq 'It cannot reclaim or move anything.' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify Folder Lens capability copy"
grep -q '^ui folder-lens-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open Folder Lens"
```

Extend the forbidden-click regular expression with `Choose a folder` so the
smoke cannot open the picker.

- [ ] **Step 2: Run static smoke and verify failure**

```sh
scripts/test-ui-smoke.sh
```

Expected: FAIL because the command is absent.

- [ ] **Step 3: Add no-scan signed navigation**

```applescript
else if commandName is "folder-lens-visible" then
    my openInsights(appGroup)
    if not (exists button "Folder Lens" of appGroup) then error "Folder Lens control is unavailable." number 1
    click button "Folder Lens" of appGroup
    delay 0.5
    if not (exists button "← Insights" of appGroup) then error "Folder Lens rail did not open." number 1
    if not (exists static text "Folder Lens" of appGroup) then error "Folder Lens heading is unavailable." number 1
    if not (exists button "Choose a folder…" of appGroup) then error "Folder chooser control is unavailable." number 1
    if not (exists static text "Drop one Finder folder here" of appGroup) then error "Folder drop guidance is unavailable." number 1
    if not (exists static text "Folder Lens can inspect, open, reveal, and search. It cannot reclaim or move anything." of appGroup) then error "Folder Lens safety boundary is unavailable." number 1
    return "PASS: Folder Lens available without choosing or scanning"
```

Add the command to usage and run `ui folder-lens-visible; ui escape` in the
signed suite.

- [ ] **Step 4: Update README and AGENTS**

Document chooser/drop flow, local-only policy, volume-honest capacity, Search,
one-auxiliary-map ownership, and complete absence from cleanup/history. Add
`folder_lens.rs` to architecture/test tables and add the fixture-only visual QA
rule to AGENTS.

- [ ] **Step 5: Run all repository guards**

```sh
scripts/test-ui-smoke.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
```

Expected: all commands pass.

- [ ] **Step 6: Commit docs and smoke coverage**

```sh
git add README.md AGENTS.md scripts/ui-smoke.applescript scripts/test-ui-smoke.sh scripts/test-signed-ui.sh
git commit -m "Document and smoke-test Folder Lens" -m "Publish the focused-scan boundary and prove the signed app exposes chooser and drop guidance without opening or scanning anything."
```

---

### Task 7: Signed fixture QA, release, merge, and exact CI

**Files:**
- Verify: `/Applications/DiskDeck.app`
- Verify: `dist/DiskDeck.zip`
- Fixture only: `${TMPDIR}/DiskDeck-Folder-Lens-QA`

**Interfaces:**
- Consumes: complete feature and tracked verification scripts.
- Produces: signed installed app, current package, four-state visual evidence, merged/pushed main, and exact green CI.

- [ ] **Step 1: Run full local gates**

```sh
~/.cargo/bin/cargo fmt -- --check
~/.cargo/bin/cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
```

Expected: zero failures; only the explicit reclaim-history fixture seeder is ignored.

- [ ] **Step 2: Build, sign, install, and package**

```sh
./make-app.sh
codesign --verify --deep --strict --verbose=2 /Applications/DiskDeck.app
unzip -t dist/DiskDeck.zip
```

Expected: signature and every archive member validate.

- [ ] **Step 3: Run the signed no-scan suite**

```sh
open -a /Applications/DiskDeck.app
scripts/test-signed-ui.sh
```

Expected: Folder Lens and all existing no-mutation checks pass.

- [ ] **Step 4: Create and select only a safe visual fixture**

Create `${TMPDIR}/DiskDeck-Folder-Lens-QA` with two nested directories, each
containing a 12 MB regular fixture file so both directories survive the 10 MB
post-scan compaction threshold. Open Folder Lens, click Choose, use
the system chooser's Go to Folder control to select that exact path, and wait
for `Scan complete`. Never select a real owner folder.

- [ ] **Step 5: Inspect required visual states**

Capture idle, choosing, scanning/complete, and invalidated-root states at
1480 × 920 and 1180 × 740 in both light and dark appearances. Confirm visible
Back, chooser, drop guidance, target identity, mapped evidence, Search, Stop or
Scan again, safety footer, no tofu, no clipping/overlap, and no cleanup or Move
to SSD action.

- [ ] **Step 6: Remove fixture and restore owner state**

Delete only the exact QA fixture after confirming its canonical path remains
beneath `${TMPDIR}`. Restore the original system appearance and DiskDeck
window bounds.

- [ ] **Step 7: Integrate, rebuild root package, and publish**

Fast-forward the verified feature branch into local `main`, rerun
`cargo test --locked`, run `./make-app.sh` from the main checkout, remove the
owned worktree/branch, and push `main`.

- [ ] **Step 8: Prove exact pushed CI and clean state**

```sh
head_sha=$(git rev-parse HEAD)
run_id=$(gh run list --repo raghavaadi/DiskDeck --branch main --limit 10 --json databaseId,headSha --jq '.[] | select(.headSha == "'"$head_sha"'") | .databaseId' | head -1)
gh run watch "$run_id" --repo raghavaadi/DiskDeck --exit-status
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
test -z "$(git status --short)"
```

Expected: exact `headSha` run succeeds, main matches origin, and the checkout is clean.
