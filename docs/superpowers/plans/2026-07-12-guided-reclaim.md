# Guided Reclaim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an everyday Mac user choose a reclaim target, receive a deterministic Safe-only plan, review the evidence, and compare planned bytes with the actual cleanup outcome.

**Architecture:** Add a pure `reclaim_plan` domain module that selects immutable `Rec` identifiers without creating paths, commands, or filesystem work. `App` owns the ephemeral goal, recommendation revision, acknowledgement, and cleanup outcome; applying a valid plan changes only existing `RecRow.checked` values and then delegates execution to the unchanged `clean::run_clean` boundary.

**Tech Stack:** Rust, egui 0.29/eframe, existing `rules::Rec`, existing `clean::CleanEvent`, shell/AppleScript signed-app smoke tooling.

## Global Constraints

- No new crate dependencies.
- Only `Tier::Safe` findings may be selected by the automatic plan.
- `Tier::Caution` findings remain visible, optional, and unchecked.
- User documents, code, media, app leftovers, duplicate files, and large-old files never enter the automatic plan.
- The planner handles identifiers and byte counts only; it never creates a path, action, or command.
- Cleanup continues exclusively through `clean::run_clean`; command recommendations execute only the vetted command stored on the original `Rec`.
- Nothing is removed without an explicitly checked item and the existing 900 ms hold.
- Full scans remain user initiated and read-only.
- Planned estimates and actual recovered bytes remain visibly distinct.
- UI verification uses the signed `/Applications/DiskDeck.app` and never invokes a cleanup action on owner data.

---

### Task 1: Pure Guided-Reclaim Planner

**Files:**
- Create: `src/reclaim_plan.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `rules::Rec`, `rules::Tier`
- Produces: `GB`, `GoalError`, `PlanItem`, `ReclaimPlan`, `parse_goal_gb(&str, i64)`, and `build_plan(&[Rec], i64)`

- [ ] **Step 1: Register the module and write failing planner tests**

Add this declaration to `src/main.rs` beside the other flat modules:

```rust
mod reclaim_plan;
```

Create `src/reclaim_plan.rs` with the public type shapes and tests first:

```rust
use crate::rules::{Rec, Tier};

pub const GB: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalError {
    Empty,
    NotWholeGigabytes,
    Zero,
    ExceedsUsedSpace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub id: String,
    pub bytes: i64,
    pub estimate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimPlan {
    pub goal_bytes: i64,
    pub items: Vec<PlanItem>,
    pub selected_bytes: i64,
    pub measured_bytes: i64,
    pub estimated_bytes: i64,
    pub shortfall_bytes: i64,
    pub caution_bytes: i64,
}

pub fn parse_goal_gb(_input: &str, _used_bytes: i64) -> Result<i64, GoalError> {
    unimplemented!()
}

pub fn build_plan(_recs: &[Rec], _goal_bytes: i64) -> ReclaimPlan {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, Rec};
    use std::path::PathBuf;

    fn rec(id: &str, bytes: i64, tier: Tier, estimate: bool) -> Rec {
        Rec {
            id: id.into(),
            title: id.into(),
            path: PathBuf::from(format!("/fixture/{id}")),
            display: format!("/fixture/{id}"),
            bytes,
            tier,
            desc: "fixture",
            restore: "fixture",
            action: Action::Trash,
            command: None,
            allow_trash: true,
            allow_delete: true,
            note: String::new(),
            estimate,
        }
    }

    #[test]
    fn goal_parser_requires_bounded_whole_gigabytes() {
        assert_eq!(parse_goal_gb("", 100 * GB), Err(GoalError::Empty));
        assert_eq!(parse_goal_gb("2.5", 100 * GB), Err(GoalError::NotWholeGigabytes));
        assert_eq!(parse_goal_gb("0", 100 * GB), Err(GoalError::Zero));
        assert_eq!(parse_goal_gb("101", 100 * GB), Err(GoalError::ExceedsUsedSpace));
        assert_eq!(parse_goal_gb(" 20 ", 100 * GB), Ok(20 * GB));
    }

    #[test]
    fn measured_safe_items_win_before_estimates_and_caution() {
        let recs = vec![
            rec("estimated-large", 30 * GB, Tier::Safe, true),
            rec("measured-six", 6 * GB, Tier::Safe, false),
            rec("measured-five", 5 * GB, Tier::Safe, false),
            rec("caution-huge", 100 * GB, Tier::Caution, false),
        ];
        let plan = build_plan(&recs, 10 * GB);
        let ids: Vec<_> = plan.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["measured-six", "measured-five"]);
        assert_eq!(plan.measured_bytes, 11 * GB);
        assert_eq!(plan.estimated_bytes, 0);
        assert_eq!(plan.shortfall_bytes, 0);
        assert_eq!(plan.caution_bytes, 100 * GB);
    }

    #[test]
    fn estimate_is_used_only_when_measured_items_cannot_reach_goal() {
        let recs = vec![
            rec("measured", 9 * GB, Tier::Safe, false),
            rec("estimated", 4 * GB, Tier::Safe, true),
        ];
        let plan = build_plan(&recs, 10 * GB);
        assert_eq!(plan.selected_bytes, 13 * GB);
        assert_eq!(plan.measured_bytes, 9 * GB);
        assert_eq!(plan.estimated_bytes, 4 * GB);
        assert_eq!(plan.shortfall_bytes, 0);
    }

    #[test]
    fn shortfall_and_stable_identifier_tie_break_are_deterministic() {
        let recs = vec![
            rec("zeta", 3 * GB, Tier::Safe, false),
            rec("alpha", 3 * GB, Tier::Safe, false),
        ];
        let plan = build_plan(&recs, 10 * GB);
        let ids: Vec<_> = plan.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zeta"]);
        assert_eq!(plan.shortfall_bytes, 4 * GB);
    }

    #[test]
    fn zero_and_negative_findings_never_enter_the_plan() {
        let recs = vec![
            rec("zero", 0, Tier::Safe, false),
            rec("negative", -1, Tier::Safe, false),
        ];
        let plan = build_plan(&recs, 5 * GB);
        assert!(plan.items.is_empty());
        assert_eq!(plan.shortfall_bytes, 5 * GB);
    }
}
```

- [ ] **Step 2: Run the focused tests and verify the deliberate failure**

Run:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test reclaim_plan::tests
```

