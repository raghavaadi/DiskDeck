# Storage Forecasting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Use completed local scan history to explain whether free space is stable, improving, volatile, or likely to cross the configured low-space threshold within an honest time range.

**Architecture:** Extend DiskDeck history snapshots from `DDHIST1` to backward-compatible `DDHIST2` records that include volume capacity and free bytes captured only after a completed foreground scan. A pure `forecast` module filters invalid/incompatible observations, derives a robust median free-space-loss rate, assigns evidence confidence, and returns typed states consumed by Growth Watch and the existing menu-monitor settings rail without introducing background scans or a second history store.

**Tech Stack:** Rust, existing lossless history codec, egui 0.29/eframe, `statfs`-backed `scan::disk_stats`, signed AppleScript smoke tooling.

## Global Constraints

- No new crate dependencies.
- Forecast data stays local and reuses the bounded 12-snapshot history.
- Only completed foreground scans record capacity observations.
- `DDHIST1` snapshots remain readable and participate in growth views, but cannot be treated as capacity evidence.
- A forecast requires at least three compatible observations spanning at least seven days.
- Confidence thresholds are exactly: Early = 3 observations/7 days, Developing = 5/14, Reliable = 8/30.
- Clock duplicates, impossible capacity values, and observations from a different capacity are excluded.
- Flat, improving, volatile, and insufficient evidence produce explicit non-forecast states.
- Forecasted bytes are never reclaimable bytes.
- The menu monitor must not open history, traverse the disk, or start a scan from its five-minute loop.
- Full scans remain explicitly user initiated.

---

### Task 1: Pure Robust Forecast Model

