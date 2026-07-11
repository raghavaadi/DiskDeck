## Summary

Describe the user-visible change and the boundary it affects.

## Verification

List every command run and its result. Add sanitized signed-app screenshots for UI changes.

## Safety checklist

- [ ] `cargo test --locked` passes.
- [ ] `cargo fmt -- --check` passes.
- [ ] `scripts/test-pre-commit.sh` and `scripts/test-pre-push.sh` pass.
- [ ] Scan remains read-only and rooted at `/System/Volumes/Data`.
- [ ] No removal bypasses explicit selection plus the 900 ms hold.
- [ ] Caution recommendations remain unselected by default.
- [ ] Commands remain fixed vetted strings from `rules.rs`, not UI-controlled input.
- [ ] `CFBundleIdentifier` remains `com.buddyhq.headroom-rs` and the default signing identity is unchanged.
- [ ] New dependencies are justified in the description, or no dependency was added.
- [ ] Screenshots, logs, commits, and fixtures contain no credentials or private machine data.
