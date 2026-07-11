# DiskDeck Adaptive Native Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace DiskDeck's dark-only flight-deck chrome with the approved native macOS light appearance and calm observatory dark appearance while preserving every scan, reclaim, offload, permission, and confirmation invariant.

**Architecture:** Introduce a copyable semantic `Palette` selected from egui's active `Theme`, configure separate light and dark egui `Visuals`, and migrate UI paint code from fixed color constants to palette fields. Keep application state and safety orchestration in `app.rs`; only presentation geometry, visible copy, and paint values change. Treemap layout remains untouched and its paint layer reads the same adaptive palette.

**Tech Stack:** Rust 2021, egui 0.29, eframe 0.29, existing in-module unit tests, signed macOS bundle via `make-app.sh`.

## Global Constraints

- `CFBundleIdentifier` remains exactly `com.buddyhq.headroom-rs`.
- The default signing identity remains exactly `Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)`.
- The scan root remains exactly `/System/Volumes/Data`.
- Removal still requires a checked recommendation and the existing `HOLD_SECS = 0.9` hold.
- `Safe` recommendations remain pre-selected; `Caution` recommendations remain unselected.
- No new crate dependencies.
- `spaced()` remains an identity function and the display family keeps proportional fallback fonts.
- Repaint remains conditional; never request it unconditionally.

---

## File structure

- `src/theme.rs`: owns adaptive semantic palettes, light/dark egui visuals, typography roles, alpha helpers, and palette tests.
- `src/app.rs`: owns the single-screen layout, toolbar, overview, reclaim rail, dialogs, activity surface, and all existing application state.
- `src/treemap.rs`: keeps layout logic and changes only adaptive painting colors plus paint-color tests.
- `src/main.rs`: keeps app identity and window constraints; no behavioral changes expected.
- `docs/superpowers/specs/2026-07-11-diskdeck-adaptive-native-overhaul-design.md`: approved design contract.

### Task 1: Adaptive semantic theme

**Files:**
- Modify: `src/theme.rs`
- Test: `src/theme.rs` in-module tests

**Interfaces:**
- Consumes: `egui::Theme`, `egui::Context`, existing bundled Saira font assets.
- Produces: `Palette::for_theme(Theme) -> Palette`, `palette(&Context) -> Palette`, `Palette::{accent_dim,safe_dim,caution_dim,danger_dim}(u8) -> Color32`, and `install(&Context)` configured for `ThemePreference::System`.

- [ ] **Step 1: Write failing palette tests**

```rust
#[test]
fn palettes_keep_readable_text_and_distinct_semantics() {
    for theme in [Theme::Light, Theme::Dark] {
        let p = Palette::for_theme(theme);
        assert!(contrast_ratio(p.ink, p.canvas) >= 7.0);
        assert!(contrast_ratio(p.muted, p.surface) >= 4.5);
        assert_ne!(p.accent, p.safe);
        assert_ne!(p.safe, p.caution);
        assert_ne!(p.caution, p.danger);
    }
}

#[test]
fn palettes_match_approved_canvas_values() {
    assert_eq!(Palette::for_theme(Theme::Light).canvas, Color32::from_rgb(0xed, 0xf1, 0xf5));
    assert_eq!(Palette::for_theme(Theme::Dark).canvas, Color32::from_rgb(0x10, 0x15, 0x1d));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `~/.cargo/bin/cargo test theme::tests -- --nocapture`

Expected: compilation fails because `Palette` and `contrast_ratio` do not exist.

- [ ] **Step 3: Implement `Palette` and dual visuals**

Implement a `#[derive(Clone, Copy)] pub struct Palette` containing `canvas`, `toolbar`, `surface`, `surface_raised`, `edge`, `edge_soft`, `ink`, `muted`, `faint`, `accent`, `safe`, `caution`, and `danger`. `Palette::for_theme` must return the exact values from the approved specification. Alpha helpers preserve each semantic color's RGB and replace only alpha.

In `install`, keep the font fallback construction, call `ctx.set_theme(egui::ThemePreference::System)`, and install a separately configured `Visuals` instance with `ctx.set_visuals_of(Theme::Light, ...)` and `ctx.set_visuals_of(Theme::Dark, ...)`. Selection uses Accent; active destructive meaning remains explicit at call sites rather than becoming a global widget default.

