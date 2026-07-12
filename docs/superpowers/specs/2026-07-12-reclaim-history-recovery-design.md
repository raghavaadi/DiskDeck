# Reclaim History and Recovery — design

**Date:** 2026-07-12
**Status:** delivered and verified locally under the owner's standing direction

## Why this is next

DiskDeck defaults eligible cleanup items to Trash and correctly describes that
space as pending until Trash is emptied. The missing half of that safety promise
is recovery: after a successful reclaim, the app keeps only an in-memory status
line and forgets the exact Trash destination. A user can open Trash, but cannot
reliably tell which item DiskDeck moved, where it came from, or whether an exact
restore is still safe.

The next independent slice is **Reclaim History**: a bounded local receipt for
each successful cleanup plus a verified, explicit restore for Trash moves whose
exact destination is known. It serves ordinary users first while giving
developers a useful audit trail for command and permanent cleanup results.

## Approaches considered

1. **History only.** Persist completed cleanup summaries and open Trash for the
   user. This improves accountability but leaves recovery ambiguous.
2. **Durable receipts plus verified Trash restore — selected.** Persist exact
   direct-rename outcomes, derive current recoverability, and restore only after
   collision, identity, path, and acknowledgement checks.
3. **APFS snapshot rollback.** Ask macOS to roll storage state backward. This is
   not consistently available, is too broad for one cache item, and would make
   an unsafe promise to ordinary users.

## Product experience

### Entry point

Add **Reclaim History** to the Insights rail. The summary also shows a quiet
“Last reclaim” line after at least one receipt exists. No history worker, scan,
or filesystem probe runs from the menu-bar monitor.

The workspace opens with three compact facts:

- reclaimed now: bytes permanently freed by retained receipts;
- pending in Trash: bytes associated with exact Trash receipts that have not
  been restored or observed missing;
- recoverable: count of receipts that pass the current read-only preflight.

These values are receipt summaries, not a new disk scan. The UI must say so and
must not add them to the live scan totals.

### Receipt rows

Newest receipts appear first. Every row shows:

- cleanup title and completion time;
- original path;
- action: Trash, permanent erase, emptied contents, or vetted command;
- measured bytes freed or pending at completion;
- current state in plain language.

Trash states are **Ready to restore**, **Trash item missing**, **Original path
occupied**, **Changed in Trash**, **Restore unavailable**, and **Restored**.
Non-Trash actions state **Permanent — cannot restore**. Finder-fallback Trash
moves state **Open Trash to restore manually** because DiskDeck did not observe
the final Finder-selected destination.

Rows expose **Reveal in Trash** only for an existing exact Trash path and
**Open Trash** as the honest fallback. No receipt row exposes cleanup, erase, or
command controls.

### Restore confirmation

Choosing **Restore…** opens one focused confirmation surface containing the
item name, original path, Trash path, receipt age, measured size, and the exact
reason the current preflight passes or fails.

The user must tick “I understand this restores the item to its original path”
and hold **Restore from Trash** for 900 ms. Escape and Back cancel without
mutation. The button is disabled while another clean, offload, or restore
operation is active.

After success the receipt becomes **Restored**, the activity feed reports the
result, and the app asks for a rescan instead of pretending the old terrain map
is current. A failure leaves the Trash item in place whenever the atomic rename
did not occur and presents a concrete recovery message.

## Persistence model

Create a focused `reclaim_history.rs` module. Store
`~/Library/Application Support/DiskDeck/reclaim-history.ddrh` using a strict
versioned binary format named `DDRH1`.

Each successful cleanup receipt contains:

- a locally unique event id;
- completion timestamp in milliseconds;
- vetted recommendation id and display title;
- original filesystem path as raw macOS path bytes;
- action enum;
- measured freed and pending bytes, clamped to non-negative values;
- exact Trash path when the direct rename path is known;
- Trash item device, inode, and file-kind identity captured after the rename;
- optional restored timestamp.

The codec rejects unknown versions, invalid flags/actions, negative sizes,
overlong fields, excessive records, truncation, and trailing bytes. Paths round
trip as raw bytes rather than lossy UTF-8. Writes use a sibling temporary file,
`sync_all`, and atomic rename. Retain the newest 200 receipts. Retention deletes
only old receipt records; it never touches an original or Trash item. A corrupt
history is surfaced as unavailable and is never overwritten automatically.

Command stdout/stderr, shell strings, usernames, telemetry, and file contents
are never written. History remains local and is excluded from SSD offload.

## Cleanup integration

Change `clean::trash_path` to return a typed outcome:

- `Exact { path, identity }` when the preferred same-volume rename succeeds;
- `FinderManaged` when Finder fallback succeeds without an observable exact
  destination.

The cleanup worker receives the fixed history path from application state. On
every successful job it creates and appends a receipt before emitting the final
result event. Receipt persistence failure never converts a successful cleanup
into a failed cleanup; it is carried as an explicit warning in the event and
activity feed.

