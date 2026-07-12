# Developer Deep Dive Implementation Plan

> **Execution note:** Follow this plan task by task in an isolated worktree. Keep every command and discovered-path boundary covered by a failing test before implementation.

**Goal:** Turn Developer Lens into an opt-in, evidence-first workspace that explains Docker, Xcode, and project storage without double counting, starting another disk walk, or turning a discovered path into cleanup authority.

**Architecture:** Replace the current flat `developer::analyze` output with a pure `DeveloperReport` assembled from existing vetted `Rec` values plus the already-compacted scan tree. Project discovery performs one in-memory traversal of retained nodes and at most 1,000 bounded marker checks near the largest candidates. Docker details come from one fixed, read-only `docker system df` invocation on a worker thread; its inside-VM categories are explanatory children of the measured Docker footprint and never contribute to totals. The egui rail consumes the report asynchronously and exposes evidence through read-only disclosures only.

**Tech stack:** Rust standard library, existing `Arc<Node>` compact scan tree, existing `Rec` safety evidence, egui 0.29/eframe, mpsc workers, signed AppleScript UI smoke.

## Non-negotiable boundaries

- Developer Deep Dive never creates a `Rec`, `CleanJob`, command cleanup action, checkbox selection, or movable path.
- Every actionable finding still resolves to an existing vetted `Rec`; discovered project output is explanation/reveal-only.
- Docker inspection is exactly a fixed binary chosen from a fixed allowlist plus fixed arguments: `system df --format {{.Type}}\t{{.Size}}\t{{.Reclaimable}}`. No shell and no UI string reaches `Command`.
- The Docker VM/container measurement is the only Docker value counted in totals. Images, containers, volumes, and build cache are explicitly labelled “inside Docker” and never summed with it.
- Project discovery traverses only the already-retained compact `Node` tree. It performs no recursive filesystem walk and probes only immediate project markers for at most 200 candidates and five marker names per candidate.
- Paths under Library, application bundles, media libraries, cloud roots, hidden roots other than a candidate `.venv`, and nodes below 20 MB are not project candidates.
- Rebuild cost is visible text, not color-only: **Quick regeneration**, **Large download**, or **Manual setup**.
- Missing tools, timeouts, malformed output, ambiguous ownership, and overlapping measurements remain explicit unavailable/uncounted states.
- Opening Developer Lens is the opt-in. No Docker command or project analysis runs at startup or from the menu-bar loop.

---

## Task 1: Evidence Model, Rebuild Cost, and Non-overlap Totals

**Files:**
- Modify: `src/developer.rs`

### 1.1 Write failing model tests

Add tests that require:

1. deterministic section order: Docker, Xcode, Projects, Package stores, Build tooling, Ungrouped;
2. exact rebuild-cost mapping for current rule identifiers;
3. every finding to retain its source `Rec` id, measured display path, tier, estimate flag, description, restore explanation, and fixed command as display-only evidence;
4. findings with identical paths to be counted once;
5. nested paths to remain visible but carry `counted: false` and an overlap explanation;
6. Docker inside-VM rows to never change `report.measured_bytes`;
7. deterministic size/title/id ordering.

