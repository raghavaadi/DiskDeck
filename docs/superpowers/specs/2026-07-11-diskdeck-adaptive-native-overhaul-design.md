# DiskDeck Adaptive Native Overhaul — Design Specification

**Status:** Approved for implementation on 2026-07-11

## Product and audience

DiskDeck is a native macOS storage visualizer and conservative space reclaimer for people who want to understand where their disk went before allowing anything to be removed. Its single-screen job is to turn a live disk scan into an understandable storage landscape and an auditable reclaim plan.

The redesign must feel at home on macOS without becoming an anonymous settings window. Its identity comes from the stacked-disk mark, the live storage landscape, and unusually clear safety semantics.

## Approved direction

Use **Adaptive Native**: Direction B's light, familiar macOS structure combined with Direction A's calm, deep dark appearance. The app follows the macOS system appearance automatically. Light and dark modes share exactly the same layout, hierarchy, labels, and safety meaning; only semantic color values change.

## Design principles

1. **The map is the signature.** The live treemap is the most expressive surface and the only place allowed a broad categorical palette.
2. **Native calm around the map.** Toolbars, cards, dialogs, and activity surfaces use restrained spacing, rounded geometry, and sentence-case labels.
3. **Color has a job.** Mint means safe or successfully complete, amber means review or caution, red means failure or irreversible danger, and ocean cyan means navigation, scanning, or selection.
4. **Safety stays structural.** Safe recommendations remain pre-selected, caution recommendations remain unselected, and removal still requires a checked row plus the 900 ms hold.
5. **One adaptive system.** No separate dark-mode layout and no manual app-only theme fork. DiskDeck follows macOS Light, Dark, and Auto through egui's system theme preference.

## Visual tokens

### Light appearance

| Token | Value | Use |
|---|---:|---|
| Canvas | `#EDF1F5` | window and panel gutters |
| Toolbar | `#FAFBFC` | top bar |
| Surface | `#FFFFFF` | primary cards and panels |
| Surface raised | `#F6F8FA` | nested cards, ticker, inactive controls |
| Edge | `#DCE2E8` | borders and separators |
| Ink | `#18212B` | primary text |
| Muted | `#536171` | secondary text |
| Faint | `#8792A0` | metadata and disabled controls |
| Accent | `#187DB7` | navigation, scanning, focus |
| Safe | `#14745C` | safe selections and success |
| Caution | `#9A630F` | review and caution |
| Danger | `#C93E4A` | failure and permanent erase |

### Dark appearance

| Token | Value | Use |
|---|---:|---|
| Canvas | `#10151D` | window and panel gutters |
| Toolbar | `#161C25` | top bar |
| Surface | `#171E28` | primary cards and panels |
| Surface raised | `#1D2632` | nested cards, ticker, inactive controls |
| Edge | `#293542` | borders and separators |
| Ink | `#EDF4FB` | primary text |
| Muted | `#AAB7C7` | secondary text |
| Faint | `#6F7D8E` | metadata and disabled controls |
| Accent | `#68CCE3` | navigation, scanning, focus |
| Safe | `#8EE1C9` | safe selections and success |
| Caution | `#E6B56F` | review and caution |
| Danger | `#FF6B78` | failure and permanent erase |

Alpha variants derive from these semantic colors; they must not hard-code the old amber/cyan RGB values.

## Typography

- Use the proportional UI face for body copy, controls, and sentence-case panel titles.
- Retain Saira Condensed only for the DiskDeck wordmark and compact high-level numerals where its storage-instrument character adds identity.
- Use the bundled monospace face for byte counts, elapsed time, filesystem paths, and activity timestamps.
- Stop using all-caps as the default voice. Reserve uppercase for tiny metadata labels where scanability benefits.
- Never reintroduce invisible-space letter-spacing; `spaced()` remains an identity function and the fallback font stack remains intact.

## Layout

The app remains a single window and keeps the existing right-side reclaim rail and bottom activity surface.

