# DiskDeck Open-Source Launch Hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add trustworthy macOS CI, contributor/security guidance, structured GitHub templates, and factual public metadata without publishing binaries or introducing credentials.

**Architecture:** Keep repository policy as versioned text and YAML under the standard GitHub community paths. Add one shell regression suite that validates every required file, security-sensitive CI constraint, and YAML parse boundary locally; GitHub Actions then runs that suite alongside Rust tests on macOS.

**Tech Stack:** GitHub Actions, POSIX shell, Ruby's bundled YAML parser, Rust/cargo, GitHub CLI/API.

## Global Constraints

- CI runs on macOS and grants only `contents: read`.
- CI never invokes `make-app.sh`, uploads binaries, or receives signing credentials.
- The default signing identity and `CFBundleIdentifier` remain unchanged.
- No work email, credentials, private paths, desktop screenshots, app bundles, or build artifacts enter Git history.
- Tags, Releases, branch protection, notarization, and Homebrew distribution remain untouched.
- No new Rust crate dependency.

---

### Task 1: Community-file contract and macOS CI

**Files:**
- Create: `scripts/test-community-files.sh`
- Create: `.github/workflows/ci.yml`
- Create: `CONTRIBUTING.md`
- Create: `SECURITY.md`
- Create: `.github/ISSUE_TEMPLATE/bug_report.yml`
- Create: `.github/ISSUE_TEMPLATE/feature_request.yml`
- Create: `.github/ISSUE_TEMPLATE/config.yml`
- Create: `.github/pull_request_template.md`

**Interfaces:**
- Consumes: existing `cargo test`, `cargo fmt`, `scripts/test-pre-commit.sh`, `scripts/test-pre-push.sh`, and GitHub's macOS runner.
- Produces: `scripts/test-community-files.sh`, a zero-network local validation command for every launch-hygiene file.

- [ ] **Step 1: Write the failing community-file test**

Create a POSIX shell script that fails unless all eight files above exist. It must parse every `.yml` file with Ruby YAML and assert that `ci.yml` contains `permissions:`, `contents: read`, `macos-14`, `cargo fmt -- --check`, both hook test scripts, and `cargo test --locked`. It must fail if the workflow contains `make-app.sh`, `upload-artifact`, `codesign`, or a signing-identity variable.

The script must also assert that `SECURITY.md` links to `https://github.com/raghavaadi/DiskDeck/security/advisories/new`, the bug form asks for Full Disk Access state, and the pull-request template mentions the 900 ms hold and bundle identifier.

- [ ] **Step 2: Run the test and verify RED**

Run: `scripts/test-community-files.sh`

Expected: FAIL because `.github/workflows/ci.yml` is missing.

- [ ] **Step 3: Implement the CI workflow**

Use `actions/checkout` v6.0.2 pinned to `de0fac2e4500dabe0009e67214ff5f5447ce83dd`. Configure push-to-main and pull-request triggers, `permissions: contents: read`, per-ref concurrency cancellation, `runs-on: macos-14`, `timeout-minutes: 20`, stable Rust plus rustfmt via rustup, then run formatting, both hook suites, and locked tests in that order.

- [ ] **Step 4: Implement community guidance and forms**

Write `CONTRIBUTING.md` and `SECURITY.md` from the approved spec. Use GitHub issue-form YAML with required dropdowns/textareas for reproducible diagnostics and safety impact. Keep blank issues enabled and link security reports to GitHub's private advisory page. Add the safety-focused pull-request checklist.

- [ ] **Step 5: Verify GREEN locally**

Run: `sh -n scripts/test-community-files.sh && scripts/test-community-files.sh && scripts/test-pre-commit.sh && scripts/test-pre-push.sh && cargo fmt -- --check && cargo test --locked`

Expected: all community, privacy, formatting, and 33 Rust tests pass.

- [ ] **Step 6: Commit CI and community files**

```bash
git add .github CONTRIBUTING.md SECURITY.md scripts/test-community-files.sh
git commit -m "Add open-source contribution checks"
```

### Task 2: Publish and verify GitHub CI

**Files:**
- No new tracked files expected.
- Verify: GitHub Actions workflow `CI` on `main`.

**Interfaces:**
- Consumes: the workflow committed in Task 1 and authenticated `gh` access.
- Produces: a completed GitHub Actions run whose conclusion is `success`.

- [ ] **Step 1: Push `main` through the identity guard**

Run: `git push origin main`

Expected: pre-push reports that personal GitHub history passed and GitHub accepts the commit.

- [ ] **Step 2: Resolve the workflow run for the pushed SHA**

Run: `gh run list --repo raghavaadi/DiskDeck --workflow CI --commit "$(git rev-parse HEAD)" --json databaseId,status,conclusion,url,headSha --limit 1`

Expected: one run exists for the exact local SHA.

- [ ] **Step 3: Wait for CI**

Run: `gh run watch <run-id> --repo raghavaadi/DiskDeck --exit-status`

Expected: the run completes with conclusion `success`. If it fails, inspect `gh run view <run-id> --log-failed`, fix only the evidenced boundary, rerun local gates, commit, and push again.

### Task 3: Safe repository metadata and final audit

**Files:**
- No tracked file changes expected.
- Verify: GitHub repository metadata, community profile, refs, tags, Releases, and local worktree.

**Interfaces:**
- Consumes: successful CI from Task 2 and GitHub admin access.
- Produces: factual description/topics and a final evidence-backed launch-readiness report.

- [ ] **Step 1: Set description and topics**

Run:

```bash
gh repo edit raghavaadi/DiskDeck \
  --description "A native macOS disk-space visualizer and safe reclaimer, built in pure Rust." \
  --add-topic macos --add-topic rust --add-topic disk-space \
  --add-topic storage --add-topic egui --add-topic utility --add-topic open-source
```

Expected: command exits successfully without changing Issues, Wiki, visibility, branch protection, tags, or Releases.

- [ ] **Step 2: Enable private vulnerability reporting**

Run: `gh api --method PUT repos/raghavaadi/DiskDeck/private-vulnerability-reporting`

Expected: HTTP success; `SECURITY.md`'s private-report link is usable.

- [ ] **Step 3: Re-query the public surface**

Run: `gh repo view raghavaadi/DiskDeck --json description,repositoryTopics,latestRelease,licenseInfo,url` and `gh api repos/raghavaadi/DiskDeck/community/profile`.

Expected: the exact description and seven topics are present; no Release exists; community health recognizes contribution, security, issue, PR, license, and README files.

- [ ] **Step 4: Verify public history and local cleanliness**

Run:

```bash
git ls-remote --heads --tags origin
git log main --format='%ae%n%ce' | sort -u
git status --short --branch
```

Expected: only remote `main` exists, no tags exist, public history contains no BuddyHQ identity, and local `main` is clean and aligned with `origin/main`.