Introduce the intended public model with stubbed constructors so tests compile and fail at behavior:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildCost {
    QuickRegeneration,
    LargeDownload,
    ManualSetup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub source_rec_id: Option<String>,
    pub display_path: String,
    pub tier: Option<Tier>,
    pub estimated: bool,
    pub command: Option<&'static str>,
    pub explanation: String,
    pub recovery: String,
    pub overlap: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepFinding {
    pub title: String,
    pub bytes: i64,
    pub rebuild_cost: RebuildCost,
    pub counted: bool,
    pub evidence: Evidence,
}
```

Run `cargo test developer::tests`; verify failure is confined to the new stubs.

### 1.2 Implement the pure report builder

Map existing ids conservatively:

- Quick regeneration: `xcode-derived`, `go-buildcache`, measured `target`, `build`, and `dist` output.
- Large download: Docker image/build cache guidance, `node_modules`, Playwright, package stores, simulator runtimes/device support, and language dependency caches.
- Manual setup: Xcode archives, ambiguous project output, Docker volumes/containers, and anything without positive automatic-regeneration evidence.

Deduplicate by normalized measured path. When paths overlap, count only the outer measured source and attach an explicit `Included in <path>` note to the nested row. Use saturating sums and ignore non-positive byte counts.

Run focused tests and `cargo test --locked`.

Commit:

```sh
git add src/developer.rs
git commit -m "Model developer storage evidence safely" -m "Preserve source rules, recovery cost, and overlap semantics so the deep dive can explain storage without creating new cleanup authority or inflated totals."
```

---

## Task 2: Bounded Project and Xcode Inventory from Existing Scan Evidence

**Files:**
- Modify: `src/developer.rs`
- Modify: `src/scan.rs` only if a small read-only traversal helper is needed

### 2.1 Write failing bounded-discovery tests

Build synthetic `Node` trees and fixture marker directories. Tests must prove:

- `node_modules` groups beneath its existing measured project root;
- Rust `target` requires a sibling `Cargo.toml` to become an owned project output;
- `.venv`/`venv` requires `pyproject.toml`, `requirements.txt`, or `Pipfile`;
- `build`/`dist` requires a standard JS, Rust, or Python marker;
- missing markers produce a visible **Ungrouped** reveal-only finding with Manual setup cost;
- Library, app/media bundles, cloud roots, and outside-home paths are excluded;
- candidates below 20 MB are excluded;
- the marker probe is called no more than 1,000 times even with more than 200 candidate nodes;
- nested project outputs are not double counted;
- ordering remains deterministic regardless of child insertion order.

Use an injected marker predicate in tests. Production passes a predicate that calls `Path::is_file` only for fixed immediate marker names.

### 2.2 Implement one retained-tree traversal

Traverse the compact `Arc<Node>` tree once, collecting candidate directory names `target`, `.venv`, `venv`, `build`, and `dist`. Sort by bytes descending plus path ascending, truncate to 200, then run only the fixed immediate marker checks.

Add fixed Xcode measurements from already-scanned nodes where present:

- `Library/Developer/Xcode/DerivedData`
- `Library/Developer/Xcode/Archives`
- `Library/Developer/Xcode/iOS DeviceSupport`
- `Library/Developer/CoreSimulator/Profiles/Runtimes`
- `Library/Developer/CoreSimulator/Devices`

Existing `Rec` evidence wins for identical paths. The `sim-unavailable` estimate may explain its vetted cleanup command, but it must not claim the entire Devices directory is exactly reclaimable.

Run focused tests, rules tests, and the full suite.

Commit:

```sh
git add src/developer.rs src/scan.rs
git commit -m "Group bounded project and Xcode storage" -m "Reuse retained scan nodes and capped nearby marker checks to organize developer output without another filesystem traversal or executable discovered paths."
```

---

## Task 3: Fixed Read-only Docker Breakdown

**Files:**
- Modify: `src/developer.rs`

### 3.1 Write failing parser and runner-boundary tests

Add tests for:

- decimal and binary-looking Docker sizes (`0B`, `12.5MB`, `1.2GB`, `3.4kB`) converted conservatively to decimal bytes;
- Images, Containers, Local Volumes, and Build Cache mapping to stable titles and rebuild costs;
- malformed/unknown lines being omitted without corrupting valid rows;
- zero/negative/impossible sizes being omitted;
- command failure, timeout, and non-UTF-8 output yielding an unavailable explanation;
- fixed arguments being generated internally with no caller-provided executable or arguments;
- Docker detail rows carrying `counted: false` and an “inside Docker; not added” explanation.

### 3.2 Implement the fixed command worker function

Resolve `docker` only from `$PATH` and these fixed locations:

- `/usr/local/bin/docker`
- `/opt/homebrew/bin/docker`
- `/Applications/Docker.app/Contents/Resources/bin/docker`

Run it without a shell using fixed arguments and a three-second timeout. Cap captured stdout/stderr to 64 KiB. Parse the fixed tab-delimited template output into a `DockerBreakdown`. A stopped Docker engine should become a normal unavailable message, not a Developer Lens failure.

Do not reuse `clean::run_command`: that function intentionally invokes a shell for vetted cleanup commands, while Developer Deep Dive requires a narrower read-only process boundary.

Run focused tests and the full suite.

Commit:

```sh
git add src/developer.rs
git commit -m "Explain Docker usage without double counting" -m "Run one fixed read-only Docker command with a strict timeout and keep inside-VM categories outside measured filesystem totals."
```

---

## Task 4: Async Developer Workspace UI and Evidence Disclosure

**Files:**
- Modify: `src/app.rs`
- Modify: `scripts/ui-smoke.applescript`
- Modify: `scripts/test-ui-smoke.sh`

### 4.1 Write failing app-state and copy tests

Add pure tests for:

- empty, loading, partial, unavailable-Docker, and populated workspace summaries;
- rebuild-cost labels exactly matching the approved visible words;
- evidence copy distinguishing measured, estimated, inside-Docker, overlap, Safe, and Caution states;
- stale reports invalidating when a new scan begins;
- Developer Lens opening being the only automatic trigger for its worker.

Add smoke-tooling assertions before implementation:

- Developer branch requires accessible static text `DEVELOPER WORKSPACE`;
- an accessible `Refresh` button exists;
- automation never clicks `Refresh`, Evidence disclosures, Reveal in Finder, cleanup, move, or scan controls.

Run the focused tests and `scripts/test-ui-smoke.sh`; verify RED.

### 4.2 Add the opt-in worker lifecycle

Add application state for `developer_rx`, `developer_report`, and a top-level worker error. When the user opens Developer Lens, clone the compact root and current `Rec` values into a worker that builds the pure report and loads the fixed Docker detail. Poll through the existing update loop. Invalidate this state in `begin_scan`.

Do not run the worker from app boot, scan completion, menu updates, Summary rendering, or Insights rendering. A manual **Refresh** reruns the same read-only worker.

### 4.3 Replace the flat cards with the deep workspace

Render:

- persistent `DEVELOPER WORKSPACE` heading and plain read-only boundary;
- Docker footprint plus uncounted inside-Docker categories or a bounded unavailable message;
- Xcode categories;
- projects grouped by root, with Ungrouped output separate;
- rebuild-cost text on every finding;
- Safe/Caution text on every rule-backed finding;
- a collapsed **Evidence** disclosure showing source path, measured/estimated size, overlap treatment, reason, recovery, and vetted cleanup command when one exists;
- no checkbox, action selector, hold button, delete button, or path-derived command.

Use a vertical `ScrollArea` inside the existing rail bounds. Keep the Summary and Insights entry unchanged.

Run focused tests, smoke-tooling checks, format, and the full suite.

Commit:

```sh
git add src/app.rs scripts/ui-smoke.applescript scripts/test-ui-smoke.sh
git commit -m "Add the Developer Deep Dive workspace" -m "Expose Docker, Xcode, project, rebuild-cost, and source evidence through an opt-in read-only rail backed by an asynchronous bounded worker."
```

---

## Task 5: Documentation, Signed Proof, and CI

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md`

### 5.1 Document shipped boundaries

README must explain:

- Docker on-disk footprint versus uncounted inside-VM categories;
- fixed read-only command and unavailable behavior;
- Xcode category semantics;
- bounded retained-tree project grouping and marker cap;
- rebuild-cost labels and evidence disclosures;
- discovered output is reveal-only and never becomes a cleanup rule.

AGENTS must document the fixed Docker command boundary, retained-tree/marker caps, overlap policy, and rule-backed-only actions. Mark Phase 3 shipped in the v3 spec only after all signed proof succeeds.

### 5.2 Run complete local proof

```sh
cargo fmt --check
cargo test --locked
git diff --check
scripts/test-ui-smoke.sh
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
./make-app.sh
scripts/test-signed-ui.sh
```

Verify the installed app signature and root `dist/DiskDeck.zip`. Do not exercise any cleanup, move, restore, watch/unwatch, scan, or reveal action.

### 5.3 Eyes-on-screen verification

Inspect Developer Deep Dive in the signed application at minimum and typical window sizes in light and dark appearances. The real machine should cover populated Xcode/project sections plus the Docker available or unavailable state. Confirm:

- navigation and Refresh remain visible;
- cards scroll without covering navigation;
- paths and totals do not overlap utility text;
- rebuild cost, tier, estimated status, and uncounted status are visible words;
- Docker detail values are not added to the measured footprint;
- Evidence disclosures fit without horizontal clipping;
- no cleanup controls appear;
- opening other rails does not rerun developer inspection.

Restore the owner's original dark appearance.

### 5.4 Final commit and integration

After signed proof, update shipped status and commit:

```sh
git add README.md AGENTS.md docs/superpowers/specs/2026-07-12-diskdeck-v3-guidance-forecast-developer-design.md
git commit -m "Ship Developer Deep Dive" -m "Document the fixed-command, bounded-discovery, overlap, and evidence rules behind DiskDeck's opt-in developer workspace."
```

Fast-forward the verified branch to `main`, rerun tests and signed smoke from `main`, push, and watch the exact GitHub Actions run to completion. Remove the worktree and branch only after green CI.

---

## Final v3 audit

After Phase 3 is green, audit the approved specification line by line:

- Guided Reclaim remains Safe-only and revision-safe.
- Storage Forecasting still requires compatible foreground evidence and introduces no background scans.
- Developer Deep Dive introduces no discovered-path cleanup authority and no double counting.
- Summary remains understandable to everyday users.
- All three phases are documented, signed-app verified, and green in GitHub CI.

Only then mark the three-phase v3 objective complete.
