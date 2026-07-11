# Treemap Context Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace modifier-click treemap actions with a discoverable secondary-click context menu and a visible Back control.

**Architecture:** Keep treemap layout and storage operations unchanged. Add pure helpers in `app.rs` for menu availability, tooltip copy, and Back depth; bind one stable egui `Response` to each laid-out map item; translate menu choices into owned requests that call the existing open, Finder reveal, and SSD offload paths only after menu rendering completes.

**Tech Stack:** Rust 2021, egui 0.29 `Response::context_menu`, eframe 0.29, existing in-module unit tests, signed macOS bundle via `make-app.sh`.

## Global Constraints

- `CFBundleIdentifier` remains exactly `com.buddyhq.headroom-rs`.
- The default signing identity remains exactly `Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)`.
- The scan root remains exactly `/System/Volumes/Data`.
- No filesystem action is available for synthetic aggregate blocks.
- **Move to SSD** must call the existing `open_offload_dialog`; its acknowledgement, capacity, verified-copy, and hold requirements remain unchanged.
- No new crate dependencies.
- Primary click opens only accessible real directories.
- Secondary click never navigates back or performs a primary-click action.
- Escape may navigate back but is not advertised.
- Visual QA must use the signed app in both macOS appearances.

---

## File structure

- `src/app.rs`: adds pure interaction-policy helpers, stable per-item responses, context-menu rendering, owned request dispatch, Back navigation, and simplified tooltip copy.
- `docs/superpowers/specs/2026-07-11-treemap-context-navigation-design.md`: approved interaction contract; no further changes expected.
- `docs/superpowers/plans/2026-07-11-treemap-context-navigation.md`: this execution checklist.

### Task 1: Lock interaction policy with unit tests

**Files:**
- Modify: `src/app.rs` near `WorkspaceLayout` and its in-module tests
- Test: `src/app.rs` in-module tests

**Interfaces:**
- Produces: `MapItemActions { open: bool, reveal: bool, move_to_ssd: bool }`
- Produces: `map_item_actions(is_dir: bool, synthetic: bool, denied: bool, has_node: bool) -> MapItemActions`
- Produces: `map_item_hint(is_dir: bool, synthetic: bool, denied: bool) -> &'static str`
- Produces: `back_target(depth: usize) -> Option<usize>`

- [ ] **Step 1: Write failing policy tests**

```rust
#[test]
fn map_actions_match_real_synthetic_and_denied_items() {
    assert_eq!(
        map_item_actions(true, false, false, true),
        MapItemActions { open: true, reveal: true, move_to_ssd: true }
    );
    assert_eq!(
        map_item_actions(false, false, false, true),
        MapItemActions { open: false, reveal: true, move_to_ssd: true }
    );
    assert_eq!(
        map_item_actions(false, true, false, false),
        MapItemActions { open: false, reveal: false, move_to_ssd: false }
    );
    assert_eq!(
        map_item_actions(true, false, true, true),
        MapItemActions { open: false, reveal: true, move_to_ssd: true }
    );
}

#[test]
fn map_hints_explain_discoverable_actions_without_modifiers() {
    assert_eq!(map_item_hint(true, false, false), "Click to open · Right-click for actions");
    assert_eq!(map_item_hint(false, false, false), "Right-click for actions");
    assert_eq!(map_item_hint(false, true, false), "Combined smaller items");
    assert_eq!(
        map_item_hint(true, false, true),
        "Access unavailable · Grant Full Disk Access to inspect"
    );
}

#[test]
fn back_target_is_one_level_and_inert_at_root() {
    assert_eq!(back_target(0), None);
    assert_eq!(back_target(1), Some(0));
    assert_eq!(back_target(3), Some(2));
}
```

- [ ] **Step 2: Run focused tests and verify RED**

Run: `~/.cargo/bin/cargo test app::tests::map_actions app::tests::map_hints app::tests::back_target -- --nocapture`

Because Cargo accepts one filter at a time, run the three exact filters separately. Expected: compilation fails because the helper types and functions do not exist.

