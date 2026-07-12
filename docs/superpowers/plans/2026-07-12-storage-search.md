# Storage Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an instant, local, read-only Command-F search over the folders and large files retained by a completed DiskDeck map.

**Architecture:** A new pure `search.rs` module traverses only the compacted `Arc<Node>` tree, ranks bounded results deterministically, and reconstructs safe breadcrumb chains. `app.rs` owns ephemeral dialog state, keyboard behavior, and read-only Open map, Quick Look, and Finder actions; every new scan drops all search state before replacing the root.

**Tech Stack:** Rust 2021, egui/eframe 0.29, existing `Arc<Node>` scan tree, macOS Finder and `/usr/bin/qlmanage`, AppleScript/Swift signed-UI smoke tooling; no new crate dependencies.

## Global Constraints

- Search only a `ScanState::Done` compacted tree; never search the live or aborted tree.
- Preserve `KEEP_DIR_BYTES = 10 MiB` and `KEEP_FILE_BYTES = 100 MiB`; state honestly that small items remain grouped.
- Do not query Spotlight, start a second traversal, access the network, persist queries/results, or add telemetry.
- Result actions carry the original `PathBuf`; lossy path text is display/matching convenience only.
- Search exposes only Open map, Quick Look, and Reveal in Finder—never cleanup, erase, command, offload, or restore.
- A new scan closes search and drops every `Arc<Node>` result before the root changes.
- Match the Adaptive Native light/dark palette and fit the app's 1180 × 740 minimum window.
- Signed smoke may open and close search but must not type a query, press Enter, or activate a result action.

---

### Task 1: Pure compact-tree search and breadcrumb reconstruction

**Files:**
- Create: `src/search.rs`
- Modify: `src/main.rs`
- Test: `src/search.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::scan::Node`, `Node::kids()`, `Node::bytes()`, raw macOS path bytes through `std::os::unix::ffi::OsStrExt`.
- Produces: `pub const DEFAULT_RESULT_LIMIT: usize = 80`, `pub struct SearchResult { pub node: Arc<Node>, pub display_path: String }`, `pub struct SearchSummary { pub total_matches: usize, pub results: Vec<SearchResult> }`, `pub fn search_tree(root: &Arc<Node>, query: &str, limit: usize) -> SearchSummary`, and `pub fn crumbs_for(root: &Arc<Node>, target: &Arc<Node>) -> Option<Vec<Arc<Node>>>`.

- [ ] **Step 1: Create ranking and breadcrumb tests that fail before the module exists**

Add test-only `root()` and `child()` constructors that initialize every public `Node` field, connect children with `Arc::downgrade`, and assign deterministic bytes. Cover this exact fixture:

```rust
let root = root("/System/Volumes/Data");
let users = child(&root, "Users", true, 40_000_000_000, false);
let project = child(&users, "WardenUI", true, 5_000_000_000, false);
let exact = child(&project, "node_modules", true, 2_000_000_000, false);
let prefix = child(&project, "node_modules-old", true, 3_000_000_000, false);
let nested = child(&users, "archive-node_modules-copy", true, 4_000_000_000, false);
let path_only = child(&exact, "cache", true, 1_000_000_000, false);
```

Assertions must prove: query `node_modules` orders `exact`, `prefix`, `nested`, then `path_only`; `WARDEN node` requires both terms; a one-character query and whitespace-only query return zero; `limit = 2` retains two rows but reports four total; a denied node passes through; equal-rank/equal-size rows sort by raw path bytes; root is excluded; `crumbs_for(root, exact)` returns `Users/WardenUI/node_modules`; root returns an empty chain; an unrelated root returns `None`.

- [ ] **Step 2: Run the focused test and verify the expected red state**

Run: `cargo test search::tests --locked`

Expected: FAIL because `mod search` and the search interfaces do not exist.

- [ ] **Step 3: Implement the minimal pure module**

Implement a private `MatchRank` ordered from exact basename through path-only. Normalize with `trim().to_lowercase()`, split on whitespace, and return an empty summary if the normalized query contains fewer than two Unicode scalar values. Visit descendants only, snapshotting each child list through `kids()`. Require every term in the lowercased display path. Sort with this exact key order:

