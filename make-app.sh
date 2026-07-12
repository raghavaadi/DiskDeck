#!/bin/zsh
# Build the release binary and bundle DiskDeck.app. Default mode signs and
# installs a stable local-QA build so TCC grants survive rebuilds. Explicit
# distribution mode additionally requires Developer ID signing, hardened
# runtime, secure timestamp, notarization, stapling, and Gatekeeper proof.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
. scripts/release-lib.sh

IDENTITY="${DISKDECK_SIGN_IDENTITY:-${HEADROOM_SIGN_IDENTITY:-Apple Development: aadithyaraghav@gmail.com (ZFZMAP2UJ3)}}"
VERSION="$(diskdeck_package_version "$PWD")"
BUILD_NUMBER="${DISKDECK_BUILD_NUMBER:-1}"
DISTRIBUTION="${DISKDECK_DISTRIBUTION:-0}"
NO_OPEN="${DISKDECK_NO_OPEN:-0}"
NOTARY_PROFILE="${DISKDECK_NOTARY_PROFILE:-DiskDeck-Notary}"
DEST="/Applications/DiskDeck.app"

diskdeck_validate_tag "v$VERSION" || {
  echo "✗ Cargo package version is not canonical SemVer: $VERSION" >&2
  exit 1
}
printf '%s\n' "$BUILD_NUMBER" | LC_ALL=C grep -Eq '^[1-9][0-9]*$' || {
  echo "✗ DISKDECK_BUILD_NUMBER must be a positive integer" >&2
  exit 1
}
case "$DISTRIBUTION" in
  0|1) ;;
  *) echo "✗ DISKDECK_DISTRIBUTION must be 0 or 1" >&2; exit 1 ;;
esac
case "$NO_OPEN" in
  0|1) ;;
  *) echo "✗ DISKDECK_NO_OPEN must be 0 or 1" >&2; exit 1 ;;
esac

if [ "$DISTRIBUTION" = "1" ]; then
  diskdeck_is_distribution_identity "$IDENTITY" || {
    echo "✗ public releases require Developer ID Application signing" >&2
    exit 1
  }
  AVAILABLE_IDENTITIES="$(security find-identity -v -p codesigning)"
  printf '%s\n' "$AVAILABLE_IDENTITIES" | grep -Fq "\"$IDENTITY\"" || {
    echo "✗ Developer ID signing identity is not available in the keychain" >&2
    exit 1
  }
fi

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

# macOS 26 adaptive icon: compile the Icon Composer document into Assets.car
# when Xcode 26 tooling is available. The classic DiskDeck.icns remains the
# macOS 12–15 and no-Xcode fallback selected by CFBundleIconFile below.
ADAPTIVE_ICON=0
ACTOOL="${ACTOOL:-$(xcrun --find actool 2>/dev/null || true)}"
if [ -n "$ACTOOL" ] && [ -x "$ACTOOL" ] && [ -d assets/AppIcon.icon ]; then
  if "$ACTOOL" assets/AppIcon.icon \
      --compile "$APP/Contents/Resources" \
      --platform macosx \
      --minimum-deployment-target 12.0 \
      --target-device mac \
      --app-icon AppIcon \
      --output-partial-info-plist "$BUILD/adaptive-icon.plist" \
      --output-format human-readable-text \
      --warnings --notices; then
    [ -f "$APP/Contents/Resources/Assets.car" ] || {
      echo "✗ actool did not produce Assets.car" >&2
      exit 1
    }
    ADAPTIVE_ICON=1
    echo "✓ adaptive icon → Assets.car (Default / Dark / Mono)"
  else
    echo "⚠ adaptive icon compile failed; using DiskDeck.icns fallback" >&2
  fi
else
  echo "▸ Icon Composer tooling unavailable; using DiskDeck.icns fallback"
fi

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
    <key>CFBundleShortVersionString</key><string>0.0.0</string>
    <key>CFBundleVersion</key><string>0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>NSHumanReadableCopyright</key><string>See where your disk went. Take it back.</string>
</dict>
</plist>
EOF

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Contents/Info.plist"

if [ "$ADAPTIVE_ICON" = "1" ]; then
  /usr/libexec/PlistBuddy -c 'Add :CFBundleIconName string AppIcon' "$APP/Contents/Info.plist"
