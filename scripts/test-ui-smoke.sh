#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

for path in \
    scripts/ui-smoke.applescript \
    scripts/right-click.swift \
    scripts/test-signed-ui.sh
do
    [ -f "$ROOT/$path" ] || fail "missing $path"
done

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-ui-smoke.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

osacompile -o "$TMP/ui-smoke.scpt" "$ROOT/scripts/ui-smoke.applescript"
/usr/bin/swiftc -typecheck "$ROOT/scripts/right-click.swift"
sh -n "$ROOT/scripts/test-signed-ui.sh"

grep -q 'guided-reclaim-visible' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose guided-reclaim-visible"

grep -q '^RIGHT_CLICK_ATTEMPTS=3$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must retry a lost context-menu click"

if grep -Eiq 'click[^[:cntrl:]]*(Hold to reclaim|Review targets|Review this plan|Open Trash|Scan again|Scan now|Move to SSD|Reveal in Finder|Restore to Mac|Hold to restore|Start review scan|button "Watch"|button "Unwatch")' \
    "$ROOT/scripts/ui-smoke.applescript" "$ROOT/scripts/test-signed-ui.sh"
then
    fail "UI smoke runner must not click a cleanup or storage action"
fi

echo "UI smoke tooling checks passed"