```text
┌──────────────────────────────────────────────────────────────────────┐
│  DiskDeck   Macintosh HD                        FDA status   Rescan   │
├─────────────────────────────────────────────┬────────────────────────┤
│  Storage used + circular usage indicator    │  Reclaimable summary   │
│  Scan status · items · footprint · access   │  Safe / caution groups │
├─────────────────────────────────────────────┤  Recommendation cards  │
│                                             │                        │
│            LIVE STORAGE LANDSCAPE           │  Selected total        │
│           breadcrumb + zoom map             │  Hold to reclaim       │
│                                             │                        │
├─────────────────────────────────────────────┴────────────────────────┤
│  Activity · latest scan / reclaim / offload events                  │
└──────────────────────────────────────────────────────────────────────┘
```

- The top toolbar is 56 px high with a drawn stacked-disk mark, a title-case wordmark, short volume context, and two clear actions.
- The main left column begins with a compact 142 px overview row: capacity on the left and four scan facts on the right.
- The live storage map receives all remaining left-column height.
- The reclaim rail remains 390 px wide so explanations and command previews stay readable.
- The activity panel remains visible but quieter at approximately 108 px high.
- At the minimum supported window size of 1180 × 740, every safety control remains usable without overlap.

## Components and interaction

### Toolbar

- Replace the gauge-like brand glyph with a small stacked-disk mark derived from the approved logo.
- Show `DiskDeck` in title case and `Macintosh HD` as muted context.
- Use `Rescan` / `Stop scan` as the primary action and `Full Disk Access` as the secondary action.
- Keep the existing Full Disk Access explainer and launch behavior.

### Storage overview

- Replace the cockpit gauge with a quiet circular usage indicator.
- Lead with `79.9 GB used` and retain total/free data as supporting copy.
- Present items mapped, footprint, no-access count, and elapsed time in a four-column fact strip.
- During scanning, the current path and progress treatment remain live but use Accent rather than animated cockpit decoration.

### Storage landscape

- Rename `Terrain map` to `Storage map` in visible UI.
- Preserve live growth, breadcrumb drill-down, right-click/Escape back navigation, Finder reveal, offload shortcut, and zoom animation.
- Use adaptive categorical fills. Dark colors are deep ocean, teal, slate, and warm sand; light colors are clearer versions of the same families.
- Hover uses Accent. Denied regions use Danger. Byte labels use Muted/Ink rather than Caution, because size itself is not a warning.

### Reclaim rail

- Rename `Reclaim plan` to `Reclaimable` and lead with the total potential space.
- Use `Safe · regenerates automatically` and `Review · may require a download` as group labels.
- Safe checkmarks and completion use Safe; caution uses Caution; erase uses Danger.
- Retain the exact action choices, command display, explanation expansion, defaults, and 900 ms hold.
- The primary footer action is mint when the selection is entirely safe and amber when any selected item is caution or destructive.

### Activity and dialogs

- Rename `Ops feed` to `Activity` and use sentence-case status labels.
- Preserve all event detail, scroll behavior, and success/failure distinctions.
- Restyle the offload dialog and completion stamp through the same palette without changing its capacity, acknowledgement, verification, or hold gates.

## Motion

- Preserve the existing treemap zoom as the single signature motion.
- Preserve progress animation only while scanning and hold-fill animation only while confirming.
- Do not add ambient glow, pulsing, or unconditional repainting.
- Continue requesting repaint only while scanning, cleaning, zooming, displaying a completion stamp, offloading, or showing an active dialog.

## Accessibility and quality

- Primary text must maintain at least 7:1 contrast against Canvas and Surface in both appearances.
- Secondary text must maintain at least 4.5:1 contrast against its surface.
- Meaning must never depend on color alone: keep checkmarks, labels, status words, and action names.
- Hit areas for custom buttons remain at least 30 px high.
- Verify at 1480 × 920 and the supported minimum 1180 × 740.
- Build and visually inspect the signed `.app` in both macOS appearances before release.

## Non-goals

- No change to scanner traversal, size accounting, cleanup rules, command execution, offload verification, bundle identifier, signing identity, or app permissions.
- No new crate dependencies.
- No new navigation model or multi-window architecture.
- No feature additions from the parked v2 list.

## Verification

1. Unit-test both palettes for contrast and semantic separation.
2. Unit-test adaptive treemap colors and retain all existing layout tests.
3. Run `cargo test` before every implementation commit.
4. Run `./make-app.sh`, confirm the unchanged bundle identifier and signing identity, and launch the installed app.
5. Visually inspect the signed app at 1480 × 920 and 1180 × 740 in both Light and Dark appearances, including type rendering, map hover, recommendation expansion, and the hold control.