fi

if [ "$DISTRIBUTION" = "1" ]; then
  codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
  SIGNATURE_DETAILS="$BUILD/signature-details.txt"
  codesign -dv --verbose=4 "$APP" 2> "$SIGNATURE_DETAILS"
  grep -Fq 'TeamIdentifier=65KMSM8WL8' "$SIGNATURE_DETAILS" || {
    echo "✗ public signing identity is not DiskDeck team 65KMSM8WL8" >&2
    exit 1
  }
else
  codesign --force --deep --sign "$IDENTITY" "$APP"
fi
codesign --verify --deep --strict "$APP" && echo "✓ signed as: $IDENTITY"

if [ "$DISTRIBUTION" = "1" ]; then
  NOTARY_ARCHIVE="$BUILD/notary-upload.zip"
  NOTARY_RESULT="$BUILD/notary-result.plist"
  COPYFILE_DISABLE=1 ditto --norsrc --noextattr -c -k --keepParent \
    "$APP" "$NOTARY_ARCHIVE"
  if ! xcrun notarytool submit "$NOTARY_ARCHIVE" \
    --keychain-profile "$NOTARY_PROFILE" \
    --wait --timeout 1h --output-format plist > "$NOTARY_RESULT"; then
    echo "✗ Apple notarization submission failed" >&2
    exit 1
  fi
  NOTARY_STATUS=$(/usr/libexec/PlistBuddy -c 'Print :status' "$NOTARY_RESULT" 2>/dev/null || true)
  if [ "$NOTARY_STATUS" != "Accepted" ]; then
    echo "✗ Apple notarization status was ${NOTARY_STATUS:-unavailable}, not Accepted" >&2
    exit 1
  fi
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  codesign --verify --deep --strict "$APP"
  echo "✓ notarized and stapled"
fi

FIRST_INSTALL=0
if [ "$DISTRIBUTION" = "0" ]; then
  [ -d "$DEST" ] || FIRST_INSTALL=1
  osascript -e 'quit app "DiskDeck"' 2>/dev/null || true
  rm -rf "$DEST"
  cp -R "$APP" "$DEST"
  echo "✓ installed → $DEST"
fi

# shareable bundle: app + one-time installer that handles Gatekeeper +
# the single Full Disk Access grant for recipients
mkdir -p dist
rm -rf "$BUILD/stage" && mkdir -p "$BUILD/stage/DiskDeck"
cp -R "$APP" "$BUILD/stage/DiskDeck/"
cp scripts/install.command "$BUILD/stage/DiskDeck/Install DiskDeck.command"
chmod +x "$BUILD/stage/DiskDeck/Install DiskDeck.command"
rm -f "dist/DiskDeck.zip"
# The repository may live on an exFAT volume, where Finder-compatible copies
# leave AppleDouble `._*` sidecars. They are not application content and can
# confuse Gatekeeper, so omit resource forks/xattrs and validate the exact ZIP.
COPYFILE_DISABLE=1 ditto --norsrc --noextattr -c -k --keepParent \
  "$BUILD/stage/DiskDeck" "dist/DiskDeck.zip"
scripts/check-dist.sh "dist/DiskDeck.zip"
if [ "$DISTRIBUTION" = "1" ]; then
  scripts/check-release-artifact.sh "dist/DiskDeck.zip" "$VERSION"
  echo "✓ public artifact → dist/DiskDeck.zip (notarized app + installer)"
else
  echo "✓ local QA artifact → dist/DiskDeck.zip (not for public release)"
fi

if [ "$DISTRIBUTION" = "0" ] && [ "$NO_OPEN" = "0" ]; then
  open "$DEST"
fi

# permissions are keyed to bundle id + signature (both stable here), so any
# grant survives rebuilds — but the very first install needs it made once
if [ "$DISTRIBUTION" = "0" ] && [ "$FIRST_INSTALL" = "1" ]; then
  echo ""
  echo "▸ FIRST INSTALL: grant Full Disk Access once (System Settings just"
  echo "  opened) — toggle ON “DiskDeck”, then RESCAN in the app."
  echo "  That single grant replaces all per-folder prompts, permanently."
  if [ "$NO_OPEN" = "0" ]; then
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
  fi
fi
