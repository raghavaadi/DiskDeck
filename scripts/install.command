#!/bin/zsh
# ─────────────────────────────────────────────────────────────────────────
#  DiskDeck — one-time installer
#  Run me ONCE (right-click → Open the first time) and macOS never nags
#  you again: no per-folder permission prompts, no repeat Gatekeeper
#  dialogs on this app.
# ─────────────────────────────────────────────────────────────────────────
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
APP="$DIR/DiskDeck.app"
DEST="/Applications/DiskDeck.app"

if [ ! -d "$APP" ]; then
  echo "✗ Couldn't find 'DiskDeck.app' next to this installer."
  echo "  Keep the app and this installer together (as they were in the zip)."
  exit 1
fi

echo "▸ Installing DiskDeck → /Applications…"
osascript -e 'quit app "DiskDeck"' 2>/dev/null || true
rm -rf "$DEST"
cp -R "$APP" "$DEST"

echo "▸ Clearing the download-quarantine flag (skips the 'unidentified developer' dialog)…"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true

echo ""
echo "▸ ONE permission to grant — then macOS never asks about folders again:"
echo "  System Settings just opened on Privacy & Security → Full Disk Access."
echo "  Toggle ON “DiskDeck” (if it isn't listed, click + and pick it from"
echo "  /Applications). This single grant replaces every Desktop/Documents/"
echo "  Downloads prompt, permanently."
open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"

echo ""
echo "▸ Launching DiskDeck…"
open "$DEST"
echo ""
echo "✓ Done. After flipping the Full Disk Access toggle, hit RESCAN in the app."
echo "  (A residual NO ACCESS count of ~185 is normal — those are root-only"
echo "   macOS system dirs no app can read. Hover the counter for details.)"