- [ ] **Step 3: Implement minimal pure helpers**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MapItemActions {
    open: bool,
    reveal: bool,
    move_to_ssd: bool,
}

fn map_item_actions(
    is_dir: bool,
    synthetic: bool,
    denied: bool,
    has_node: bool,
) -> MapItemActions {
    let real = has_node && !synthetic;
    MapItemActions {
        open: real && is_dir && !denied,
        reveal: real,
        move_to_ssd: real,
    }
}

fn map_item_hint(is_dir: bool, synthetic: bool, denied: bool) -> &'static str {
    if synthetic {
        "Combined smaller items"
    } else if denied {
        "Access unavailable · Grant Full Disk Access to inspect"
    } else if is_dir {
        "Click to open · Right-click for actions"
    } else {
        "Right-click for actions"
    }
}

fn back_target(depth: usize) -> Option<usize> {
    depth.checked_sub(1)
}
```

- [ ] **Step 4: Run focused and full tests for GREEN**

Run each focused filter, then `~/.cargo/bin/cargo test`.

Expected: all policy tests and all existing 35 tests pass.

- [ ] **Step 5: Commit the interaction policy**

```bash
git add src/app.rs
git commit -m "Define treemap interaction policy"
```

### Task 2: Add context menu and visible Back navigation

**Files:**
- Modify: `src/app.rs:1005-1222`
- Test: `src/app.rs` in-module tests from Task 1

**Interfaces:**
- Consumes: `MapItemActions`, `map_item_actions`, `map_item_hint`, and `back_target` from Task 1.
- Consumes: existing `strip_data_root`, `reveal_in_finder`, `open_offload_dialog`, `crumbs`, `view`, and `zoom` behavior.
- Produces: stable context menus for every painted treemap item and a visible Back button at every depth.

- [ ] **Step 1: Add an owned request type beside the policy helpers**

```rust
enum MapActionRequest {
    Open { node: Arc<Node>, source: Rect },
    Reveal(std::path::PathBuf),
    MoveToSsd { path: std::path::PathBuf, bytes: i64 },
}
```

This request boundary prevents menu closures from mutating `App` while treemap items are borrowed.

- [ ] **Step 2: Replace Up with a persistent Back control**

Always render a trailing button using `ui.add_enabled(depth > 0, ...)` with visible copy `← Back`. On click, set `go_to = back_target(depth)`. Keep ancestor breadcrumb segments clickable. Change the hover copy to `Return to the previous folder`. Remove every visible instruction that describes right-click as Back.

- [ ] **Step 3: Replace the map-wide click response with stable item responses**

Use the current pointer only for painting:

```rust
let hover = ui
    .input(|input| input.pointer.hover_pos())
    .filter(|position| map_rect.contains(*position));
