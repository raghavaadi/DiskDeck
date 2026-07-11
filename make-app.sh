#!/bin/zsh
# Build the release binary, bundle "DiskDeck.app", codesign with a stable
# identity (TCC permission grants survive rebuilds), install to /Applications,
# and produce a shareable dist zip.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

IDENTITY="${DISKDECK_SIGN_IDENTITY:-${HEADROOM_SIGN_IDENTITY:-Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)}}"
DEST="/Applications/DiskDeck.app"

# Assemble and codesign the bundle on an internal APFS volume, never inside the
# repo. If the checkout lives on an exFAT disk (e.g. an external SSD), macOS
# scatters AppleDouble `._*` files through the bundle and `codesign --deep`
# aborts with "Operation not permitted". mktemp -d lands in /var/folders (APFS).
BUILD="$(mktemp -d "${TMPDIR:-/tmp}/diskdeck.XXXXXX")"
trap 'rm -rf "$BUILD"' EXIT
APP="$BUILD/DiskDeck.app"

cargo build --release

# ── icns from assets/icon.png ──
if [ ! -f assets/DiskDeck.icns ] || [ assets/icon.png -nt assets/DiskDeck.icns ]; then
  rm -rf /tmp/diskdeck.iconset && mkdir -p /tmp/diskdeck.iconset
  for s in 16 32 64 128 256 512; do
    sips -z $s $s assets/icon.png --out /tmp/diskdeck.iconset/icon_${s}x${s}.png >/dev/null
    sips -z $((s*2)) $((s*2)) assets/icon.png --out /tmp/diskdeck.iconset/icon_${s}x${s}@2x.png >/dev/null
  done
  iconutil -c icns /tmp/diskdeck.iconset -o assets/DiskDeck.icns
fi

# ── bundle ──
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/diskdeck "$APP/Contents/MacOS/diskdeck"
cp assets/DiskDeck.icns "$APP/Contents/Resources/DiskDeck.icns"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>DiskDeck</string>
    <key>CFBundleDisplayName</key><string>DiskDeck</string>
    <key>CFBundleIdentifier</key><string>com.buddyhq.headroom-rs</string>
    <key>CFBundleExecutable</key><string>diskdeck</string>
    <key>CFBundleIconFile</key><string>DiskDeck</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>NSHumanReadableCopyright</key><string>See where your disk went. Take it back.</string>
</dict>
</plist>
EOF

codesign --force --deep --sign "$IDENTITY" "$APP"
codesign --verify --strict "$APP" && echo "✓ signed as: $IDENTITY"

FIRST_INSTALL=0
[ -d "$DEST" ] || FIRST_INSTALL=1
osascript -e 'quit app "DiskDeck"' 2>/dev/null || true
rm -rf "$DEST"
cp -R "$APP" "$DEST"
echo "✓ installed → $DEST"

# shareable bundle: app + one-time installer that handles Gatekeeper +
# the single Full Disk Access grant for recipients
mkdir -p dist
rm -rf "$BUILD/stage" && mkdir -p "$BUILD/stage/DiskDeck"
cp -R "$APP" "$BUILD/stage/DiskDeck/"
cp scripts/install.command "$BUILD/stage/DiskDeck/Install DiskDeck.command"
chmod +x "$BUILD/stage/DiskDeck/Install DiskDeck.command"
ditto -c -k --keepParent "$BUILD/stage/DiskDeck" "dist/DiskDeck.zip"
echo "✓ shareable → dist/DiskDeck.zip (app + installer)"

open "$DEST"

# permissions are keyed to bundle id + signature (both stable here), so any
# grant survives rebuilds — but the very first install needs it made once
if [ "$FIRST_INSTALL" = "1" ]; then
  echo ""
  echo "▸ FIRST INSTALL: grant Full Disk Access once (System Settings just"
  echo "  opened) — toggle ON “DiskDeck”, then RESCAN in the app."
  echo "  That single grant replaces all per-folder prompts, permanently."
  open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
fi
