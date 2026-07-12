# Contributing to DiskDeck

Thanks for helping make disk cleanup safer and easier to understand. DiskDeck is a native macOS application written in Rust with egui/eframe.

## Before you start

- Use an Apple Silicon Mac running macOS 12 or later.
- Install a current stable Rust toolchain with `rustfmt`.
- Read [AGENTS.md](AGENTS.md) before changing scanner, cleanup, offload, bundle, signing, or UI behavior. Its invariants are part of the product's safety model.
- Open an issue before a large feature or a new crate dependency so the scope can be agreed first.

Enable the repository guards once per clone:

```sh
git config core.hooksPath .githooks
scripts/test-pre-commit.sh
scripts/test-pre-push.sh
```

## Development loop

```sh
cargo test --locked
cargo fmt -- --check
cargo run
```

`cargo run` is for development only. It has a different macOS TCC identity from the signed application, so Full Disk Access granted to `/Applications/DiskDeck.app` does not apply to the development binary.

For UI work, use the actual ship path and inspect the signed application in both macOS appearances:

```sh
./make-app.sh
```

### Signed UI smoke check

DiskDeck includes the non-destructive Accessibility automation used by its
maintainers. First validate that the tracked AppleScript, Swift helper, and
shell runner compile:

```sh
scripts/test-ui-smoke.sh
```

For the live check, build and install the signed app with `./make-app.sh`, then
grant Accessibility to the terminal or coding app launching the command in
**System Settings → Privacy & Security → Accessibility**. Run:

```sh
scripts/test-signed-ui.sh
```

The live runner discovers the signed window, opens and dismisses a treemap
context menu, verifies Escape does not change the breadcrumb, and may navigate
one level with the named Back button. It never selects Open, Reveal in Finder,
Move to SSD, Review targets, a recommendation, or the reclaim control.

Do not share or commit the generated app, ZIP, `target`, `dist`, or AppleDouble `._*` files.

## Maintainer releases

The normal `./make-app.sh` output is for signed local QA. It may use an Apple
Development identity so Full Disk Access survives local rebuilds, but it is
not a public distribution artifact.

Public releases run only from clean, synchronized `main` after exact-commit CI
passes. They require a Keychain-resident `Developer ID Application` identity
selected through `DISKDECK_SIGN_IDENTITY` and a notary credential stored under
the `DISKDECK_NOTARY_PROFILE` Keychain profile name. Never put certificate
exports, passwords, API keys, or app-specific passwords in this repository or
GitHub Actions.

```sh
scripts/release.sh v1.0.0            # non-mutating preflight
scripts/release.sh v1.0.0 --publish  # notarize, draft, verify, publish
```

The publisher never substitutes an unsigned, ad-hoc, or Apple Development
build. If the required distribution identity or notary profile is missing,
the release remains unpublished.

## Safety expectations

- Scanning stays read-only.
- Nothing is removed without an explicitly selected recommendation and the 900 ms hold.
- Command recommendations execute only the fixed vetted command stored in `rules.rs`.
- Safe recommendations may be preselected; caution recommendations never are.
- User documents, source code, photos, and media are never recommended for cleanup.
- The scan root stays `/System/Volumes/Data`, and the bundle identifier stays `com.buddyhq.headroom-rs`.

Never test permanent deletion or command cleanup against real user data. Use fixture directories, or at most the smallest recoverable safe-tier Trash action described in `AGENTS.md`.

## Pull requests

Keep each pull request focused. Include:

- what user-visible or safety boundary changed;
- the exact test commands and results;
- tests for new logic or regressions;
- signed-app screenshots for UI changes, sanitized so no private desktop or path data is visible;
- an explanation for any new dependency.

Use an imperative commit subject. The commit body should explain why the change is needed when the reason is not obvious from the diff.