Expected: the new tests compile and fail because `parse_goal_gb` and `build_plan` are `unimplemented!()`.

- [ ] **Step 3: Implement parsing and deterministic Safe-only planning**

Replace both temporary failing bodies with:

```rust
pub fn parse_goal_gb(input: &str, used_bytes: i64) -> Result<i64, GoalError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(GoalError::Empty);
    }
    let whole_gb: i64 = input
        .parse()
        .map_err(|_| GoalError::NotWholeGigabytes)?;
    if whole_gb <= 0 {
        return Err(GoalError::Zero);
    }
    let bytes = whole_gb
        .checked_mul(GB)
        .ok_or(GoalError::ExceedsUsedSpace)?;
    if used_bytes <= 0 || bytes > used_bytes {
        return Err(GoalError::ExceedsUsedSpace);
    }
    Ok(bytes)
}

pub fn build_plan(recs: &[Rec], goal_bytes: i64) -> ReclaimPlan {
    let goal_bytes = goal_bytes.max(0);
    let caution_bytes = recs
        .iter()
        .filter(|rec| rec.tier == Tier::Caution)
        .map(|rec| rec.bytes.max(0))
        .sum();
    let mut candidates: Vec<PlanItem> = recs
        .iter()
        .filter(|rec| rec.tier == Tier::Safe && rec.bytes > 0)
        .map(|rec| PlanItem {
            id: rec.id.clone(),
            bytes: rec.bytes,
            estimate: rec.estimate,
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.estimate
            .cmp(&right.estimate)
            .then(right.bytes.cmp(&left.bytes))
            .then(left.id.cmp(&right.id))
    });

    let mut items = Vec::new();
    let mut selected_bytes = 0i64;
    for item in candidates {
        if selected_bytes >= goal_bytes {
            break;
        }
        selected_bytes = selected_bytes.saturating_add(item.bytes);
        items.push(item);
    }
    let measured_bytes = items
        .iter()
        .filter(|item| !item.estimate)
        .map(|item| item.bytes)
        .sum();
    let estimated_bytes = items
        .iter()
        .filter(|item| item.estimate)
        .map(|item| item.bytes)
        .sum();

    ReclaimPlan {
        goal_bytes,
        items,
        selected_bytes,
        measured_bytes,
        estimated_bytes,
        shortfall_bytes: goal_bytes.saturating_sub(selected_bytes),
        caution_bytes,
    }
}
```

- [ ] **Step 4: Run focused and full tests**

Run:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test reclaim_plan::tests
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
```

Expected: five planner tests and the complete existing suite pass.

- [ ] **Step 5: Commit the planner**

```sh
git add src/main.rs src/reclaim_plan.rs
git commit -m "Add deterministic guided reclaim planner" -m "Select only measured or estimated Safe findings in a stable order while exposing honest goal shortfalls and optional Caution capacity."
```

---

### Task 2: Recommendation Revision and Plan Application

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `reclaim_plan::{build_plan, ReclaimPlan, GB}` and the current `Vec<RecRow>`
- Produces: `RailView::GuidedReclaim`, revision-safe guide state, `can_apply_guided_plan`, and `apply_guided_plan`

- [ ] **Step 1: Add failing state-transition tests**

Add these helpers above the `#[cfg(test)]` block in `src/app.rs`:

