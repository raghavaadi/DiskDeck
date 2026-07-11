#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
APP=/Applications/DiskDeck.app
APP_EXECUTABLE="$APP/Contents/MacOS/diskdeck"
APPLESCRIPT="$ROOT/scripts/ui-smoke.applescript"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -d "$APP" ] || fail "signed app not installed at $APP; run ./make-app.sh"
[ -x "$APP_EXECUTABLE" ] || fail "DiskDeck executable is missing"

ui() {
    osascript "$APPLESCRIPT" "$@"
}

ui check
before=$(ui signature)
coordinates=$(ui tile-center)
set -- $coordinates
[ "$#" -eq 2 ] || fail "invalid tile coordinates: $coordinates"

/usr/bin/swift "$ROOT/scripts/right-click.swift" "$1" "$2"

menu_visible=false
attempt=0
while [ "$attempt" -lt 20 ]; do
    if [ "$(ui menu-visible)" = "true" ]; then
        menu_visible=true
        break
    fi
    attempt=$((attempt + 1))
    sleep 0.1
done
[ "$menu_visible" = true ] || fail "context menu did not expose its fixed labels"

ui escape
sleep 0.3
after=$(ui signature)
[ "$before" = "$after" ] || fail "Escape changed breadcrumb: $before -> $after"

ui back
ui moved-items-visible
ui escape
ui growth-watch-visible
ui escape
ui developer-lens-visible
ui escape
ui apfs-accounting-visible
ui escape
ui app-leftovers-visible
ui escape
ui menu-monitor-visible
ui escape
echo "signed UI smoke check passed"