let hovered = treemap::paint(ui, map_rect, &items, &laid, hover, zoom);
let mut requested_action: Option<MapActionRequest> = None;
let mut menu_open = false;
```

For every `(idx, item_rect)` in `laid`, allocate `ui.interact(*item_rect, ui.id().with(("treemap-item", idx)), Sense::click())`. Skip primary-click dispatch while `zoom.is_some()`. An enabled primary click creates `MapActionRequest::Open`.

- [ ] **Step 4: Render the secondary-click menu and collect requests**

For every item response, call `response.context_menu` unconditionally so an open menu remains stable while the pointer moves into it. Set a minimum width of 180 px and render:

```rust
if menu_ui.add_enabled(actions.open, egui::Button::new("Open")).clicked() {
    requested_action = node.clone().map(|node| MapActionRequest::Open {
        node,
        source: *item_rect,
    });
    menu_ui.close_menu();
}
if menu_ui
    .add_enabled(actions.reveal, egui::Button::new("Reveal in Finder"))
    .clicked()
{
    requested_action = node.as_ref().map(|node| MapActionRequest::Reveal(node.path.clone()));
    menu_ui.close_menu();
}
menu_ui.separator();
if menu_ui
    .add_enabled(actions.move_to_ssd, egui::Button::new("Move to SSD…"))
    .clicked()
{
    requested_action = node.as_ref().map(|node| MapActionRequest::MoveToSsd {
        path: strip_data_root(&node.path),
        bytes: node.bytes(),
    });
    menu_ui.close_menu();
}
```

After each menu call, OR `response.context_menu_opened()` into `menu_open`. Draw the existing tooltip only when `!menu_open`, and obtain its final line from `map_item_hint`.

- [ ] **Step 5: Dispatch exactly one request after the interaction loop**

```rust
match requested_action {
    Some(MapActionRequest::Open { node, source }) => {
        self.crumbs.push(node.clone());
        self.view = Some(node);
        self.zoom = Some((source, Instant::now()));
    }
    Some(MapActionRequest::Reveal(path)) => reveal_in_finder(&path),
    Some(MapActionRequest::MoveToSsd { path, bytes }) => {
        self.open_offload_dialog(path, bytes);
    }
    None => {}
}
```

Handle Escape after dispatch only when no menu is open: call `back_target(self.crumbs.len())`, truncate crumbs, refresh `view`, and clear `zoom`. Delete Command-click, Option-click, and secondary-click Back branches.

- [ ] **Step 6: Run formatting and regression tests**

Run: `~/.cargo/bin/cargo fmt -- --check && ~/.cargo/bin/cargo test`

Expected: formatting is clean; all tests pass; no cleanup/offload tests change.

- [ ] **Step 7: Commit the discoverable navigation UI**

```bash
git add src/app.rs
git commit -m "Add treemap context navigation"
```

### Task 3: Signed interaction and appearance proof

**Files:**
- Modify only if QA exposes a defect: `src/app.rs`
- Verify: `make-app.sh`, `/Applications/DiskDeck.app`, `dist/DiskDeck.zip`

**Interfaces:**
- Consumes: completed Task 2 UI.
- Produces: a signed installed app with verified context menu, Back navigation, light/dark appearance, and unchanged identity.

- [ ] **Step 1: Run repository guards and full tests**

Run: `~/.cargo/bin/cargo fmt -- --check`, `~/.cargo/bin/cargo test`, `scripts/test-pre-commit.sh`, and `scripts/test-pre-push.sh`.

Expected: 0 test failures and both hook suites pass.

- [ ] **Step 2: Build and install through the signed ship path**

Run: `./make-app.sh`.

Expected: release build succeeds, the app is installed, and the distribution zip is created.

- [ ] **Step 3: Verify identity**

Run:

```bash
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/DiskDeck.app/Contents/Info.plist
codesign --verify --strict --verbose=2 /Applications/DiskDeck.app
```

Expected: `com.buddyhq.headroom-rs` and a valid designated requirement.

- [ ] **Step 4: Verify interaction without destructive execution**

In the signed app, verify primary-click opens a directory; Back returns one level and disables at Data; breadcrumb jumps to an ancestor; secondary-click opens the menu at the pointer; Open works for a directory; Reveal in Finder works on the smallest harmless real item; Move to SSD opens the existing dialog but is cancelled before confirmation; synthetic items show disabled actions; Escape returns one level only when the menu is closed.

- [ ] **Step 5: Verify dark and light appearance**

Capture the signed window in each system appearance. Confirm the context menu uses readable Inter text, the Back button is visible without crowding breadcrumbs, modifier-click copy is absent, and no tofu, clipping, white backgrounds, or map repaint gaps appear. Restore the user's original system appearance.

- [ ] **Step 6: Run final verification after any correction**

Run: `~/.cargo/bin/cargo fmt -- --check && ~/.cargo/bin/cargo test && git diff --check`.

Expected: formatting and all tests pass with no whitespace errors.

- [ ] **Step 7: Commit corrections, push main, and verify the remote SHA**

```bash
git add src/app.rs
git commit -m "Polish treemap navigation" # only when QA required a correction
git push origin main
git ls-remote origin refs/heads/main
```

Expected: the remote SHA equals local `HEAD`, and personal GitHub identity checks pass.
