#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
APP=/Applications/DiskDeck.app
APP_EXECUTABLE="$APP/Contents/MacOS/diskdeck"
APPLESCRIPT="$ROOT/scripts/ui-smoke.applescript"
# Safety contract consumed by test-ui-smoke.sh. Navigation may never activate
# any control containing these mutation/Finder labels.
FORBIDDEN_ACTION_LABELS='Restore Reveal Open Trash Hold to restore Refresh drives Scan read-only Stop Choose a folder Open Full Disk Access'

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ -d "$APP" ] || fail "signed app not installed at $APP; run ./make-app.sh"
[ -x "$APP_EXECUTABLE" ] || fail "DiskDeck executable is missing"

ui() {
    osascript "$APPLESCRIPT" "$@"
}

/usr/bin/swift "$ROOT/scripts/focus-window.swift" DiskDeck
sleep 0.5
ui check
ui safety-guide-visible
ui escape
ui storage-search-visible
ui escape
ui largest-files-visible
ui escape
ui scan-coverage-visible
ui escape
ui guided-reclaim-visible
ui escape
before=$(ui signature)
coordinates=$(ui tile-center)
set -- $coordinates
[ "$#" -eq 2 ] || fail "invalid tile coordinates: $coordinates"

RIGHT_CLICK_ATTEMPTS=3
menu_visible=false
click_attempt=0
while [ "$click_attempt" -lt "$RIGHT_CLICK_ATTEMPTS" ]; do
    /usr/bin/swift "$ROOT/scripts/right-click.swift" "$1" "$2"
    poll_attempt=0
    while [ "$poll_attempt" -lt 20 ]; do
        if [ "$(ui menu-visible)" = "true" ]; then
            menu_visible=true
            break
        fi
        poll_attempt=$((poll_attempt + 1))
        sleep 0.1
    done
    if [ "$menu_visible" = true ]; then
        break
    fi
    ui escape >/dev/null
    click_attempt=$((click_attempt + 1))
    sleep 0.2
done
[ "$menu_visible" = true ] || fail "context menu did not expose its fixed labels"

ui escape
sleep 0.3
after=$(ui signature)
[ "$before" = "$after" ] || fail "Escape changed breadcrumb: $before -> $after"

ui back
ui reclaim-history-visible
ui escape
ui external-drives-visible
ui escape
ui folder-lens-visible
ui escape
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
ui file-review-visible
ui escape
echo "signed UI smoke check passed"