```rust
fn can_apply_guided_plan(
    acknowledged: bool,
    draft_revision: Option<u64>,
    recs_revision: u64,
    scanning: bool,
    plan: &crate::reclaim_plan::ReclaimPlan,
) -> bool {
    acknowledged
        && draft_revision == Some(recs_revision)
        && !scanning
        && !plan.items.is_empty()
}

fn apply_guided_plan(rows: &mut [RecRow], plan: &crate::reclaim_plan::ReclaimPlan) {
    let ids: std::collections::BTreeSet<&str> =
        plan.items.iter().map(|item| item.id.as_str()).collect();
    for row in rows {
        row.checked = ids.contains(row.rec.id.as_str()) && row.rec.tier == Tier::Safe;
    }
}
```

Initially give both functions `unimplemented!()` bodies, then add tests:

```rust
#[test]
fn guided_plan_requires_acknowledgement_current_revision_and_items() {
    use crate::reclaim_plan::ReclaimPlan;
    let plan = ReclaimPlan {
        goal_bytes: 10,
        items: vec![crate::reclaim_plan::PlanItem {
            id: "safe".into(),
            bytes: 10,
            estimate: false,
        }],
        selected_bytes: 10,
        measured_bytes: 10,
        estimated_bytes: 0,
        shortfall_bytes: 0,
        caution_bytes: 20,
    };
    assert!(can_apply_guided_plan(true, Some(4), 4, false, &plan));
    assert!(!can_apply_guided_plan(false, Some(4), 4, false, &plan));
    assert!(!can_apply_guided_plan(true, Some(3), 4, false, &plan));
    assert!(!can_apply_guided_plan(true, Some(4), 4, true, &plan));
}

#[test]
fn guided_plan_checks_only_named_safe_rows() {
    let mut rows = vec![
        rec_row("safe-a", Tier::Safe, true),
        rec_row("safe-b", Tier::Safe, true),
        rec_row("caution", Tier::Caution, true),
    ];
    let plan = crate::reclaim_plan::ReclaimPlan {
        goal_bytes: 10,
        items: vec![crate::reclaim_plan::PlanItem {
            id: "safe-b".into(),
            bytes: 10,
            estimate: false,
        }],
        selected_bytes: 10,
        measured_bytes: 10,
        estimated_bytes: 0,
        shortfall_bytes: 0,
        caution_bytes: 50,
    };
    apply_guided_plan(&mut rows, &plan);
    assert!(!rows[0].checked);
    assert!(rows[1].checked);
    assert!(!rows[2].checked);
}
```

Add this fixture helper inside the test module using the exact existing private types:

```rust
fn rec_row(id: &str, tier: Tier, checked: bool) -> RecRow {
    RecRow {
        rec: Rec {
            id: id.into(),
            title: id.into(),
            path: std::path::PathBuf::from(format!("/fixture/{id}")),
            display: format!("/fixture/{id}"),
            bytes: 10,
            tier,
            desc: "fixture",
            restore: "fixture",
            action: Action::Trash,
            command: None,
            allow_trash: true,
            allow_delete: true,
            note: String::new(),
            estimate: false,
        },
        checked,
        action: Action::Trash,
        expanded: false,
        status: RecStatus::Idle,
    }
}
```

- [ ] **Step 2: Verify the tests fail, then restore the concrete helper bodies**

Run:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::guided_plan
```

Expected: both tests fail at the deliberate `unimplemented!()` calls. Restore the concrete bodies shown in Step 1 and rerun; both pass.

- [ ] **Step 3: Add guide state and revision invalidation**

Import the planner:

```rust
use crate::reclaim_plan::{build_plan, parse_goal_gb, GoalError, ReclaimPlan, GB};
```

Add `GuidedReclaim` to `RailView`, returning to Summary:

```rust
enum RailView {
    Summary,
    GuidedReclaim,
    Reclaim,
    // existing variants remain unchanged
}

