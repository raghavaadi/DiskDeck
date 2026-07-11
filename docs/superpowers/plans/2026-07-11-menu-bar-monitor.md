# Menu-Bar Monitor Implementation Plan

**Goal:** Provide an optional native menu-bar free-space readout and local
low-space warning without a background full scan or privileged helper.

- Default off; create/remove one AppKit `NSStatusItem` only on explicit user
  action and update it from `statfs` at most every five minutes.
- Persist a versioned bounded local setting and threshold; corrupt settings
  fail closed to disabled and are never overwritten automatically.
- Keep launch at login a separate explicit setting implemented through a
  user-owned LaunchAgent pointing only to `/Applications/DiskDeck.app`.
- Never enable launch at login merely because the menu readout is enabled.
- Add pure settings/threshold tests, fixed-path launch-agent tests, an Insights
  rail, docs, privacy gates, signed build, and visual/interaction proof.
