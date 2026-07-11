# DiskDeck Open-Source Launch Hygiene — Design Specification

**Status:** Approved under the owner's AFK autonomy instruction on 2026-07-11

## Goal

Make the public `raghavaadi/DiskDeck` repository safe and welcoming for its first outside contributors without introducing release credentials, changing product behavior, or publishing an unverified binary release.

## Current state

- The repository is public and `main` has a clean root containing only the personal GitHub identity.
- Pre-open-source commits with a BuddyHQ identity exist only on local archive branches and are not available from GitHub.
- The repository has no CI workflow, contribution guide, security policy, issue templates, pull-request template, topics, description, tag, or Release.
- GitHub reports 28% community-profile completeness.
- The signed local ship path works, but public release distribution and notarization have not been proven on a second Mac.

## Chosen approach

Adopt **minimal launch hygiene**:

1. Keep the tested pre-commit and pre-push privacy boundary.
2. Add one macOS CI workflow for formatting, repository guard tests, and locked Rust tests.
3. Add concise contributor and security guidance grounded in DiskDeck's real safety invariants.
4. Add structured bug, feature, and pull-request templates.
5. Set a factual repository description and topics after CI is green.

## CI design

- Trigger on pushes to `main` and all pull requests.
- Use a current GitHub-hosted macOS runner because DiskDeck includes macOS-only code paths and its supported platform is macOS.
- Grant only `contents: read` permission.
- Cancel an older in-progress run for the same branch or pull request.
- Install the stable Rust toolchain with `rustfmt` using `rustup`; do not add a third-party Rust action or dependency cache.
- Run, in order:
  1. `cargo fmt -- --check`
  2. `scripts/test-pre-commit.sh`
  3. `scripts/test-pre-push.sh`
  4. `cargo test --locked`
- Do not run `make-app.sh` in CI: it requires the owner's stable signing identity and installs to `/Applications`.
- Do not upload unsigned binaries or store signing credentials in GitHub Actions.

## Community files

### `CONTRIBUTING.md`

- Explain macOS and Rust prerequisites, hook setup, test commands, and the signed-vs-dev TCC distinction.
- Link maintainers to `AGENTS.md` for the full invariant set.
- Require focused changes, tests for logic, signed visual proof for UI work, and no destructive testing on real data.
- Explain the imperative commit style and that new dependencies need strong justification.

### `SECURITY.md`

- Support the latest `main` state and latest published Release once one exists.
- Direct vulnerability reports to GitHub's private vulnerability-reporting flow, not public issues.
- Treat unsafe deletion paths, command injection, symlink/path traversal, permission identity changes, and secret exposure as security issues.
- Promise no response-time SLA from a single-maintainer project.

### Issue templates

- Bug reports collect macOS version, DiskDeck build source, installation method, Full Disk Access state, reproducible steps, expected/actual behavior, and sanitized Activity output.
- Feature requests collect the user problem, proposed workflow, safety impact, and alternatives.
- Blank issues remain enabled for questions that do not fit either form.
- Security contact links to GitHub's private advisory page.

### Pull-request template

- Require summary and verification evidence.
- Include safety checkboxes for destructive paths, bundle identity, scan root, tier defaults, 900 ms hold, dependency additions, UI screenshots, and privacy hooks.

## Repository metadata

After the first CI run passes:

- Description: `A native macOS disk-space visualizer and safe reclaimer, built in pure Rust.`
- Topics: `macos`, `rust`, `disk-space`, `storage`, `egui`, `utility`, `open-source`.
- Keep Issues and Wiki enabled.
- Do not enable branch protection until the owner chooses a pull-request-only workflow.
- Do not create a tag or GitHub Release in this slice.

## Safety and privacy

- No work email, credentials, private paths, screenshots of the owner's desktop, signed app bundles, or build artifacts enter Git history.
- The local archive branches remain local; the pre-push hook blocks their BuddyHQ-authored history from the personal GitHub remote.
- Community examples use placeholders or fixture data only.

## Verification

1. Validate all YAML and shell syntax locally.
2. Run both hook test suites and `cargo test --locked` before commit.
3. Push to `main` and wait for the GitHub Actions run to finish.
4. Inspect failed logs if CI is not green; do not set repository metadata until CI passes.
5. Re-query GitHub community health, repository metadata, remote refs, tags, and Releases.
6. Confirm local and remote `main` SHAs match and the worktree is clean.

## Deferred decisions

- Developer ID Application signing and Apple notarization.
- A binary GitHub Release and `v1.0.0` tag.
- Branch protection or rulesets.
- Homebrew distribution.
- Contributor Covenant adoption and enforcement contact.