fn rail_back_target(view: RailView) -> Option<RailView> {
    match view {
        RailView::Summary => None,
        RailView::GuidedReclaim | RailView::Reclaim | RailView::Insights => {
            Some(RailView::Summary)
        }
        // existing Insights children remain unchanged
    }
}
```

Add these fields to `App`:

```rust
recs_revision: u64,
guide_goal_bytes: i64,
guide_custom_gb: String,
guide_goal_error: Option<GoalError>,
guide_acknowledged: bool,
guide_revision: Option<u64>,
guided_goal_for_review: Option<i64>,
```

Initialize them in `App::new()`:

```rust
recs_revision: 0,
guide_goal_bytes: 20 * GB,
guide_custom_gb: String::new(),
guide_goal_error: None,
guide_acknowledged: false,
guide_revision: None,
guided_goal_for_review: None,
```

At the start of `begin_scan`, invalidate the draft before starting the worker:

```rust
self.recs_revision = self.recs_revision.wrapping_add(1);
self.guide_revision = None;
self.guide_acknowledged = false;
self.guided_goal_for_review = None;
```

At the end of `on_scan_finished`, after `self.recs_built = true`, establish the new immutable finding revision and keep the default goal within used space:

```rust
self.guide_goal_bytes = (20 * GB).min(self.stats.used.max(GB));
self.guide_revision = Some(self.recs_revision);
self.guide_acknowledged = false;
self.guide_goal_error = None;
```

When `CleanEvent::Done` arrives, invalidate the recommendation revision because measured findings are stale until a rescan:

```rust
self.recs_revision = self.recs_revision.wrapping_add(1);
self.guide_revision = None;
self.guide_acknowledged = false;
```

- [ ] **Step 4: Add the apply method and test back navigation**

Add:

```rust
fn accept_guided_plan(&mut self, plan: &ReclaimPlan) {
    if !can_apply_guided_plan(
        self.guide_acknowledged,
        self.guide_revision,
        self.recs_revision,
        self.scanning(),
        plan,
    ) {
        return;
    }
    apply_guided_plan(&mut self.recs, plan);
    self.guided_goal_for_review = Some(plan.goal_bytes);
    self.guide_acknowledged = false;
    self.rail_view = RailView::Reclaim;
}
```

Extend the existing `rail_back_returns_each_detail_view_to_summary` test:

```rust
assert_eq!(
    rail_back_target(RailView::GuidedReclaim),
    Some(RailView::Summary)
);
```

- [ ] **Step 5: Run verification and commit**

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::guided_plan
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
git add src/app.rs
git commit -m "Add revision-safe reclaim plan state" -m "Invalidate guided selections across rescans and cleanup while applying only the Safe identifiers returned by the pure planner."
```

---

### Task 3: Everyday Guided-Reclaim Interface

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Phase 1 planner and Task 2 guide state
- Produces: Summary `Free up space` CTA and the `GuidedReclaim` rail with goal presets, custom validation, shortfall copy, evidence rows, acknowledgement, and plan application

- [ ] **Step 1: Route and expose the new primary entry point**

Add this branch before `RailView::Reclaim` in `draw_recs`:

```rust
RailView::GuidedReclaim => {
    self.draw_guided_reclaim(ui, rect);
    return;
}
```

In `draw_reclaim_summary`, change the large footer CTA label from `Review targets` to `Free up space`, its interaction id to `free-up-space`, and its click target to:

```rust
if enabled && response.clicked() {
    self.guide_revision = Some(self.recs_revision);
    self.guide_acknowledged = false;
    self.rail_view = RailView::GuidedReclaim;
}
```

Keep the Safe and Caution summary cards pointing to the manual `RailView::Reclaim`, so expert users retain direct review access.

- [ ] **Step 2: Add goal error and goal-change helpers with tests**

Add these pure helpers near the other UI helpers:

```rust
fn goal_error_copy(error: GoalError) -> &'static str {
    match error {
        GoalError::Empty => "Enter a goal in whole gigabytes.",
        GoalError::NotWholeGigabytes => "Use a whole number such as 25.",
        GoalError::Zero => "Choose at least 1 GB.",
        GoalError::ExceedsUsedSpace => "The goal cannot exceed currently used space.",
    }
}

fn plan_status_copy(plan: &ReclaimPlan) -> String {
    if plan.items.is_empty() {
        "No automatically safe targets are available in this scan.".into()
    } else if plan.shortfall_bytes > 0 {
        format!(
            "Safe targets provide about {}. Your goal is short by {}.",
            fmt_bytes(plan.selected_bytes),
            fmt_bytes(plan.shortfall_bytes)
        )
    } else {
        format!(
            "This Safe plan reaches the goal with about {} selected.",
            fmt_bytes(plan.selected_bytes)
        )
    }
}
```

Test exact empty, shortfall, and reached strings using fixture `ReclaimPlan` values. Run `cargo test app::tests::plan_status_copy` and verify the focused tests pass.

- [ ] **Step 3: Implement the guided rail**

Add a `draw_guided_reclaim` method using `panel_chrome` and a vertical `ScrollArea`. The method must perform state changes after the UI closure to avoid overlapping mutable borrows. Use this concrete flow:

