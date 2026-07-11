# AGENTS.md — maintainer instructions for DiskDeck

You are maintaining **DiskDeck**, a pure-Rust (egui 0.29 / eframe) macOS
disk-space app. The owner is Raghav (aadithyaraghav@gmail.com). Read this whole file before
changing anything; most lines exist because something broke.

## Non-negotiable invariants

1. **Safety model.** The clean orchestrator (`clean::run_clean`) only ever
   executes the vetted command string stored on a `Rec` in `rules.rs` —
   command recs are locked to `Action::Command` regardless of what the UI
   requests. Never add a path or command that flows from UI input into a
   destructive operation.
2. **Nothing is deleted without explicit user action.** Scan is read-only.
   Every removal requires a ticked checkbox + the 900 ms hold. Keep it so.
3. **`CFBundleIdentifier` is `com.buddyhq.headroom-rs` and must NEVER
   change** (in `make-app.sh`'s Info.plist heredoc). macOS TCC keys Full Disk
   Access to bundle id + signing identity — changing either silently resets
   permissions for every user. Same rule for the signing identity default.
4. **Scan root is `/System/Volumes/Data`** (`scan::DATA_ROOT`), not `/` —
   scanning `/` double-counts through APFS firmlinks. Don't "fix" this.
5. **Sizes are on-disk usage** (`metadata.blocks() * 512`), sparse-aware, so
   `Docker.raw` reports honestly. Hardlinks dedup by inode; other volumes are
   never crossed (`dev` check).
6. **Tier policy:** `Safe` = regenerates fully automatically (pre-checked);
   `Caution` = costs a re-download/re-install (NEVER pre-checked — selection
   default keys off tier in `App::on_scan_finished`). User documents/code/
   media are never recommended.
7. **SSD offload policy is structural.** `offload::classify_movable` owns the
   protected-path rules; `check_movable` adds real metadata/symlink checks,
   and `run_offload` repeats them at the worker boundary. `perform_move`
   refuses an existing destination and rechecks source device/inode before
   deletion. Never bypass these layers from UI code or add an override for
   Library, hidden, cloud-sync, app-bundle, or managed-library paths.
8. **History records completed truth only.** `history::record_scan` starts only
   after `ScanState::Done`, when `compact()` has finished. Never persist live
   or aborted trees, never move snapshot I/O onto the UI thread, and never
   upload history. Retention may delete only matching `snapshot-*.ddhist`
   files inside DiskDeck's Application Support History directory.

## Build & ship

```sh
cargo test       # all unit tests must pass before any commit
./make-app.sh    # THE ship path: build + bundle + codesign + install + dist zip
cargo run        # dev run only — unbundled binary has its own TCC identity,
                 # so FDA grants made for the .app won't apply to it
```

- **Never ship bare `cargo build` output.** Unsigned binaries get a fresh
  TCC identity per build → macOS re-asks all folder permissions. That exact
  complaint ("it keeps asking permission") is why `make-app.sh` signs with a
  stable cert and why `dist/DiskDeck.zip` bundles
  `scripts/install.command` — a recipient-facing installer that copies to
  /Applications, clears quarantine, opens the FDA pane, and launches.
- `assets/logo.svg` is the single canonical transparent brand mark.
  `scripts/render-icon.cjs` synchronizes the three SVG layers in
  `assets/AppIcon.icon`, derives the universal blue fallback at
  `assets/icon.svg`, and renders `assets/icon.png`. `make-app.sh` regenerates
  `assets/DiskDeck.icns` when the PNG is newer, and when Xcode 26 `actool` is
  available it also compiles Default, Dark, and Mono/Tinted renditions into
  `Assets.car`. Never remove the `.icns` fallback or change the macOS 12.0
  deployment target while adding adaptive icon behavior.
- cargo lives at `~/.cargo/bin` — not on the default PATH of this machine's
  non-interactive shells.

## Architecture (one screen)

| File | Role | Watch out |
|---|---|---|
| `scan.rs` | rayon parallel walker; `Arc<Node>` tree; **atomic size bubbling up the ancestor chain so the UI renders the tree WHILE it grows** (the headline feature) | post-scan `compact()` folds dirs <10 MB into parent aggregates; files <100 MB are never materialized as nodes |
| `rules.rs` | safety KB → `Vec<Rec>`; port of the proven Go doctrine | overlap control: caches with dedicated rules are in the `skip` list so generic `~/Library/Caches` enumeration doesn't double-report; recs carry data-volume paths — fs ops go through `strip_data_root` |
| `clean.rs` | trash/delete/empty/command executors + background orchestrator (mpsc events) | **trash = `os::rename` into `~/.Trash` FIRST** — Finder-osascript hangs silently without the Automation TCC grant and is fallback only; `delete_path` chmods-and-retries for write-protected trees (go-modcache style) |
| `history.rs` | compact completed-scan snapshots, binary codec, previous-scan comparison, atomic retention worker | raw path bytes must round-trip; corrupt snapshots are skipped; unrelated Application Support files are never pruned |
| `transfer.rs` | shared path-identity, collision, apparent-size, and verified-ditto primitives | copy helpers never remove either side; callers own the final identity recheck and deletion order |
| `offload.rs` | protected-path policy, external-volume checks, verified copy/move, ledger, worker events | UI eligibility is advisory; the worker must repeat full policy/target/capacity checks, and only a matching source identity may reach `delete_path` |
| `moves.rs` | lossless local move registry, drive reconciliation, restore preflight and worker | restore is copy → verify → atomic install → target identity recheck → external delete; any occupied or changed path blocks before mutation |
| `treemap.rs` | squarified layout + paint + zoom-from animation | caps at 40 rects + synthetic "smaller items" aggregate |
| `app.rs` | egui panels, gauge, telemetry, rec cards, hold-button, ops feed | `request_repaint_after(40ms)` only while scanning/cleaning/animating — don't repaint unconditionally |
| `theme.rs` | colors, fonts, `spaced()` | see the tofu gotcha below |

## Hard-won gotchas (do not relearn)

- **egui font tofu:** `spaced()` once inserted U+200A hair-spaces for
  letter-spacing — the former Saira Condensed face had no glyph for it, and a
  custom `FontFamily` had no fallback fonts, so every label rendered `?`
  boxes. `spaced()` is now identity. Inter owns the regular/medium/semibold UI
  roles, and every family appends egui's proportional fallback stack. Never
  use invisible-space tricks without checking glyph coverage; **always verify
  type on screen** after font work.
- **Icon:** the gauge deliberately has NO unfilled track arc — at dock size
  a dark track remainder reads as a "backwards-L" artifact (user-reported).
  Ticks define the dial. If you regenerate the icon, render the SVG via
  headless Chromium with `omitBackground: true` (qlmanage paints an opaque
  white background) and check 256px + 96px downscales for artifacts.
- **Playwright caches `file://` pages hard** — re-rendering a tweaked local
  SVG returned stale pixels repeatedly. Inline markup via `setContent`.
- **Live-tree memory:** during a scan every directory gets a node (~hundreds
  of MB transient on big volumes); `compact()` slims it afterwards. Don't
  "optimize" by pruning during the scan — a small dir may still grow, and
  live pruning would make the growing map lie.
- A fresh scan of this machine: ~2.1 M items / ~140 GB in 20 s–5 min
  (cold vs warm fs cache). Denied ≈185 with FDA granted, ≈360+ without —
  both normal (see README FAQ before "fixing" the denied count).
- egui 0.29 API notes that mattered: `Rounding` (not `CornerRadius`),
  `allocate_new_ui(UiBuilder::new().max_rect(..))`, `FontData::from_static`,
  `Shape::line(points, stroke)` for arcs, `is_pointer_button_down_on()` +
  `stable_dt` for the hold button.

## The most common task: adding a cleanup rule

1. In `rules.rs`: fixed-path + command rules go in the `cmd_rules` table;
   trash/delete-able dirs go in the `simple` table; anything dynamic follows
   the generic-caches or node_modules blocks.
2. Be conservative with tiers (invariant 6). Set `estimate: true` when the
   rec's byte count is an upper bound. Write commands to be readable — the
   UI shows them verbatim in the expander.
3. If the path overlaps `~/Library/Caches`, add its dir name to `skip`.
4. **Extend `fake_tree()` + assertions in `rules.rs` tests** (every path
   segment needs its own node — `lookup` walks segment by segment), then
   `cargo test`.

## Verification protocol

- `cargo test` for logic. For UI work, build + launch via `make-app.sh` and
  verify visually (computer-use screenshot) — the tofu incident shipped
  precisely because the first release skipped eyes-on-screen.
- **Destructive-path testing:** only ever exercise the clean pipeline on the
  smallest safe-tier item with the `trash` action (recoverable), or on
  fixture dirs you created. Never test `delete`/`command` on the owner's
  real data. Restore tests likewise use fixture roots only; signed UI smoke may
  open Moved Items but must never click or hold Restore to Mac.

## Public repository and commit hygiene

- This repository publishes to the owner's personal GitHub identity:
  `aadithyaraghav@gmail.com`. Never use a work or Bitbucket identity here.
- Never commit credentials, private keys, `.env` files, signed app bundles,
  build output, AppleDouble `._*` files, internal planning notes, or literal
  machine-specific `/Users/...` and `/Volumes/...` paths.
- Enable the tracked guard once per clone with
  `git config core.hooksPath .githooks`.
- The pre-commit hook rejects sensitive content and generated artifacts. The
  pre-push hook additionally rejects any BuddyHQ-authored commit history from
  the personal `raghavaadi/DiskDeck` GitHub remote, preventing an accidental
  `git push --all` from publishing local archive branches.
- Run `scripts/test-pre-commit.sh` and `scripts/test-pre-push.sh` after changing
  either guard. Both must pass before publishing; never bypass them with
  `--no-verify`.

## Repo conventions

- Flat module layout (`scan/rules/clean/history/transfer/offload/moves/treemap/theme/app`), one concern per
  file. No new crate dependencies without strong reason.
- The approved aesthetic is **Adaptive Native**: a crisp, familiar macOS light
  appearance and a calm Storage Observatory dark appearance. The live storage
  map is the signature surface; surrounding chrome stays quiet. Color is
  semantic: mint=safe, amber=review, red=danger, cyan=navigation/scanning.
- The committed v2 roadmap is safety foundation → regrowth history → moved-item
  restore → optional Developer Lens → APFS accounting → app leftovers →
  menu-bar monitor. Deliver and verify one independent slice at a time.
- Commit style: imperative subject, body explains the why. `cargo test`
  before every commit.