- [ ] **Step 4: Run theme and full tests**

Run: `~/.cargo/bin/cargo test theme::tests -- --nocapture && ~/.cargo/bin/cargo test`

Expected: palette tests pass and the existing 28 tests remain green.

- [ ] **Step 5: Commit the adaptive theme foundation**

```bash
git add src/theme.rs
git commit -m "Add adaptive semantic color system"
```

### Task 2: Native panel chrome and workspace hierarchy

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs` in-module tests

**Interfaces:**
- Consumes: `theme::palette(&Context) -> Palette` from Task 1 and all existing `App` state.
- Produces: `WorkspaceLayout::from_rect(Rect) -> WorkspaceLayout`, restyled `panel_chrome`, `ghost_button`, `top_bar`, `central`, `draw_capacity`, and `draw_telemetry` functions.

- [ ] **Step 1: Write failing geometry tests**

```rust
#[test]
fn workspace_layout_preserves_map_space_at_minimum_window() {
    let full = Rect::from_min_size(Pos2::ZERO, vec2(766.0, 564.0));
    let layout = WorkspaceLayout::from_rect(full);
    assert_eq!(layout.overview.height(), 142.0);
    assert!(layout.map.height() >= 410.0);
    assert_eq!(layout.overview.min, full.min);
    assert_eq!(layout.map.max, full.max);
}

