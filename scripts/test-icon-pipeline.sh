#!/bin/zsh
set -euo pipefail

cd "$(dirname "$0")/.."

ICON_DIR="assets/AppIcon.icon"
ICON_JSON="$ICON_DIR/icon.json"
ICTOOL="${ICTOOL:-/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool}"

fail() {
  echo "icon-pipeline: $*" >&2
  exit 1
}

[ -f "$ICON_JSON" ] || fail "missing $ICON_JSON"
[ -x "$ICTOOL" ] || fail "Icon Composer ictool is unavailable at $ICTOOL"

layers=("$ICON_DIR"/Assets/*.svg(N))
[ "${#layers[@]}" -eq 3 ] || fail "expected exactly 3 SVG layers, found ${#layers[@]}"
xmllint --noout assets/logo.svg "${layers[@]}"

jq -e '."supported-platforms".squares | index("macOS") != null' "$ICON_JSON" >/dev/null \
  || fail "icon.json does not support macOS squares"
jq -e '."fill-specializations"[] | select(.appearance == "dark")' "$ICON_JSON" >/dev/null \
  || fail "icon.json has no dark background specialization"
jq -e '[.groups[].layers[] | ."fill-specializations"[]? | select(.appearance == "dark")] | length >= 3' "$ICON_JSON" >/dev/null \
  || fail "all three layers need dark appearance fills"
jq -e '[.groups[].layers[] | ."fill-specializations"[]? | select(.appearance == "tinted")] | length >= 3' "$ICON_JSON" >/dev/null \
  || fail "all three layers need tinted/mono fills"

rm -rf /tmp/diskdeck-icon-renditions
mkdir -p /tmp/diskdeck-icon-renditions

for rendition in Default Dark; do
  "$ICTOOL" "$ICON_DIR" --export-image \
    --output-file "/tmp/diskdeck-icon-renditions/${rendition}.png" \
    --platform macOS --rendition "$rendition" \
    --width 1024 --height 1024 --scale 1
done

"$ICTOOL" "$ICON_DIR" --export-image \
  --output-file /tmp/diskdeck-icon-renditions/TintedDark.png \
  --platform macOS --rendition TintedDark \
  --width 1024 --height 1024 --scale 1 \
  --tint-color 0.58 --tint-strength 0.7

for preview in /tmp/diskdeck-icon-renditions/*.png; do
  [ -s "$preview" ] || fail "empty rendition: $preview"
done

rg -q -- '--app-icon AppIcon' make-app.sh \
  || fail "make-app.sh does not compile the AppIcon document"
rg -q 'Assets\.car' make-app.sh \
  || fail "make-app.sh does not bundle Assets.car"
rg -q 'CFBundleIconName.*AppIcon' make-app.sh \
  || fail "Info.plist does not select the adaptive AppIcon"
rg -q 'CFBundleIconFile.*DiskDeck' make-app.sh \
  || fail "Info.plist does not retain the DiskDeck.icns fallback"

echo "icon-pipeline: Icon Composer source and renditions passed"