```rust
fn draw_guided_reclaim(&mut self, ui: &mut egui::Ui, rect: Rect) {
    let palette = theme::palette(ui.ctx());
    let plan = build_plan(
        &self.recs.iter().map(|row| row.rec.clone()).collect::<Vec<_>>(),
        self.guide_goal_bytes,
    );
    let current = self.guide_revision == Some(self.recs_revision) && !self.scanning();
    let can_apply = can_apply_guided_plan(
        self.guide_acknowledged,
        self.guide_revision,
        self.recs_revision,
        self.scanning(),
        &plan,
    );
    let content = panel_chrome(
        ui,
        rect,
        "Free up space",
        Some(("Safe plan · local only".into(), palette.safe)),
    );
    let mut go_back = false;
    let mut choose_goal = None;
    let mut submit_custom = false;
    let mut apply = false;

    ui.allocate_new_ui(
        egui::UiBuilder::new().max_rect(content.shrink2(vec2(12.0, 8.0))),
        |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if ui.button("← Reclaim summary").clicked() {
                    go_back = true;
                }
                ui.heading("How much space do you need?");
                ui.horizontal(|ui| {
                    for gb in [10i64, 20, 50] {
                        if ui.button(format!("{gb} GB")).clicked() {
                            choose_goal = Some(gb * GB);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.guide_custom_gb)
                            .hint_text("Custom GB")
                            .desired_width(110.0),
                    );
                    if ui.button("Use custom goal").clicked() {
                        submit_custom = true;
                    }
                });
                if let Some(error) = self.guide_goal_error {
                    ui.colored_label(palette.caution, goal_error_copy(error));
                }
                ui.separator();
                ui.label(RichText::new(plan_status_copy(&plan)).color(
                    if plan.shortfall_bytes > 0 { palette.caution } else { palette.safe }
                ));
                for item in &plan.items {
                    let finding = self
                        .recs
                        .iter()
                        .find(|row| row.rec.id == item.id)
                        .map(|row| &row.rec);
                    let title = finding
                        .map(|rec| rec.title.as_str())
                        .unwrap_or(item.id.as_str());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).color(palette.ink));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{}{}",
                                    if item.estimate { "≈ " } else { "" },
                                    fmt_bytes(item.bytes)
                                ))
                                .font(theme::mono(10.0))
                                .color(if item.estimate { palette.caution } else { palette.safe }),
                            );
                        });
                    });
                    if let Some(rec) = finding {
                        ui.label(
                            RichText::new(rec.desc)
                                .font(theme::body(9.5))
                                .color(palette.muted),
                        );
                        ui.label(
                            RichText::new(format!("Afterward: {}", rec.restore))
                                .font(theme::body(9.0))
                                .color(palette.faint),
                        );
                    }
                }
                if plan.caution_bytes > 0 {
                    ui.label(
                        RichText::new(format!(
                            "More options: {} need review and remain unchecked.",
                            fmt_bytes(plan.caution_bytes)
                        ))
                        .color(palette.caution),
                    );
                }
                if !current {
                    ui.colored_label(
                        palette.caution,
                        "This plan is stale. Scan again before applying it.",
                    );
                }
                ui.checkbox(
                    &mut self.guide_acknowledged,
                    "I reviewed this Safe plan and will confirm each selected target.",
                );
                if ui
                    .add_enabled(can_apply, egui::Button::new("Review this plan"))
                    .clicked()
                {
                    apply = true;
                }
            });
        },
    );

    if go_back {
        self.rail_view = RailView::Summary;
    }
    if let Some(goal) = choose_goal {
        if goal <= self.stats.used {
            self.guide_goal_bytes = goal;
            self.guide_goal_error = None;
            self.guide_acknowledged = false;
            self.guide_revision = Some(self.recs_revision);
        } else {
            self.guide_goal_error = Some(GoalError::ExceedsUsedSpace);
        }
    }
    if submit_custom {
        match parse_goal_gb(&self.guide_custom_gb, self.stats.used) {
            Ok(goal) => {
                self.guide_goal_bytes = goal;
                self.guide_goal_error = None;
                self.guide_acknowledged = false;
                self.guide_revision = Some(self.recs_revision);
            }
            Err(error) => self.guide_goal_error = Some(error),
        }
    }
    if apply {
        self.accept_guided_plan(&plan);
    }
}
```

Replace the raw `ui.heading` call in the snippet with
`ui.label(RichText::new("How much space do you need?").font(theme::display_md(13.0)).color(palette.ink))`
so the new surface uses the existing adaptive native type roles. Do not alter
planner behavior or safety state to solve styling.

- [ ] **Step 4: Preserve minimum-window layout with a structural test**