#[test]
fn workspace_layout_keeps_twelve_point_gap() {
    let full = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let layout = WorkspaceLayout::from_rect(full);
    assert_eq!(layout.map.min.y - layout.overview.max.y, 12.0);
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `~/.cargo/bin/cargo test app::tests::workspace_layout -- --nocapture`

Expected: compilation fails because `WorkspaceLayout` does not exist.

- [ ] **Step 3: Implement the native workspace shell**

Add a private `WorkspaceLayout { overview: Rect, map: Rect }` with a 142 px overview and 12 px gap. Replace HUD corner brackets with 12 px rounded surfaces, a one-pixel semantic edge, sentence-case headings, and no decorative amber lines. Replace fixed theme constants with a local `let palette = theme::palette(ui.ctx())` or `theme::palette(ctx)`.

Draw the approved stacked-disk toolbar mark with three quiet platter curves and a small Accent spindle. Use `DiskDeck`, `Macintosh HD`, `Full Disk Access`, and `Rescan` / `Stop scan` visible copy. Keep the exact button actions and Full Disk Access hover text.

Replace `draw_gauge` with `draw_capacity`: a circular usage ring, `used` as the primary value, total/free supporting copy, and Danger only at the existing 85% threshold. Restyle telemetry into four facts plus the current-path/progress row, retaining every counter and no-access explainer.

- [ ] **Step 4: Run formatting, geometry tests, and full tests**

Run: `~/.cargo/bin/cargo fmt -- --check && ~/.cargo/bin/cargo test app::tests::workspace_layout -- --nocapture && ~/.cargo/bin/cargo test`

Expected: formatting check and all tests pass.

- [ ] **Step 5: Commit the workspace hierarchy**

```bash
git add src/app.rs
git commit -m "Redesign DiskDeck workspace hierarchy"
```

### Task 3: Adaptive storage map, reclaim rail, activity, and dialogs

**Files:**
- Modify: `src/treemap.rs`
- Modify: `src/app.rs`
- Test: `src/treemap.rs` in-module tests

**Interfaces:**
- Consumes: `Palette` fields and alpha helpers from Task 1; `collect_items`, `squarify`, recommendation and clean/offload state already present.
- Produces: `map_fills(Theme) -> [Color32; 6]` and the complete Adaptive Native presentation.

- [ ] **Step 1: Write failing treemap palette tests**

```rust
#[test]
fn map_palettes_adapt_without_changing_category_count() {
    let light = map_fills(egui::Theme::Light);
    let dark = map_fills(egui::Theme::Dark);
    assert_eq!(light.len(), 6);
    assert_eq!(dark.len(), 6);
    assert_ne!(light, dark);
    assert_eq!(light[0], Color32::from_rgb(0x3e, 0x88, 0xb1));
    assert_eq!(dark[0], Color32::from_rgb(0x24, 0x5a, 0x75));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `~/.cargo/bin/cargo test treemap::tests::map_palettes -- --nocapture`

Expected: compilation fails because `map_fills` does not exist.

- [ ] **Step 3: Implement adaptive treemap painting**

Replace the dark-only `FILLS` constant with `map_fills(theme)` returning the approved ocean/teal/slate/sand families. In `paint`, derive the current palette from `ui.ctx()`. Use Accent for hover, Danger for denied, `surface_raised` for synthetic items, Ink for names, and Muted for byte counts.

- [ ] **Step 4: Restyle the remaining app surfaces without changing behavior**

In `app.rs`, migrate `draw_map`, `recs_panel`, `rec_card`, `reclaim_footer`, `offload_dialog`, `ops_panel`, and `stamp_overlay` to the semantic palette. Change visible headings to `Storage map`, `Reclaimable`, and `Activity`; change group copy to `Safe · regenerates automatically` and `Review · may require a download`.

The reclaim footer must compute whether any selected row is `Tier::Caution` or has `Action::Delete`: use Safe for an all-safe selection, Caution otherwise, and keep the 900 ms hold and disabled behavior exactly intact. Preserve command locking, expansion, Trash/Delete switching, and every offload acknowledgement and capacity gate.

- [ ] **Step 5: Run formatting and all tests**

Run: `~/.cargo/bin/cargo fmt -- --check && ~/.cargo/bin/cargo test`

Expected: all existing and new tests pass.

- [ ] **Step 6: Commit the complete adaptive presentation**

```bash
git add src/app.rs src/treemap.rs
git commit -m "Complete adaptive native interface"
```

### Task 4: Signed bundle and visual proof

**Files:**
- Modify only if visual defects are discovered: `src/theme.rs`, `src/app.rs`, `src/treemap.rs`
- Verify: `make-app.sh`, `/Applications/DiskDeck.app`, `dist/DiskDeck.zip`

**Interfaces:**
- Consumes: completed UI and existing signed ship pipeline.
- Produces: signed app proof in Light and Dark appearances at default and minimum window sizes.

- [ ] **Step 1: Run the repository safety gate**

Run: `scripts/test-pre-commit.sh`

Expected: all privacy, secret, artifact, and hook tests pass.

- [ ] **Step 2: Build and install through the signed ship path**

Run: `./make-app.sh`

Expected: release build succeeds, `/Applications/DiskDeck.app` is signed and installed, and `dist/DiskDeck.zip` is produced.

- [ ] **Step 3: Verify immutable bundle identity**

```bash
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' '/Applications/DiskDeck.app/Contents/Info.plist'
codesign -dv --verbose=4 '/Applications/DiskDeck.app' 2>&1 | rg 'Authority=Apple Development: aadithyaraghav@gmail.com|Identifier=com.buddyhq.headroom-rs'
```

Expected: bundle identifier is `com.buddyhq.headroom-rs` and the expected Apple Development authority is present.

- [ ] **Step 4: Launch and visually verify both appearances**

Open `/Applications/DiskDeck.app`, inspect at 1480 × 920 and 1180 × 740 in macOS Light and Dark appearances, and verify: readable type with no tofu, native toolbar, circular capacity card, live map growth, adaptive treemap colors, recommendation expansion, action chip semantics, 900 ms hold fill, Activity feed, offload dialog, and no clipped controls.

- [ ] **Step 5: Run final tests after visual corrections**

Run: `~/.cargo/bin/cargo test && scripts/test-pre-commit.sh`

Expected: all tests and repository guards pass.

- [ ] **Step 6: Commit the design documents and any visual corrections**

```bash
git add -f docs/superpowers/specs/2026-07-11-diskdeck-adaptive-native-overhaul-design.md docs/superpowers/plans/2026-07-11-diskdeck-adaptive-native-overhaul.md
git add src/theme.rs src/app.rs src/treemap.rs
git commit -m "Document adaptive native overhaul"
```

- [ ] **Step 7: Push the verified main branch**

Run: `git push origin main`

Expected: `origin/main` advances to the final verified commit.

