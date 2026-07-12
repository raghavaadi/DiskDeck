# Safety & Quick Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a permanent, non-blocking in-app guide that explains DiskDeck's real workflow and routes everyday Mac users and developers to the right existing tools.

**Architecture:** Extend the existing `RailView` router with one read-only Guide destination and keep all guide presentation in `app.rs`. Pure action-state and layout helpers make readiness and minimum-size behavior testable; the guide itself only changes `rail_view` and never starts work or mutates storage.

**Tech Stack:** Rust 1.80+, egui 0.29 / eframe, AccessKit through egui, AppleScript signed-app smoke tests, POSIX shell verification.

## Global Constraints

- Preserve `scan::DATA_ROOT` as `/System/Volumes/Data` and do not alter scan, cleanup, move, or restore authority.
- Nothing is deleted without a checked item and the existing 900 ms hold.
- Add no crate dependency, settings file, telemetry, migration, network request, background task, or first-run modal.
- Keep the toolbar unchanged and preserve the supported 1180 × 740 minimum layout.
- The guide is read-only; opening it may only change `rail_view`.
- Follow Adaptive Native palette semantics and verify both light and dark appearances.

---

## File map

- `src/app.rs` — Guide route, pure readiness/layout helpers, entry points, and rendered rail.
- `scripts/ui-smoke.applescript` — non-destructive AccessKit navigation proof.
- `scripts/test-ui-smoke.sh` — static safety contract for the new smoke command.
- `scripts/test-signed-ui.sh` — runs the Guide proof against the signed app.
- `README.md` — user-facing feature and control documentation.
- `AGENTS.md` — maintainer architecture and guide-accuracy rule.

---

### Task 1: Add the read-only Guide route and readiness model

**Files:**
- Modify: `src/app.rs:178-208`
- Modify: `src/app.rs:2277-2290`
- Modify: `src/app.rs:2630-2970`
- Modify: `src/app.rs:4380-4430`

**Interfaces:**
- Consumes: existing `RailView`, `rail_back_target`, `panel_chrome`, `recs_built`, `scanning()`.
- Produces: `RailView::Guide`, `GuidePrimaryAction { enabled: bool, label: &'static str }`, and `guide_primary_action(bool, bool) -> GuidePrimaryAction`.

- [ ] **Step 1: Write failing routing and readiness tests**

Add these assertions to the existing `app::tests` module:

```rust
#[test]
fn guide_returns_to_insights_without_mutation_routing() {
    assert_eq!(rail_back_target(RailView::Guide), Some(RailView::Insights));
}

#[test]
fn guide_primary_action_is_honest_about_readiness() {
    assert_eq!(
        guide_primary_action(false, true),
        GuidePrimaryAction {
            enabled: false,
            label: "Scanning for safe targets…",
        }
    );
    assert_eq!(
        guide_primary_action(false, false),
        GuidePrimaryAction {
            enabled: false,
            label: "No safe targets yet",
        }
    );
    assert_eq!(
        guide_primary_action(true, false),
        GuidePrimaryAction {
            enabled: true,
            label: "Free up space",
        }
    );
}
```

- [ ] **Step 2: Run the focused tests and verify failure**

Run:

```sh
~/.cargo/bin/cargo test app::tests::guide_ --locked
```

Expected: compilation fails because `RailView::Guide`, `GuidePrimaryAction`, and `guide_primary_action` do not exist.

- [ ] **Step 3: Implement the route and pure action state**

Add `Guide` immediately after `Insights` in `RailView`. Route it back to
Insights:

```rust
RailView::Guide
| RailView::Moved
| RailView::Growth
| RailView::Developer
| RailView::Apfs
| RailView::Leftovers
| RailView::Monitor
| RailView::FileReview
| RailView::ReclaimHistory
| RailView::External => Some(RailView::Insights),
```

Add the pure model beside the other rail helpers:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GuidePrimaryAction {
    enabled: bool,
    label: &'static str,
}