**Files:**
- Create: `src/forecast.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces: `CapacityPoint`, `Confidence`, `StorageForecast`, `ForecastState`, `analyze(&[CapacityPoint], i64)`
- Consumes: timestamped volume total/free byte observations and the configured threshold in bytes

- [ ] **Step 1: Register `forecast` and write failing model tests**

Add `mod forecast;` to `src/main.rs`. Create the public types below in `src/forecast.rs`, with `analyze` initially using `unimplemented!()`:

```rust
pub const DAY_MS: i64 = 86_400_000;
const MIN_RATE_BYTES_PER_DAY: i64 = 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityPoint {
    pub captured_at_ms: i64,
    pub total_bytes: i64,
    pub free_bytes: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Early,
    Developing,
    Reliable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageForecast {
    pub confidence: Confidence,
    pub days_low: i64,
    pub days_high: i64,
    pub bytes_per_day: i64,
    pub observations: usize,
    pub span_days: i64,
    pub latest_free_bytes: i64,
    pub threshold_bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForecastState {
    NeedHistory { observations: usize, span_days: i64 },
    AlreadyLow { free_bytes: i64, threshold_bytes: i64 },
    Flat { observations: usize, span_days: i64 },
    Improving { bytes_per_day: i64, observations: usize, span_days: i64 },
    Volatile { observations: usize, span_days: i64 },
    Estimate(StorageForecast),
}

pub fn analyze(_points: &[CapacityPoint], _threshold_bytes: i64) -> ForecastState {
    unimplemented!("storage forecasting is introduced in v3 phase 2")
}
```

Tests must cover:

```rust
const GB: i64 = 1_000_000_000;

fn point(day: i64, free_gb: i64) -> CapacityPoint {
    CapacityPoint {
        captured_at_ms: day * DAY_MS,
        total_bytes: 250 * GB,
        free_bytes: free_gb * GB,
    }
}

#[test]
fn needs_three_points_and_seven_days() {
    assert_eq!(
        analyze(&[point(0, 80), point(7, 70)], 15 * GB),
        ForecastState::NeedHistory { observations: 2, span_days: 7 }
    );
    assert_eq!(
        analyze(&[point(0, 80), point(3, 76), point(6, 72)], 15 * GB),
        ForecastState::NeedHistory { observations: 3, span_days: 6 }
    );
}

#[test]
fn confidence_follows_exact_evidence_thresholds() {
    let early = analyze(&[point(0, 80), point(3, 77), point(7, 73)], 15 * GB);
    assert!(matches!(early, ForecastState::Estimate(StorageForecast { confidence: Confidence::Early, .. })));

    let developing = analyze(
        &[point(0, 90), point(4, 86), point(8, 82), point(11, 79), point(14, 76)],
        15 * GB,
    );
    assert!(matches!(developing, ForecastState::Estimate(StorageForecast { confidence: Confidence::Developing, .. })));

    let reliable = analyze(
        &[point(0, 100), point(5, 95), point(10, 90), point(15, 85), point(20, 80), point(24, 76), point(27, 73), point(30, 70)],
        15 * GB,
    );
    assert!(matches!(reliable, ForecastState::Estimate(StorageForecast { confidence: Confidence::Reliable, .. })));
}

#[test]
fn median_rate_ignores_one_cleanup_spike() {
    let state = analyze(
        &[point(0, 80), point(2, 76), point(4, 72), point(6, 90), point(8, 86)],
        15 * GB,
    );
    let ForecastState::Estimate(forecast) = state else { panic!("expected estimate") };
    assert_eq!(forecast.bytes_per_day, 2 * GB);
    assert!(forecast.days_low <= forecast.days_high);
}

#[test]
fn flat_improving_volatile_and_already_low_are_not_estimates() {
    assert!(matches!(
        analyze(&[point(0, 80), point(4, 80), point(8, 80)], 15 * GB),
        ForecastState::Flat { .. }
    ));
    assert!(matches!(
        analyze(&[point(0, 70), point(4, 75), point(8, 80)], 15 * GB),
        ForecastState::Improving { .. }
    ));
    assert!(matches!(
        analyze(&[point(0, 80), point(2, 70), point(4, 82), point(6, 70), point(8, 82)], 15 * GB),
        ForecastState::Volatile { .. }
    ));
    assert!(matches!(
        analyze(&[point(0, 20), point(4, 16), point(8, 14)], 15 * GB),
        ForecastState::AlreadyLow { .. }
    ));
}

#[test]
fn invalid_duplicate_and_different_capacity_points_are_excluded() {
    let mut wrong_capacity = point(2, 78);
    wrong_capacity.total_bytes = 500 * GB;
    let impossible = CapacityPoint {
        captured_at_ms: 3 * DAY_MS,
        total_bytes: 250 * GB,
        free_bytes: 300 * GB,
    };
    let state = analyze(
        &[point(0, 80), wrong_capacity, impossible, point(7, 73), point(7, 72)],
        15 * GB,
    );
    assert!(matches!(state, ForecastState::NeedHistory { observations: 2, .. }));
}
```

- [ ] **Step 2: Run RED**

Run `cargo test forecast::tests`; verify every test fails only at `analyze`.

- [ ] **Step 3: Implement filtering, confidence, median rate, volatility, and ranges**

Implement these private helpers and `analyze`:

```rust
fn median(values: &mut [i64]) -> i64 {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        ((values[middle - 1] as i128 + values[middle] as i128) / 2) as i64
    }
}

fn confidence(observations: usize, span_days: i64) -> Option<Confidence> {
    if observations >= 8 && span_days >= 30 {
        Some(Confidence::Reliable)
    } else if observations >= 5 && span_days >= 14 {
        Some(Confidence::Developing)
    } else if observations >= 3 && span_days >= 7 {
        Some(Confidence::Early)
    } else {
        None
    }
}
```

`analyze` must:

1. reject negative timestamps, non-positive totals, free bytes below zero, and free bytes above total;
2. sort by timestamp, collapse duplicate timestamps, and retain only the latest observation's exact volume total;
3. return `NeedHistory` before computing a rate when confidence is unavailable;
4. return `AlreadyLow` when latest free is at or below threshold;
5. calculate every interval as `(previous.free - current.free) * DAY_MS / elapsed_ms` using `i128` intermediate math;
6. use the interval median as `bytes_per_day`;
7. return `Flat` when the median magnitude is below 10 MB/day and `Improving` when the median is negative;
8. return `Volatile` when at most half of intervals lose space or when median absolute deviation exceeds twice the positive median;
9. derive uncertainty from the larger of median absolute deviation and the confidence floor (35% Early, 25% Developing, 15% Reliable);
10. calculate `days_low` with the faster rate and `days_high` with the slower positive rate, clamping both to at least one day and `days_high` to no more than five times the central estimate.

- [ ] **Step 4: Verify and commit**

```sh
cargo fmt -- --check
cargo test forecast::tests
cargo test --locked
git add src/main.rs src/forecast.rs
git commit -m "Add robust local storage forecast model" -m "Require multi-day evidence and use median interval rates so cleanup spikes, invalid capacity observations, and volatile history never become false precision."
```

---

### Task 2: Backward-Compatible Capacity History

**Files:**
- Modify: `src/history.rs`
- Modify: `src/app.rs` only if public type imports require formatting changes

**Interfaces:**
- Consumes: `scan::disk_stats()` after `ScanState::Done`
- Produces: v2 snapshots containing optional `volume_total_bytes`/`volume_free_bytes` and `GrowthWatch.capacity: Vec<CapacityPoint>`

- [ ] **Step 1: Write RED codec and extraction tests**

Rename the current magic to `MAGIC_V1`, add `MAGIC_V2 = b"DDHIST2\0"`, and extend private `Snapshot` with:

```rust
volume_total_bytes: Option<i64>,
volume_free_bytes: Option<i64>,
```

Extend public `GrowthWatch` with:

```rust
pub capacity: Vec<crate::forecast::CapacityPoint>,
```

Add tests that fail until the codec and builder support capacity:

```rust
#[test]
fn v2_codec_round_trips_capacity_observation() {
    let snapshot = Snapshot {
        captured_at_ms: 123,
        root: PathBuf::from("/System/Volumes/Data"),
        total_bytes: 456,
        volume_total_bytes: Some(250_000_000_000),
        volume_free_bytes: Some(80_000_000_000),
        entries: vec![entry("Users", 789)],
    };
    assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
}

#[test]
fn v1_codec_remains_readable_without_capacity_evidence() {
    let bytes = encode_v1_for_test(&Snapshot {
        captured_at_ms: 123,
        root: PathBuf::from("/System/Volumes/Data"),
        total_bytes: 456,
        volume_total_bytes: None,
        volume_free_bytes: None,
        entries: vec![],
    });
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.volume_total_bytes, None);
    assert_eq!(decoded.volume_free_bytes, None);
}