```rust
left.rank
    .cmp(&right.rank)
    .then_with(|| right.node.bytes().cmp(&left.node.bytes()))
    .then_with(|| {
        left.node.path.as_os_str().as_bytes()
            .cmp(right.node.path.as_os_str().as_bytes())
    })
```

Compute `total_matches` before truncating to `limit.min(DEFAULT_RESULT_LIMIT)`. For `crumbs_for`, walk weak parents with a 4096-hop bound, stop only at `Arc::ptr_eq(current, root)`, reverse the descendants, and return `None` if a parent is missing or the bound is exhausted.

Add `mod search;` to `src/main.rs`.

- [ ] **Step 4: Run the focused and scanner tests**

Run: `cargo test search::tests --locked && cargo test scan::tests --locked`

Expected: all new search tests and existing scanner tests PASS.

- [ ] **Step 5: Commit the pure engine**

```bash
git add src/search.rs src/main.rs
git commit -m "Search the completed storage map"
```

---

### Task 2: Ephemeral application state, availability, and invalidation

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 1's `SearchSummary`, `SearchResult`, `search_tree`, and `crumbs_for`.
- Produces: private `SearchDialog { query: String, summary: SearchSummary, selected: usize, request_focus: bool, scan_revision: u64 }`; `can_open_storage_search(scan_state, confirmation_open)`; and `invalidate_storage_search(&mut Option<SearchDialog>)`.

- [ ] **Step 1: Add failing state-machine tests**

Add tests proving:

```rust
assert!(can_open_storage_search(Some(ScanState::Done), false));
assert!(!can_open_storage_search(None, false));
assert!(!can_open_storage_search(Some(ScanState::Running), false));
assert!(!can_open_storage_search(Some(ScanState::Aborted), false));
assert!(!can_open_storage_search(Some(ScanState::Done), true));
```

Create a dialog containing a non-empty query, one result, selected index, and revision; call `invalidate_storage_search`; assert it becomes `None`. Extend the existing Escape-priority test so an open search is consumed before rail Back or map Back. Add a small `SearchAction` enum test proving it contains only `OpenMap`, `QuickLook`, and `Reveal` variants.

- [ ] **Step 2: Run the focused application tests and verify red**

Run: `cargo test app::tests::storage_search --locked`

Expected: FAIL because the state and helpers are absent.

- [ ] **Step 3: Implement state and keyboard boundaries**

Import the Task 1 interfaces. Add `search_dialog: Option<SearchDialog>` to `App`, initialize it to `None`, and clear it at the very beginning of `begin_scan()` before assigning a new `ScanHandle`.

Add:

```rust
fn confirmation_dialog_open(&self) -> bool {
    self.dialog.is_some()
        || self.restore_dialog.is_some()
        || self.trash_restore_dialog.is_some()
}

fn open_storage_search(&mut self) {
    let state = self.scan.as_ref().map(ScanHandle::state);
    if !can_open_storage_search(state, self.confirmation_dialog_open()) {
        return;
    }
    self.search_dialog = Some(SearchDialog::new(self.recs_revision));
}
```

In `update`, detect `input.modifiers.command && input.key_pressed(egui::Key::F)` and call `open_storage_search` only after confirmation-dialog state is known. In the Escape chain, close search first; only then close restore dialogs, navigate rails, or navigate the map. Do not add search to the 40 ms repaint loop.

- [ ] **Step 4: Run focused and full application tests**

Run: `cargo test app::tests::storage_search --locked && cargo test app::tests --locked`

Expected: all application tests PASS without changing existing mutation gates.

- [ ] **Step 5: Commit state boundaries**

```bash
git add src/app.rs
git commit -m "Gate Storage Search to completed maps"
```

---

### Task 3: Adaptive-native search dialog and read-only result actions

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 2's `SearchDialog`, completed scan root, search revision, existing `reveal_in_finder`, and `/usr/bin/qlmanage -p` Quick Look pattern.
- Produces: `draw_storage_search(&mut self, ctx: &Context)`, `open_search_folder(&mut self, node: Arc<Node>)`, `launch_quick_look(&mut self, path: &Path)`, fixed `SearchRowLayout`, and the visible Find/Command-F entry points.