fn guide_primary_action(recs_ready: bool, scanning: bool) -> GuidePrimaryAction {
    if scanning {
        GuidePrimaryAction {
            enabled: false,
            label: "Scanning for safe targets…",
        }
    } else if !recs_ready {
        GuidePrimaryAction {
            enabled: false,
            label: "No safe targets yet",
        }
    } else {
        GuidePrimaryAction {
            enabled: true,
            label: "Free up space",
        }
    }
}
```

Make the rail dispatcher exhaustive with a minimal read-only surface:

```rust
RailView::Guide => {
    self.draw_safety_guide(ui, rect);
    return;
}
```

Add the temporary complete renderer that Task 2 will expand:

```rust
fn draw_safety_guide(&mut self, ui: &mut egui::Ui, rect: Rect) {
    let palette = theme::palette(ui.ctx());
    let content = panel_chrome(
        ui,
        rect,
        "Safety & Quick Start",
        Some(("read-only guide".into(), palette.faint)),
    );
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
        if ui.button("← Insights").clicked() {
            self.rail_view = RailView::Insights;
        }
    });
}
```

- [ ] **Step 4: Run the focused and full test suites**

Run:

```sh
~/.cargo/bin/cargo test app::tests::guide_ --locked
~/.cargo/bin/cargo test --locked
```

Expected: focused tests pass and the full current suite passes.

- [ ] **Step 5: Commit the route**

```sh
git add src/app.rs
git commit -m "Add the read-only Quick Start route" -m "Give the guide an isolated rail destination and test its navigation and readiness copy before rendering the full experience."
```

---

### Task 2: Render the adaptive guide and add both entry points

**Files:**
- Modify: `src/app.rs:2100-2390`
- Modify: `src/app.rs:2880-2980`
- Modify: `src/app.rs:4540-4755`
- Modify: `src/app.rs:5136-5265`

**Interfaces:**
- Consumes: Task 1 `RailView::Guide` and `guide_primary_action`; existing `guided_layout`, palette, typography, button, and rail patterns.
- Produces: `GuideLayout { nav: Rect, body: Rect, action: Rect }`, `guide_layout(Rect) -> GuideLayout`, complete `draw_safety_guide`, Summary `Guide` entry, and first Insights `Safety & Quick Start` entry.

- [ ] **Step 1: Write the failing minimum-size layout test**

```rust
#[test]
fn guide_layout_keeps_navigation_body_and_actions_separate() {
    let content = Rect::from_min_size(Pos2::ZERO, vec2(320.0, 600.0));
    let layout = guide_layout(content);
    assert!(layout.nav.max.y < layout.body.min.y);
    assert!(layout.body.max.y < layout.action.min.y);
    assert_eq!(layout.action.max, content.shrink2(vec2(12.0, 8.0)).max);
    assert!(layout.body.height() > 300.0);
}
```

- [ ] **Step 2: Run the focused layout test and verify failure**

Run:

```sh
~/.cargo/bin/cargo test app::tests::guide_layout_ --locked
```

Expected: compilation fails because `guide_layout` does not exist.

- [ ] **Step 3: Implement the fixed action layout**

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
struct GuideLayout {
    nav: Rect,
    body: Rect,
    action: Rect,
}

fn guide_layout(content: Rect) -> GuideLayout {
    let inset = content.shrink2(vec2(12.0, 8.0));
    let nav = Rect::from_min_size(inset.min, vec2(inset.width(), 30.0));
    let action = Rect::from_min_max(pos2(inset.min.x, inset.max.y - 76.0), inset.max);
    let body = Rect::from_min_max(
        pos2(inset.min.x, nav.max.y + 6.0),
        pos2(inset.max.x, action.min.y - 8.0),
    );
    GuideLayout { nav, body, action }
}
```

- [ ] **Step 4: Replace the temporary renderer with the complete guide**

Use `panel_chrome` with title `Safety & Quick Start` and metadata `read-only guide`.
Render a frameless `← Insights` control in `layout.nav`. In a vertical
`ScrollArea` inside `layout.body`, render these exact accessible labels and
plain-language descriptions using existing surface cards:

```rust
let sections = [
    (
        "1",
        "SCAN · EXPLORE · REVIEW · HOLD",
        "Scanning and map navigation are read-only. Nothing changes until you check a target and hold the final control for 0.9 seconds.",
        palette.accent,
    ),
    (
        "✓",
        "SAFE MEANS IT REBUILDS",
        "Safe targets regenerate automatically. Caution targets can cost a download or reinstall and are never pre-selected.",
        palette.safe,
    ),
    (
        "↗",
        "EVERYDAY MAC PATH",
        "Use Find or right-click the map to inspect storage. Guided Reclaim builds a Safe-only plan; Reclaim History explains what happened and what can be restored.",
        palette.accent,
    ),
    (
        "{}",
        "DEVELOPER PATH",
        "Developer Lens explains Docker, Xcode, projects, and package stores. Growth Watch and APFS accounting show evidence without turning it into a cleanup claim.",
        palette.caution,
    ),
];
```

Use a two-column action row in `layout.action`: **Explore the map** sets
`RailView::Summary`; the second button uses `guide_primary_action` and sets
`guide_revision`, clears acknowledgement, and opens `RailView::GuidedReclaim`
only when enabled. Do not call any `begin_*` method.

- [ ] **Step 5: Add Summary and Insights entry points**

Replace the single full-width Summary Insights button with a horizontal pair
of equal-width buttons named **Guide** and **Insights** in the existing
34-point row. `Guide` opens `RailView::Guide`; `Insights` preserves the current
route and hover explanation.

Add this as the first `draw_insights` entry:

```rust
(
    "Safety & Quick Start",
    "How scanning, review, reclaim, and recovery fit together".into(),
    RailView::Guide,
),
```

- [ ] **Step 6: Run focused and full tests**

Run:

```sh
~/.cargo/bin/cargo test app::tests::guide_ --locked
~/.cargo/bin/cargo test --locked
```

Expected: all Guide tests and the complete suite pass.

- [ ] **Step 7: Commit the rendered feature**

```sh
git add src/app.rs
git commit -m "Teach DiskDeck's safe workflow in app" -m "Add a persistent adaptive guide with everyday and developer paths while keeping every action behind the existing safety boundaries."
```