Add this structural helper beside `WorkspaceLayout`:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
struct GuidedLayout {
    nav: Rect,
    body: Rect,
    action: Rect,
}

fn guided_layout(content: Rect) -> GuidedLayout {
    let inset = content.shrink2(vec2(12.0, 8.0));
    let nav = Rect::from_min_size(inset.min, vec2(inset.width(), 30.0));
    let action = Rect::from_min_max(
        pos2(inset.min.x, inset.max.y - 82.0),
        inset.max,
    );
    let body = Rect::from_min_max(
        pos2(inset.min.x, nav.max.y + 6.0),
        pos2(inset.max.x, action.min.y - 8.0),
    );
    GuidedLayout { nav, body, action }
}
```

Refactor `draw_guided_reclaim` to allocate the Back button only in
`layout.nav`, the goal controls/status/finding explanations only in a vertical
`ScrollArea` bounded by `layout.body`, and the acknowledgement plus
`Review this plan` button only in `layout.action`. The action allocation must
use this exact shape:

```rust
ui.allocate_new_ui(egui::UiBuilder::new().max_rect(layout.action), |ui| {
    ui.checkbox(
        &mut self.guide_acknowledged,
        "I reviewed this Safe plan and will confirm each selected target.",
    );
    if ui
        .add_enabled(can_apply, egui::Button::new("Review this plan"))
        .clicked()
    {
        apply = true;
    }
});
```

Remove those two controls from the scroll-body closure shown in Step 3. Add:

```rust
#[test]
fn guided_layout_reserves_a_fixed_non_overlapping_action_area() {
    let content = Rect::from_min_size(pos2(0.0, 0.0), vec2(390.0, 650.0));
    let layout = guided_layout(content);
    assert_eq!(layout.action.height(), 82.0);
    assert!(layout.nav.max.y < layout.body.min.y);
    assert!(layout.body.max.y < layout.action.min.y);
    assert!(layout.body.height() > 400.0);
}
```

This proves that at the minimum application window the action remains fixed
while all variable-length preview content scrolls above it.

Run:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::guided
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
```

Expected: guide state, copy, back target, and minimum-layout tests pass with the full suite.

- [ ] **Step 5: Commit the guided interface**

```sh
git add src/app.rs
git commit -m "Add everyday guided reclaim flow" -m "Lead with a plain-language space goal while keeping manual review available and Caution findings visibly optional."
```

---

### Task 4: Planned-versus-Actual Reclaim Outcome

**Files:**
- Modify: `src/reclaim_plan.rs`
- Modify: `src/app.rs`
- Modify: `src/clean.rs`

**Interfaces:**
- Consumes: selected `RecRow` values and `CleanEvent::{Result, Done}`
- Produces: `OutcomeTracker`, `ReclaimOutcome`, a guided result rail, and `clean::open_trash()`

- [ ] **Step 1: Write failing outcome tests**

Add these types and deliberately unimplemented methods to `reclaim_plan.rs`:

```rust
use std::collections::BTreeSet;

pub struct OutcomeTracker {
    goal_bytes: i64,
    planned_bytes: i64,
    planned_estimated_bytes: i64,
    item_ids: BTreeSet<String>,
    failed_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub goal_bytes: i64,
    pub planned_bytes: i64,
    pub planned_estimated_bytes: i64,
    pub actual_freed_bytes: i64,
    pub pending_trash_bytes: i64,
    pub goal_shortfall_bytes: i64,
    pub attempted_items: usize,
    pub failed_items: usize,
}

impl OutcomeTracker {
    pub fn new(
        _goal_bytes: i64,
        _planned_bytes: i64,
        _planned_estimated_bytes: i64,
        _item_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        unimplemented!()
    }

    pub fn record_result(&mut self, _id: &str, _ok: bool) {
        unimplemented!()
    }

    pub fn finish(self, _freed: i64, _pending: i64) -> ReclaimOutcome {
        unimplemented!()
    }
}
```

Add tests proving that failures are counted once, unrelated event identifiers are ignored, Trash bytes do not pretend to be already freed, and negative event values clamp to zero:

```rust
#[test]
fn outcome_separates_actual_free_space_from_pending_trash() {
    let mut tracker = OutcomeTracker::new(
        20 * GB,
        24 * GB,
        4 * GB,
        ["a".to_string(), "b".to_string()],
    );
    tracker.record_result("a", true);
    tracker.record_result("b", false);
    tracker.record_result("b", false);
    tracker.record_result("unrelated", false);
    let outcome = tracker.finish(8 * GB, 6 * GB);
    assert_eq!(outcome.actual_freed_bytes, 8 * GB);
    assert_eq!(outcome.pending_trash_bytes, 6 * GB);
    assert_eq!(outcome.goal_shortfall_bytes, 12 * GB);
    assert_eq!(outcome.attempted_items, 2);
    assert_eq!(outcome.failed_items, 1);
}
```