#[test]
fn growth_watch_exposes_only_complete_capacity_pairs() {
    let snapshots = vec![
        snapshot_with_capacity(10, 100, Some((250, 80))),
        snapshot_with_capacity(20, 110, None),
        snapshot_with_capacity(30, 120, Some((250, 70))),
    ];
    let watch = build_growth_watch(&snapshots, &[]);
    assert_eq!(watch.capacity.len(), 2);
    assert_eq!(watch.capacity[0].free_bytes, 80);
    assert_eq!(watch.capacity[1].free_bytes, 70);
}
```

Add fixture helpers with all fields explicit. Mechanically add `volume_total_bytes: None` and `volume_free_bytes: None` to every existing `Snapshot` fixture; do not weaken existing assertions.

- [ ] **Step 2: Run RED**

Run the three focused tests. Verify v2 round-trip or capacity extraction fails before implementation while existing v1 decoding behavior remains the target.

- [ ] **Step 3: Implement v2 encoding and dual-version decoding**

`encode` must always write `MAGIC_V2`, followed by captured time, scanned total, volume total (or `-1`), volume free (or `-1`), root, and entries. `decode` must read either magic:

```rust
let magic = read_exact::<8>(&mut cursor)?;
let version = if &magic == MAGIC_V2 {
    2
} else if &magic == MAGIC_V1 {
    1
} else {
    return Err("snapshot format is not supported".into());
};
let captured_at_ms = read_i64(&mut cursor)?;
let total_bytes = read_i64(&mut cursor)?;
let (volume_total_bytes, volume_free_bytes) = if version == 2 {
    let total = read_i64(&mut cursor)?;
    let free = read_i64(&mut cursor)?;
    (
        (total >= 0).then_some(total),
        (free >= 0).then_some(free),
    )
} else {
    (None, None)
};
```

The test-only v1 encoder must duplicate the old field order exactly and stay inside `#[cfg(test)]`.

- [ ] **Step 4: Record capacity only with completed scans and expose points**

Change `capture` to accept `capacity: Option<(i64, i64)>` and populate both optional fields. Existing capture tests pass `None`. In the `record_scan` worker, call `disk_stats()` immediately before capture and pass capacity only when `total > 0`, `free >= 0`, and `free <= total`.

In `build_growth_watch`, add:

```rust
let capacity = snapshots
    .iter()
    .filter_map(|snapshot| {
        Some(crate::forecast::CapacityPoint {
            captured_at_ms: snapshot.captured_at_ms,
            total_bytes: snapshot.volume_total_bytes?,
            free_bytes: snapshot.volume_free_bytes?,
        })
    })
    .collect();
```

Return it in `GrowthWatch`. Do not alter timeline, recurring-grower, watched-folder, corruption, or retention semantics.

- [ ] **Step 5: Verify and commit**

```sh
cargo fmt -- --check
cargo test history::tests
cargo test forecast::tests
cargo test --locked
git add src/history.rs src/app.rs
git commit -m "Record capacity in compatible scan history" -m "Write new completed scans as DDHIST2 while preserving DDHIST1 growth history and withholding forecasts where historical free-space evidence does not exist."
```

---

### Task 3: Growth Watch Forecast and Menu-Monitor Explanation

**Files:**
- Modify: `src/app.rs`
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-ui-smoke.sh`

**Interfaces:**
- Consumes: `forecast::analyze(&GrowthWatch.capacity, monitor threshold)`
- Produces: plain-language forecast card in Growth Watch and read-only latest-foreground forecast in Menu-bar monitor settings

- [ ] **Step 1: Write RED copy tests**

Add pure helpers to `app.rs` with tests first:

```rust
fn confidence_copy(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Early => "Early estimate",
        Confidence::Developing => "Developing pattern",
        Confidence::Reliable => "Reliable trend",
    }
}