---

### Task 3: Document and smoke-test the guide

**Files:**
- Modify: `README.md:14-130`
- Modify: `README.md:210-245`
- Modify: `AGENTS.md:100-125`
- Modify: `AGENTS.md:220-240`
- Modify: `scripts/ui-smoke.applescript:120-185`
- Modify: `scripts/test-ui-smoke.sh:15-105`
- Modify: `scripts/test-signed-ui.sh:20-80`

**Interfaces:**
- Consumes: accessible names `Guide`, `Safety & Quick Start`, `← Insights`, and `SCAN · EXPLORE · REVIEW · HOLD` from Task 2.
- Produces: `safety-guide-visible` AppleScript command and a signed-app navigation proof that never activates a storage action.

- [ ] **Step 1: Add the failing smoke contract**

In `scripts/test-ui-smoke.sh`, require the new command and its safety copy:

```sh
grep -q 'commandName is "safety-guide-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose safety-guide-visible"

grep -q 'static text "Safety & Quick Start"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Safety & Quick Start heading"

grep -Fq 'static text "SCAN · EXPLORE · REVIEW · HOLD"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the core safe workflow"

grep -q '^ui safety-guide-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open the Safety & Quick Start guide"
```

- [ ] **Step 2: Run the smoke contract and verify failure**

Run:

```sh
scripts/test-ui-smoke.sh
```

Expected: FAIL because the AppleScript command is absent.

- [ ] **Step 3: Implement non-destructive signed UI navigation**

Add this branch before `guided-reclaim-visible`:

```applescript
else if commandName is "safety-guide-visible" then
    my openSummary(appGroup)
    if not (exists button "Guide" of appGroup) then error "Guide entry point is unavailable." number 1
    click button "Guide" of appGroup
    delay 0.5
    if not (exists button "← Insights" of appGroup) then error "Safety guide rail did not open." number 1
    if not (exists static text "Safety & Quick Start" of appGroup) then error "Safety guide heading is unavailable." number 1
    if not (exists static text "SCAN · EXPLORE · REVIEW · HOLD" of appGroup) then error "Safe workflow explanation is unavailable." number 1
    return "PASS: Safety & Quick Start available without starting a storage action"
```

Add `safety-guide-visible` to the usage string. In `test-signed-ui.sh`, run:

```sh
ui safety-guide-visible
ui escape
```

immediately after `ui check`.

- [ ] **Step 4: Update public and maintainer documentation**

Add a README feature bullet describing the permanent guide, both paths, and
the fact that it never runs work. Add Controls rows for `Guide` and
`Safety & Quick Start`. Add `Guide rail` to the `app.rs` architecture row and
state in AGENTS conventions that user-visible workflow changes must keep the
guide copy and signed smoke proof current.

- [ ] **Step 5: Run documentation and smoke guards**

Run:

```sh
scripts/test-ui-smoke.sh
scripts/test-pre-commit.sh
git diff --check
```

Expected: all commands pass.

- [ ] **Step 6: Commit documentation and smoke coverage**

```sh
git add README.md AGENTS.md scripts/ui-smoke.applescript scripts/test-ui-smoke.sh scripts/test-signed-ui.sh
git commit -m "Document and smoke-test the Quick Start guide" -m "Keep the permanent safety explanation discoverable and prove the signed app can open it without invoking storage work."
```

---

### Task 4: Build, inspect, and publish the signed feature

**Files:**
- Verify: `src/app.rs`
- Verify: `/Applications/DiskDeck.app`
- Verify: `dist/DiskDeck.zip`

**Interfaces:**
- Consumes: complete feature, tracked verification scripts, and `make-app.sh` ship path.
- Produces: signed installed app, current distributable zip, visual evidence, pushed commits, and green GitHub CI.

- [ ] **Step 1: Run the full logic and repository guards**

```sh
~/.cargo/bin/cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-package-artifact.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
```

Expected: every command passes.

- [ ] **Step 2: Build, sign, install, and package**

```sh
./make-app.sh
codesign --verify --deep --strict --verbose=2 /Applications/DiskDeck.app
unzip -t dist/DiskDeck.zip
```

Expected: the app signature is valid and the zip reports no errors.

- [ ] **Step 3: Run the signed UI smoke suite**

```sh
scripts/test-signed-ui.sh
```

Expected: the Guide check and all existing non-destructive UI checks pass.

- [ ] **Step 4: Inspect all supported visual states**

Launch the signed app and capture the Guide at 1480 × 920 and 1180 × 740 in
both macOS light and dark appearances. Confirm the heading, four cards, Back,
Explore the map, and primary action remain visible or scroll correctly without
overlap, clipping, tofu glyphs, or an opaque icon background. Restore the
owner's original appearance and window size after inspection.

- [ ] **Step 5: Publish and verify exact CI**

```sh
git status --short
git push origin main
run_id=$(gh run list --repo raghavaadi/DiskDeck --branch main --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch --repo raghavaadi/DiskDeck "$run_id" --exit-status
```

Expected: the worktree is clean, `origin/main` contains every feature commit,
and the exact pushed run completes successfully.
