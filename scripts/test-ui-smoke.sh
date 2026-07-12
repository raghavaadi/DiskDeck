#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

for path in \
    scripts/ui-smoke.applescript \
    scripts/focus-window.swift \
    scripts/right-click.swift \
    scripts/test-signed-ui.sh
do
    [ -f "$ROOT/$path" ] || fail "missing $path"
done

TMP=$(mktemp -d "${TMPDIR:-/tmp}/diskdeck-ui-smoke.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

osacompile -o "$TMP/ui-smoke.scpt" "$ROOT/scripts/ui-smoke.applescript"
/usr/bin/swiftc -typecheck "$ROOT/scripts/right-click.swift"
/usr/bin/swiftc -typecheck "$ROOT/scripts/focus-window.swift"
sh -n "$ROOT/scripts/test-signed-ui.sh"

grep -q 'guided-reclaim-visible' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose guided-reclaim-visible"

grep -q 'commandName is "safety-guide-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose safety-guide-visible"

grep -q 'static text "Safety & Quick Start"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Safety & Quick Start heading"

grep -Fq 'static text "SCAN · EXPLORE · REVIEW · HOLD"' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the core safe workflow"

grep -Fq 'click button "Safety & Quick Start"' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Insights guide entry"

grep -q '^ui safety-guide-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open the Safety & Quick Start guide"

grep -q 'static text "DEVELOPER WORKSPACE"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Developer Deep Dive workspace"

grep -q 'commandName is "reclaim-history-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose reclaim-history-visible"

grep -q 'static text "Reclaim History"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Reclaim History heading"

grep -q 'commandName is "storage-search-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose storage-search-visible"

grep -q 'static text "Storage Search"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Storage Search heading"

grep -Fq 'Searches folders and large files retained in this completed map. Small items remain grouped.' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the honest Storage Search scope"

grep -q '^ui storage-search-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open Storage Search"

grep -q 'commandName is "external-drives-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose external-drives-visible"

grep -q 'static text "External drives"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the External drives heading"

grep -q 'static text "Read-only map"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the External drives safety boundary"

grep -q '^ui external-drives-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open External drives"

if grep -Fq 'enabled of button "Find  ⌘F"' "$ROOT/scripts/ui-smoke.applescript"
then
    fail "egui AccessKit omits AXEnabled; Storage Search smoke must poll for the opened heading"
fi

for forbidden in 'Restore' 'Reveal' 'Open Trash' 'Hold to restore'
do
    grep -Fq "$forbidden" "$ROOT/scripts/test-signed-ui.sh" || \
        fail "signed UI smoke safety contract is missing forbidden label: $forbidden"
done

grep -q 'my openSummary(appGroup)' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "guided smoke must reset a previously open detail rail"

grep -Fq 'if (exists button "Guide" of appGroup) and (exists button "Insights" of appGroup) then return' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "summary detection must use the unique Guide and Insights controls"

grep -q '^RIGHT_CLICK_ATTEMPTS=3$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must retry a lost context-menu click"

grep -q 'scripts/focus-window.swift' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must focus the native window before AccessKit checks"

if grep -Eiq 'click[^[:cntrl:]]*(Hold to reclaim|Review targets|Review this plan|Open Trash|Scan again|Scan now|Scan read-only|Stop|Refresh drives|Move to SSD|Reveal in Finder|Restore to Mac|Hold to restore|Start review scan|button "Refresh"|button "Watch"|button "Unwatch")' \
    "$ROOT/scripts/ui-smoke.applescript" "$ROOT/scripts/test-signed-ui.sh"
then
    fail "UI smoke runner must not click a cleanup or storage action"
fi

if grep -Eiq '(keystroke |key code (36|76)|set value of text field|click text field|click button "(Open map|Quick Look|Reveal)")' \
    "$ROOT/scripts/ui-smoke.applescript" "$ROOT/scripts/test-signed-ui.sh"
then
    fail "Storage Search smoke must not type, press Enter, or activate a result action"
fi

echo "UI smoke tooling checks passed"