fn forecast_headline(state: &ForecastState) -> String {
    match state {
        ForecastState::NeedHistory { observations, span_days } => format!(
            "Not enough history yet — {observations} compatible scan(s) across {span_days} day(s)."
        ),
        ForecastState::AlreadyLow { .. } => "Storage is already below your low-space threshold.".into(),
        ForecastState::Flat { .. } => "Storage use is roughly steady.".into(),
        ForecastState::Improving { .. } => "Free space is improving across recent scans.".into(),
        ForecastState::Volatile { .. } => "Recent storage changes are too volatile for an honest forecast.".into(),
        ForecastState::Estimate(value) => {
            let low_weeks = (value.days_low + 6) / 7;
            let high_weeks = (value.days_high + 6) / 7;
            format!("At the recent rate, storage may become low in about {low_weeks}–{high_weeks} weeks.")
        }
    }
}
```

Test exact copy for every state, including a 35–49 day estimate yielding `5–7 weeks`. Run the focused tests and confirm the temporary `unimplemented!()` versions fail before adding these bodies.

- [ ] **Step 2: Add the Growth Watch forecast card**

Add an App helper:

```rust
fn current_forecast(&self) -> ForecastState {
    crate::forecast::analyze(
        &self.growth_watch.capacity,
        self.monitor_settings.threshold_gb as i64 * 1_000_000_000,
    )
}
```

In `draw_growth_watch`, render a `STORAGE FORECAST` card before `TOTAL STORAGE TREND`. It must show `forecast_headline`, confidence/evidence/rate details for `Estimate`, and a safe **Scan now** button only for `NeedHistory`. Clicking **Scan now** calls `begin_scan()` after the UI closure; it never starts automatically.

For an estimate, evidence copy is:

```rust
format!(
    "{} · {} compatible scans · {} days · median loss {}/day",
    confidence_copy(value.confidence),
    value.observations,
    value.span_days,
    fmt_bytes(value.bytes_per_day)
)
```

The card must explicitly say the forecast is not reclaimable space.

- [ ] **Step 3: Reuse the already-loaded forecast in Menu-bar monitor settings**

In `draw_menu_monitor`, add a `FOREGROUND FORECAST` section using the same `current_forecast` value and headline. Label it “Updated only after a completed foreground scan.” Do not call `load_growth_watch`, `start_scan`, or any filesystem function from `update_menu_monitor` or `MenuBarItem::update`.

- [ ] **Step 4: Extend non-destructive signed smoke coverage**

In the existing `growth-watch-visible` AppleScript branch, after opening Growth Watch, assert `static text "STORAGE FORECAST"` exists. In the `menu-monitor-visible` branch, assert `static text "FOREGROUND FORECAST"` exists. Add `Scan now` to the destructive-click deny pattern so smoke automation cannot trigger a scan.

Run:

```sh
scripts/test-ui-smoke.sh
cargo test app::tests::forecast_
cargo test --locked
```

- [ ] **Step 5: Commit**

```sh
git add src/app.rs scripts/ui-smoke.applescript scripts/test-ui-smoke.sh
git commit -m "Explain local storage forecasts" -m "Show confidence, evidence span, and honest non-estimate states in Growth Watch while reusing only already-loaded foreground evidence in menu-monitor settings."
```

---

### Task 4: Documentation, Signed Proof, and Hosted CI

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md`

- [ ] **Step 1: Update docs without overclaiming**

README must explain the 3/7 minimum, confidence labels, local-only v2 capacity evidence, explicit Scan now behavior, and the fact that old snapshots still support growth but not forecast history. AGENTS must add `forecast.rs`, document the dual codec and invalid-observation rules, and prohibit history reads/background scans in the menu loop.

Mark Phase 2 shipped in the v3 spec only after signed proof succeeds.

- [ ] **Step 2: Run release gates and inspect both appearances**

```sh
cargo fmt -- --check
cargo test --locked
scripts/test-ui-smoke.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
git diff --check
./make-app.sh
codesign --verify --deep --strict /Applications/DiskDeck.app
scripts/test-signed-ui.sh
```

Visually verify Growth Watch and Menu-bar monitor at minimum and typical sizes in light and dark appearances. Confirm forecast cards scroll without covering navigation, insufficient history asks for a manual scan, confidence is not color-only, and no background scan starts while observing the app.

- [ ] **Step 3: Commit, merge, rebuild from main, push, and watch CI**

```sh
git add README.md AGENTS.md docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md
git commit -m "Ship Storage Forecasting" -m "Document the evidence thresholds and backward-compatible local capacity history behind DiskDeck's honest time-to-low guidance."
```

Use the finishing-a-development-branch workflow to fast-forward the verified branch into `main`, rebuild `dist/DiskDeck.zip` from that exact revision, push, and require the hosted macOS job to finish green before beginning Phase 3.
