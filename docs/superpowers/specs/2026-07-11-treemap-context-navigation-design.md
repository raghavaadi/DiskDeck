# Treemap Context Navigation Design

**Date:** 2026-07-11
**Status:** Approved

## Goal

Make DiskDeck's storage map understandable without teaching modifier-click
shortcuts. Primary click remains fast navigation. Secondary click exposes the
actions people expect from a macOS context menu, and a visible Back control
makes reverse navigation discoverable.

## Decision

Use direct navigation plus a context menu. A Finder-style selection model was
rejected because the treemap has no persistent inspector or other useful
selected state; it would add a click before every navigation. An always-visible
inline action bar was rejected because it would obscure small map blocks and
compete with the map-first hierarchy.

## Interaction model

- A primary click on an accessible, real directory opens that directory in the
  treemap.
- A secondary click on any painted item opens a context menu at the pointer.
- The menu orders actions by intent: **Open**, **Reveal in Finder**, then
  **Move to SSD** after a separator.
- **Open** is enabled only for accessible, non-synthetic directories.
- **Reveal in Finder** and **Move to SSD** are enabled for real items. All
  filesystem actions are visible but disabled for synthetic aggregate blocks.
- Choosing **Move to SSD** opens the existing offload dialog. The existing
  acknowledgement, free-space check, verified copy, and hold-to-confirm flow
  remain unchanged.
- The old Command-click and Option-click routes are removed. Escape may still
  navigate back as an unadvertised keyboard convenience.

## Navigation controls

- A visible **Back** button with a left arrow sits at the trailing side of the
  breadcrumb row.
- Back navigates exactly one level and is disabled at the Data root.
- Breadcrumb segments remain clickable, allowing a direct jump to any ancestor.
- The former **Up** label and all instructions that describe right-click as
  back navigation are removed.

## Copy and visual treatment

- Directory tooltip: `Click to open · Right-click for actions`.
- File tooltip: `Right-click for actions`.
- Synthetic aggregate tooltip: `Combined smaller items`.
- Denied item tooltip: `Access unavailable · Grant Full Disk Access to inspect`.
- Menu labels use Inter and the current Adaptive Native system palette. The
  menu should read like a quiet macOS utility menu, not a destructive action
  sheet.
- **Move to SSD** is neutral in the menu because it opens a separate reviewed
  flow; destructive color is reserved for actual destructive choices.

## Component boundaries and data flow

`draw_map` remains responsible for hit testing and painting. It derives a
small action-availability value from the hovered treemap item. Menu choices
produce one of three existing outcomes:

1. Open updates `crumbs`, `view`, and the zoom source rectangle.
2. Reveal passes the item path to `reveal_in_finder`.
3. Move strips the Data root and passes the path and measured size to
   `open_offload_dialog`.

No menu action constructs shell commands or changes cleanup recommendations.

## Safety and edge cases

- Right-clicking empty map space opens no item menu and performs no navigation.
- Synthetic aggregate blocks never expose filesystem actions.
- Denied directories may be revealed or moved only when represented by a real
  node; they cannot be opened in the map.
- A context-menu click must not also trigger primary-click navigation.
- Back at the root is inert.
- Scan, clean, and offload safety invariants are unchanged.

## Verification

- Unit tests cover action availability for directories, files, synthetic
  blocks, and denied items, plus one-level Back behavior at root and depth.
- Existing 900 ms hold and offload confirmation tests remain green.
- The signed app is checked visually in dark and light modes.
- Interaction QA covers primary-click open, secondary-click menu placement,
  every enabled menu action, disabled synthetic actions, Back, breadcrumb jump,
  and Escape.