Command recommendations remain locked to `Action::Command`. This feature does
not accept, construct, or replay a command from receipt data.

## Restore safety boundary

Restore accepts only a retained `DDRH1` receipt with `Action::Trash` and an
exact Trash outcome. It never accepts a path from text input or a generic UI
string.

The pure preflight and the worker both enforce:

1. the recorded Trash path is a normalized direct child of the current
   `$HOME/.Trash`;
2. the Trash item exists, is not a symlink, and its device, inode, and file kind
   exactly match the post-move identity stored in the receipt;
3. the original path is absolute and normalized; it is not `/`, `/System` or
   anything below `/System`, `/Applications` or anything below
   `/Applications`, `/Library`, `/Users`, the home directory, the home
   `Library` directory, `.Trash`, or anything below `.Trash`;
4. the original path is absent;
5. the original parent exists, is a real directory, and no existing ancestor
   below the filesystem root is a symlink;
6. the Trash item and original parent are on the same device so restore can use
   one atomic `rename`;
7. the receipt has not already been marked restored.

Immediately before mutation the worker repeats the full preflight. It performs
exactly one `rename(trash_path, original_path)`, verifies the identity at the
original path, and then atomically marks the receipt restored. If receipt-state
persistence fails after the rename, the UI reports “restored, but history could
not be updated”; it does not move the item back or claim failure.

Restore never overwrites, deletes, copies, empties Trash, escalates privileges,
or invokes Finder/AppleScript. Items outside the exact direct-rename boundary
stay manual-only.

## Concurrency and UI state

History loading and recoverability classification run on a bounded background
worker when Reclaim History opens or when a cleanup/restore completes. The UI
renders loading, empty, populated, corrupt/unavailable, and restore-in-progress
states. Opening the workspace is read-only.

Only one mutating pipeline may run at a time across reclaim, SSD offload,
moved-item restore, and Trash restore. Existing scanning may continue to render,
but Trash restore invalidates the completed scan result and asks for a fresh
scan after success.

## Verification

### Pure and fixture tests

- `DDRH1` raw-path round trip, action/state round trip, size bounds, truncation,
  trailing-data, invalid-version, invalid-action, and 200-record retention;
- corrupt history refuses automatic overwrite;
- direct Trash rename returns the exact final collision-safe path and recorded
  identity; Finder fallback remains separately representable;
- recoverability classification covers ready, missing, occupied, changed,
  symlink, cross-device, protected-root, and restored states;
- fixture restore proves atomic move-back and history update;
- path or inode replacement between UI preflight and worker execution blocks
  before mutation;
- history write failure after a successful cleanup is a warning, not a false
  cleanup failure;
- command receipts cannot become command authority or restore authority.

### Signed application proof

Build only with `./make-app.sh`, verify the stable signature, then inspect
Reclaim History in light and dark appearance at minimum and typical window
sizes. The checked-in AppleScript smoke may open the rail and return with Escape
but must not click Restore, Reveal, Open Trash, cleanup, move, or scan controls.

Destructive-path proof uses fixture directories only. A live signed test may
Trash and restore one tiny fixture created specifically for the test; it must
never exercise the pipeline on the owner's real recommendations or data.

Run formatting, all shell guards, `cargo test --locked`, the distribution ZIP
validator, signed UI smoke, and the exact GitHub CI run before declaring the
slice shipped.

### Delivered proof

- `149` active Rust tests pass; the sole ignored test is the explicit signed-UI
  fixture seeder.
- The fixture suite proves exact direct-to-Trash identity capture, no-overwrite
  restoration, replacement detection, persisted restored state, corrupt-history
  refusal, and warning-only receipt persistence failures.
- The signed app was inspected in light and dark appearance at minimum and
  typical window sizes with both empty and populated history. The populated
  fixture covered ready, restored, permanent, Finder-managed, changed, and
  missing states; its confirmation dialog was inspected without activating the
  hold control.
- Signed smoke navigates Reclaim History without activating restore, reveal,
  Open Trash, scan, cleanup, move, or refresh controls. On macOS versions that
  lazily expose AccessKit until the app owns focus, the harness clicks only the
  harmless native title bar before running the same non-mutating navigation.
- QA used only the exact `DiskDeck-QA-Reclaim-History` fixture paths. They were
  removed after inspection, the prior absent history state was restored, and
  the owner's dark appearance preference was restored.

## Documentation

README explains that recovery is available only while the exact Trash item is
still present and unchanged, that emptying Trash makes it unrecoverable, and
that permanent/command cleanup cannot be undone. AGENTS documents the receipt
codec, path/identity boundary, no-overwrite rule, worker-only I/O, and fixture-
only mutation test policy.

This slice adds no account, network request, telemetry, dependency, privileged
helper, background full scan, arbitrary path input, or new cleanup authority.