- [ ] **Step 2: Implement the tracker and pass focused tests**

Use:

```rust
impl OutcomeTracker {
    pub fn new(
        goal_bytes: i64,
        planned_bytes: i64,
        planned_estimated_bytes: i64,
        item_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            goal_bytes: goal_bytes.max(0),
            planned_bytes: planned_bytes.max(0),
            planned_estimated_bytes: planned_estimated_bytes.max(0),
            item_ids: item_ids.into_iter().collect(),
            failed_ids: BTreeSet::new(),
        }
    }

    pub fn record_result(&mut self, id: &str, ok: bool) {
        if !ok && self.item_ids.contains(id) {
            self.failed_ids.insert(id.to_string());
        }
    }

    pub fn finish(self, freed: i64, pending: i64) -> ReclaimOutcome {
        let actual_freed_bytes = freed.max(0);
        ReclaimOutcome {
            goal_bytes: self.goal_bytes,
            planned_bytes: self.planned_bytes,
            planned_estimated_bytes: self.planned_estimated_bytes,
            actual_freed_bytes,
            pending_trash_bytes: pending.max(0),
            goal_shortfall_bytes: self.goal_bytes.saturating_sub(actual_freed_bytes),
            attempted_items: self.item_ids.len(),
            failed_items: self.failed_ids.len(),
        }
    }
}
```

Run `cargo test reclaim_plan::tests::outcome` and then the whole planner test module.

- [ ] **Step 3: Track the actual selection when cleanup starts**

Import `OutcomeTracker` and `ReclaimOutcome` in `app.rs`. Add:

```rust
active_guided_reclaim: Option<OutcomeTracker>,
guided_outcome: Option<ReclaimOutcome>,
```

Initialize both to `None`. In `fire_reclaim`, collect the jobs first as today, then—only when `guided_goal_for_review` is set—derive the tracker from the final checked rows so manual changes in Review targets are reported honestly:

```rust
if let Some(goal_bytes) = self.guided_goal_for_review.take() {
    let selected: Vec<&RecRow> = self.recs.iter().filter(|row| row.checked).collect();
    self.active_guided_reclaim = Some(OutcomeTracker::new(
        goal_bytes,
        selected.iter().map(|row| row.rec.bytes).sum(),
        selected
            .iter()
            .filter(|row| row.rec.estimate)
            .map(|row| row.rec.bytes)
            .sum(),
        selected.iter().map(|row| row.rec.id.clone()),
    ));
    self.guided_outcome = None;
}
```

On each `CleanEvent::Result`, before mutating the row, call:

```rust
if let Some(tracker) = &mut self.active_guided_reclaim {
    tracker.record_result(&id, ok);
}
```

On `CleanEvent::Done`, finalize before returning:

```rust
if let Some(tracker) = self.active_guided_reclaim.take() {
    self.guided_outcome = Some(tracker.finish(freed, pending));
    self.rail_view = RailView::GuidedReclaim;
}
```

If the user leaves Review targets without starting cleanup, clear `guided_goal_for_review`; do not allow a later unrelated manual cleanup to inherit the old goal.

- [ ] **Step 4: Add the recoverable Trash link**

Add to `clean.rs`:

```rust
pub fn open_trash() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let _ = Command::new("/usr/bin/open")
        .arg(PathBuf::from(home).join(".Trash"))
        .spawn();
}
```

Import `open_trash` in `app.rs`. In `draw_guided_reclaim`, branch before the plan UI when `guided_outcome` exists. The result view must display:

- target goal;
- planned total, prefixed with `≈` when any planned bytes were estimates;
- actual freed bytes without `≈`;
- pending Trash bytes separately;
- failed item count when non-zero;
- remaining shortfall from actual freed bytes only;
- **Open Trash** only when `pending_trash_bytes > 0`;
- **Scan again** to refresh findings and start a new plan;
- **Back to summary** without starting a scan.

The result view must not auto-empty Trash and must not call `fire_reclaim`.

