# DiskDeck v2 Roadmap — design

**Date:** 2026-07-11
**Status:** approved for autonomous phased delivery

## Product direction

DiskDeck serves both ordinary Mac users and developers. The default experience
stays calm and general-purpose; developer-specific detail appears only in an
optional **Developer Lens**. Every feature remains local-first: no account,
cloud service, telemetry, AI advisor, or always-on privileged helper is added.

The roadmap is delivered as independent, reversible slices. A slice must pass
the Rust suite, repository privacy gates, signed-app build, and relevant live UI
checks before the next slice starts.

## Delivery order

### 0. Safety foundation — shipped

Harden Move to SSD before expanding the feature set. A single eligibility
policy disables unsafe actions in the UI and is re-enforced inside the worker.
The move engine also refuses destination collisions and detects source changes
before deleting the original. Publish non-destructive AppleScript-based UI
smoke tooling so contributors can repeat the signed-app checks.

### 1. Regrowth history foundation — shipped

Persist compact local scan snapshots and compare the newest completed scan with
the previous compatible scan. The summary answers “what grew?” with absolute
and percentage deltas. History is stored only on the Mac, written atomically,
bounded by retention, and never uploaded.

### 2. Moved items and restore — shipped

Turn the existing offload ledger into a **Moved items** view. It reports whether
the target volume is attached, whether the origin symlink is healthy, and offers
a verified move-back flow. Restore uses the same copy, verify, identity, space,
and hold-to-confirm safety boundaries as offload.

### 3. Growth Watch — shipped

Turn the previous-scan comparison into a local timeline and watchlist. Users can
see recurring growers, inspect absolute and percentage change across retained
snapshots, and choose folders to watch without running an always-on full scan.

### 4. Developer Lens — shipped

Add an opt-in view that explains Docker, Xcode, simulators, node_modules,
package caches, and build artifacts without changing the default summary.
Developer Lens uses deterministic rules and local measurements; it never sends
paths or project names elsewhere.

### 5. APFS accounting — shipped

Separate ordinary file usage from snapshots and purgeable capacity when macOS
provides reliable local data. Values that cannot be measured exactly remain
visibly approximate. DiskDeck must not claim that purgeable or snapshot space
is immediately reclaimable.

### 6. App leftovers — shipped

Identify large support directories whose owning application is no longer
installed. Findings are always Caution, never pre-selected, and show the
evidence used to associate a directory with an absent bundle identifier.

### 7. Menu-bar monitor — shipped

Add an optional, low-frequency free-space readout and local low-space warning.
It does not run a full scan in the background and does not become a privileged
daemon. Users explicitly enable and disable launch-at-login behavior.

### 8. Duplicate and large-old-file review — shipped

Add opt-in scans for exact duplicate files and large files that have not been
used recently. Results are never pre-selected, preserve at least one copy of a
duplicate group, provide Quick Look and Finder reveal, and remain separate from
the deterministic cache recommendations.

## Shared product rules

- The scan remains read-only and rooted at `/System/Volumes/Data`.
- Cleanup still requires an explicit selection and the 900 ms hold.
- Offload and restore use copy, verify, then remove; no original is removed
  after a failed copy or verification.
- Default UI stays approachable; advanced developer and filesystem detail is
  disclosed progressively.
- Persistent data lives under the standard DiskDeck Application Support
  directory and is excluded from SSD offload.
- No new crate dependency is introduced unless a slice cannot be implemented
  safely with the standard library and existing dependencies.

## Program verification

Each slice includes pure unit tests, failure-path tests at the actual mutation
boundary, repository privacy checks, `cargo test --locked`, `make-app.sh`, code
signature verification, and signed-app visual or interaction proof appropriate
to the changed surface.