- [ ] **Step 1: Add failing behavior and geometry tests**

Add a pure `SearchRowLayout::from_rect(rect)` with `content` and `actions` rectangles. Assert at widths 620 and 760 that `content.max.x <= actions.min.x`, both stay inside the row, and the action width is at least 168 points.

Add tests that `primary_search_action` returns `OpenMap` for readable folders, `QuickLook` for files, and no primary action for denied folders. Add a breadcrumb-opening test asserting the app's `crumbs` ends at the selected folder and `view` points to the same `Arc<Node>`.

- [ ] **Step 2: Run focused tests and verify red**

Run: `cargo test app::tests::storage_search --locked`

Expected: FAIL because layout and UI action helpers are absent.

- [ ] **Step 3: Add the Find affordance to the map breadcrumb row**

Reserve room beside Back for a compact semantic button labelled `Find  ⌘F`. Enable it only for `ScanState::Done` and when no confirmation dialog is active. Disabled hover copy must be `Available when this map completes` during Running/Aborted and `Run a scan to search mapped storage` with no scan. Clicking calls `open_storage_search()`; it never starts a scan.

- [ ] **Step 4: Render the focused dialog**

Call `self.draw_storage_search(ctx)` after `central(ctx)` and before destructive confirmation windows. Use a centered, non-collapsible, non-resizable egui window titled `Storage Search`, 720 points wide and no more than 520 points tall. Render semantic widgets for:

```text
Searches folders and large files retained in this completed map. Small items remain grouped.
```

Request focus on the `TextEdit::singleline` exactly once. Recompute with `search_tree(root, &query, DEFAULT_RESULT_LIMIT)` only when `TextEdit::changed()` and the dialog revision equals `recs_revision`; otherwise close stale state. Empty/one-character guidance, no-match copy, `N matches`, and `Showing 80 of N` must be separate states.

Rows use `SearchRowLayout`: elided name/path and size/kind on the left; fixed actions on the right. Denied rows say `No access`. Up/Down clamp and update `selected`; Enter dispatches only the selected row's primary action. The selected row has a semantic selected label and non-color indicator.

- [ ] **Step 5: Wire only the three allowed actions**

For readable folders, `Open map` calls `crumbs_for(root, node)`, then sets `crumbs`, `view`, clears zoom, and closes search. For files, Quick Look spawns `/usr/bin/qlmanage -p <PathBuf>` with null stdio. Reveal calls the existing `reveal_in_finder` with the original `PathBuf`. A missing breadcrumb chain disables Open map and leaves Reveal available. No `Rec`, `Action`, `OffloadJob`, `RestoreJob`, or cleanup function may appear in search dispatch.

- [ ] **Step 6: Format and run application plus full Rust tests**

Run: `cargo fmt && cargo test app::tests::storage_search --locked && cargo test --locked`

Expected: all tests PASS; the only ignored test remains the explicit reclaim-history visual fixture seeder.

- [ ] **Step 7: Commit the visible feature**

```bash
git add src/app.rs
git commit -m "Add the Storage Search workspace"
```

---

### Task 4: Accessibility smoke, contributor contract, and product documentation

