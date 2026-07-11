# Offload Safety and Contributor UI Smoke — design

**Date:** 2026-07-11
**Status:** approved as DiskDeck v2 safety foundation

## Problem

Move to SSD currently applies a broad `$HOME` check only after the user chooses
the action. The treemap can advertise the action for paths that will later be
refused, and the background worker trusts the UI preflight. The move engine also
needs explicit destination-collision and source-identity guards before it may
remove an original.

The signed-app interaction checks used during development are not yet available
to contributors. Repeating those checks requires undocumented Accessibility and
right-click automation knowledge.

## Goals

1. Make unsafe SSD sources visibly ineligible before the dialog opens.
2. Enforce the same policy again at the worker and pre-delete boundaries.
3. Refuse an existing destination instead of merging into it.
4. Publish non-destructive UI smoke tooling and contributor instructions.

## Offload eligibility policy

A source is eligible only when all of these conditions hold:

- It is an absolute, normalized path with no `.` or `..` components.
- It exists, is not a symlink, has no symlinked ancestor below the home
  directory, is below the current user's home directory, and is not the home
  directory itself.
- Its first home-relative component is not hidden and is not `Library`,
  `Applications`, `Public`, or `Trash`.
- It is not beneath a known top-level cloud-sync root: Dropbox, OneDrive,
  Google Drive, or iCloud Drive archives.
- No component represents an application or managed media-library bundle:
  `.app`, `.photoslibrary`, `.musiclibrary`, `.imovielibrary`, or `.fcpbundle`.
- It is not already beneath `/Volumes`.

This protected blocklist preserves ordinary and custom user folders, including
developer projects, while refusing app-managed state, credentials in hidden
roots, Trash, synchronized roots, and package-like bundles.

`src/offload.rs` exposes one structured decision type containing eligibility
and stable user-facing refusal copy. `app.rs` uses it to disable Move to SSD in
the context menu and recommendation rows, while `open_offload_dialog` retains
the same check as a fallback.

## Mutation-boundary enforcement

`OffloadJob` carries the expected home directory. The worker re-evaluates the
source policy, target mount eligibility, and free-space margin immediately
before copying.

Before `ditto`, the engine records the source device and inode from
`symlink_metadata`. It refuses to start if the destination already exists.
After copy and size verification, it reads source metadata again. If the source
became a symlink or its device/inode changed, the operation fails with the
original path untouched and the copied destination left for inspection. Only a
matching identity may proceed to `delete_path`.

Failures remain explicit:

- policy, volume, room, or collision failure: no copy and no deletion;
- copy or size verification failure: original untouched;
- source identity change: original untouched, copied destination retained;
- symlink creation failure after a verified move: move succeeds with a warning
  and ledger entry recording `symlinked: false`.

## User experience

- **Move to SSD…** is disabled for protected items rather than opening a dialog
  that can never complete.
- Hover copy gives the short refusal reason, such as “App-managed Library data
  stays on this Mac” or “Cloud-synced folders cannot be offloaded safely.”
- Eligible items retain the existing target picker, capacity display, symlink
  explanation, acknowledgement, and hold-to-confirm ceremony.
- No cleanup recommendation becomes newly selected or destructive.

## Contributor automation

Add three tracked tools:

- `scripts/ui-smoke.applescript` discovers the signed DiskDeck window through
  System Events, verifies stable controls, records the breadcrumb signature,
  exercises the named Back control when nested, and emits machine-readable
  results. It never selects a reclaim row or menu action.
- `scripts/right-click.swift` dispatches one secondary click at coordinates
  supplied by the smoke runner. It contains no filesystem operations.
- `scripts/test-signed-ui.sh` verifies `/Applications/DiskDeck.app`, checks
  Accessibility availability with an actionable error, locates a real treemap
  item through the accessibility tree, opens its context menu, sends Escape,
  and asserts that the breadcrumb signature did not change.

The runner never chooses Open, Reveal, Move to SSD, Review targets, or reclaim.
It documents the one-time Accessibility permission and exits without changing
system appearance, files, selections, or cleanup state.

## Testing

Unit tests cover every allowed and refused path category, normalized-path
rejection, bundle detection, stable refusal copy, destination collision, and
source identity mismatch. Existing verified-move and symlink tests remain
green.

Script checks use shell fixtures for missing app and missing Accessibility
permission. Live verification uses the signed app and performs navigation and
menu dismissal only. The full slice also runs formatting, all Rust tests,
community checks, privacy/identity guard tests, `make-app.sh`, bundle-id check,
and strict code-signature verification.

## Non-goals

- No restore UI in this slice; it is the next offload-specific phase.
- No automatic deletion of partial destinations.
- No network volume support.
- No moving app bundles, managed media libraries, cloud-sync roots, hidden
  roots, or app-managed Library data through an advanced override.