- [ ] **Step 5: Verify outcome behavior and commit**

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test reclaim_plan::tests
PATH="$HOME/.cargo/bin:$PATH" cargo test app::tests::guided
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
git add src/reclaim_plan.rs src/app.rs src/clean.rs
git commit -m "Report guided reclaim outcomes honestly" -m "Compare the final reviewed selection with actual freed and pending Trash bytes without treating estimates or recoverable Trash contents as free space."
```

---

### Task 5: Signed-App Proof, Documentation, and Release Gate

**Files:**
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-signed-ui.sh`
- Modify: `scripts/test-ui-smoke.sh`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md`

**Interfaces:**
- Consumes: the complete Guided Reclaim UI
- Produces: non-destructive signed-app navigation proof, contributor documentation, and shipped Phase 1 status

- [ ] **Step 1: Extend the smoke-tool syntax check before UI automation**

First add this required-command assertion to `scripts/test-ui-smoke.sh`:

```sh
grep -q 'guided-reclaim-visible' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose guided-reclaim-visible"
```

Extend the existing destructive-click deny pattern with
`Review this plan|Open Trash|Scan again`. Do not add `Free up space` to that
pattern: it is safe navigation and is intentionally clicked by the new smoke
command. The static guard must continue rejecting clicks on `Hold to reclaim`
and every existing storage action.

Run:

```sh
scripts/test-ui-smoke.sh
```

Expected: FAIL with `UI smoke runner must expose guided-reclaim-visible` until
the new command exists; after Step 2 it passes while allowing only the
`Free up space` navigation click.

- [ ] **Step 2: Add non-destructive Guided Reclaim automation**

Add this branch to `scripts/ui-smoke.applescript`:

```applescript
else if commandName is "guided-reclaim-visible" then
    if not (exists button "Free up space" of appGroup) then error "Guided Reclaim entry point is unavailable." number 1
    click button "Free up space" of appGroup
    delay 0.5
    if not (exists button "← Reclaim summary" of appGroup) then error "Guided Reclaim rail did not open." number 1
    if not (exists button "10 GB" of appGroup) then error "10 GB goal is unavailable." number 1
    if not (exists button "20 GB" of appGroup) then error "20 GB goal is unavailable." number 1
    if not (exists button "50 GB" of appGroup) then error "50 GB goal is unavailable." number 1
    if not (exists button "Review this plan" of appGroup) then error "Guided plan review control is unavailable." number 1
    return "PASS: Guided Reclaim rail available without applying or cleaning"
```

Call `ui guided-reclaim-visible` in `scripts/test-signed-ui.sh`, followed by `ui escape`, before the existing map/context-menu checks. Do not tick the acknowledgement, apply the plan, hold the reclaim control, open Trash, or start a scan.

- [ ] **Step 3: Update contributor and user documentation**

Add a README feature bullet explaining goal-based Safe-only planning, honest shortfalls, and planned-versus-actual results. Add usage rows for **Free up space**, goal presets/custom goal, **Review this plan**, and the fact that the existing hold remains the only execution boundary.

Add `reclaim_plan` to the AGENTS flat-module list and architecture table. Record these maintainer rules:

- the planner accepts `Rec` evidence and returns identifiers only;
- automatic plans never include Caution findings;
- pending Trash bytes are not actual freed bytes;
- a scan or cleanup invalidates the plan revision.

Mark only Phase 1 as shipped in the v3 design after signed-app verification succeeds.

- [ ] **Step 4: Run all local and signed release gates**

Run exactly:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check
PATH="$HOME/.cargo/bin:$PATH" cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
PATH="$HOME/.cargo/bin:$PATH" ./make-app.sh
codesign --verify --deep --strict /Applications/DiskDeck.app
scripts/test-signed-ui.sh
```

Expected:

- all Rust tests pass;
- UI smoke tooling compiles and its destructive-click guard passes;
- privacy and personal-identity guards pass;
- the app is signed as the stable personal development identity with bundle id `com.buddyhq.headroom-rs`;
- the Guided Reclaim rail opens in the signed app without applying a plan;
- existing map, Back, Insights, Moved Items, Growth Watch, Developer Lens, APFS, leftovers, monitor, and file-review navigation still pass;
- `dist/DiskDeck.zip` contains the signed app and installer.

Use eyes-on-screen verification at 1180×740 and 1480×920 in both system light and dark appearances. Confirm no overlap, readable typography, visible shortfall/estimate language, keyboard focus, and that Caution findings are not checked after applying a Safe plan. Exercise execution only against fixture recommendations or the smallest recoverable Safe Trash item permitted by `AGENTS.md`; never run delete or command actions on owner data.

- [ ] **Step 5: Commit, push, and watch GitHub CI**

```sh
git add AGENTS.md README.md src scripts docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md
git commit -m "Ship Guided Reclaim" -m "Give everyday Mac users a Safe-only space goal, transparent shortfalls, and an honest planned-versus-actual result without weakening DiskDeck's cleanup boundary."
git push origin main
gh run watch "$(gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```

Expected: the pushed `main` revision completes the macOS CI job successfully. Do not start the Phase 2 implementation plan until the signed-app proof and GitHub run are both green.