**Files:**
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-signed-ui.sh`
- Modify: `scripts/test-ui-smoke.sh`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Test: `scripts/test-ui-smoke.sh`

**Interfaces:**
- Consumes: semantic `Find  ⌘F`, `Storage Search`, search text field, scope copy, and Escape behavior from Task 3.
- Produces: a static and signed non-mutating `storage-search-visible` smoke command plus accurate public/maintainer contracts.

- [ ] **Step 1: Make the static smoke contract fail first**

Extend `scripts/test-ui-smoke.sh` to require `storage-search-visible`, the heading `Storage Search`, the exact scope copy, and a signed runner call. Add forbidden patterns for typing into the field, Enter/Return, `Open map`, `Quick Look`, and `Reveal in Finder` in the signed path.

Run: `scripts/test-ui-smoke.sh`

Expected: FAIL because the AppleScript command and runner call are absent.

- [ ] **Step 2: Add read-only signed navigation**

In `ui-smoke.applescript`, add `storage-search-visible`: locate and click the semantic `Find  ⌘F` button, verify the `Storage Search` heading, text field, and exact scope copy, then return success without typing or activating a result. Add the command to usage copy. In `test-signed-ui.sh`, call it after the basic control check and immediately send Escape.

Run: `scripts/test-ui-smoke.sh`

Expected: `UI smoke tooling checks passed`.

- [ ] **Step 3: Document exactly what shipped**

README feature list and Controls must explain Command-F, completed-map-only scope, folders ≥10 MB, files ≥100 MB, ranking, Open map, Quick Look, Reveal, no hidden traversal, and no destructive result action. Add `search.rs` to the test table.

AGENTS must add `search` to the flat-module list and record: completed compact tree only, no second traversal/Spotlight/network/persistence, small items remain grouped, raw `PathBuf` actions, new-scan invalidation, and the signed-smoke prohibition on query typing and result activation.

- [ ] **Step 4: Run documentation and shell contracts**

Run:

```bash
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
scripts/test-community-files.sh
git diff --check
```

Expected: every command exits 0.

- [ ] **Step 5: Commit smoke and documentation**

```bash
git add scripts/ui-smoke.applescript scripts/test-signed-ui.sh scripts/test-ui-smoke.sh README.md AGENTS.md
git commit -m "Document the Storage Search boundary"
```

---

### Task 5: Signed-app visual proof, release integration, and exact CI

**Files:**
- Modify: `docs/superpowers/specs/2026-07-12-storage-search-design.md` (delivered proof/status only)
- Generated/ignored: `dist/DiskDeck.zip`, `/Applications/DiskDeck.app`

**Interfaces:**
- Consumes: all preceding tasks and the repository's stable signing/package path.
- Produces: verified signed light/dark UX, a clean main branch, current distribution ZIP, and green exact GitHub CI.

- [ ] **Step 1: Run the complete local gate on the feature branch**

Run:

```bash
cargo fmt -- --check
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
scripts/test-community-files.sh
cargo test --locked
git diff --check main...HEAD
```

Expected: 0 failures; one intentionally ignored reclaim-history visual fixture test.

- [ ] **Step 2: Build and inspect the signed app without mutation**

Run `./make-app.sh`, validate with `codesign --verify --deep --strict --verbose=2 /Applications/DiskDeck.app` and `scripts/check-dist.sh dist/DiskDeck.zip`, then run `scripts/test-signed-ui.sh`.

In the signed app inspect empty, one-character, no-match, populated, capped, and long-path results at 1180 × 740 and typical size in light and dark appearance. Verify Command-F focus, Up/Down selection, Enter folder opening, Escape priority, Quick Look, and Finder reveal using existing read-only paths only. Restore the owner's appearance and window state afterward.

- [ ] **Step 3: Record exact delivered proof**

Update the design status to delivered and append the test count, signed smoke result, inspected states/sizes/appearances, read-only action proof, and explicit statement that no filesystem mutation or QA file creation occurred.

Commit:

```bash
git add -f docs/superpowers/specs/2026-07-12-storage-search-design.md
git commit -m "Record Storage Search verification"
```

- [ ] **Step 4: Fast-forward main and rebuild from the authoritative checkout**

Fetch `origin/main`, confirm no remote divergence, fast-forward merge the feature branch, rerun `cargo test --locked`, `./make-app.sh`, signature validation, ZIP validation, and signed smoke from the main checkout.

- [ ] **Step 5: Push and watch the exact workflow**

Push `main` without bypassing hooks. Locate the workflow whose `headSha` equals the pushed `HEAD`, run `gh run watch <run-id> --repo raghavaadi/DiskDeck --exit-status`, and require every macOS check to pass.

- [ ] **Step 6: Clean only the owned worktree and audit final state**

Remove the `.worktrees/storage-search` worktree from the main checkout, prune, delete the merged feature branch, and verify: clean `git status`, `HEAD == origin/main`, no open worktree registration, no generated fixture under user data, valid `/Applications/DiskDeck.app`, and current `dist/DiskDeck.zip`.
