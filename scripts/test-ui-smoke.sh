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

grep -q 'commandName is "largest-files-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose largest-files-visible"

grep -q 'static text "Largest files"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Largest files heading"

grep -Fq 'static text "RETAINED MAP · FILES ≥ 100 MB"' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Largest files coverage boundary"

grep -Fq 'Largest files answers the biggest-file question from the completed map without another scan.' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "Safety guide smoke must explain the Largest files workflow"

grep -q '^ui largest-files-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open Largest files"

grep -q 'commandName is "scan-coverage-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose scan-coverage-visible"

grep -q 'static text "Scan coverage"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Scan coverage heading"

grep -Fq 'static text "LOCAL COVERAGE · COMPLETED MAP"' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the completed-map coverage boundary"

grep -Fq 'static text "Local only · paths are not saved · no reveal, clean, or move actions"' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Scan coverage privacy boundary"

grep -Fq 'Scan coverage explains what the completed map could not read without starting another scan.' \
    "$ROOT/scripts/ui-smoke.applescript" || \
    fail "Safety guide smoke must explain the Scan coverage workflow"

grep -q '^ui scan-coverage-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open Scan coverage"

grep -q 'commandName is "external-drives-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose external-drives-visible"

grep -q 'static text "External drives"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the External drives heading"

grep -q 'static text "Read-only map"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the External drives safety boundary"

grep -q '^ui external-drives-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open External drives"

grep -q 'commandName is "folder-lens-visible"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must expose folder-lens-visible"

grep -Fq 'button "Choose a folder…"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify the Folder Lens chooser"

grep -Fq 'static text "Drop one Finder folder here"' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify drag-and-drop discoverability"

grep -Fq 'It cannot reclaim or move anything.' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "UI smoke runner must verify Folder Lens capability copy"

grep -q '^ui folder-lens-visible$' "$ROOT/scripts/test-signed-ui.sh" || \
    fail "signed UI smoke must open Folder Lens"

if grep -Fq 'enabled of button "Find  ⌘F"' "$ROOT/scripts/ui-smoke.applescript"
then
    fail "egui AccessKit omits AXEnabled; Storage Search smoke must poll for the opened heading"
fi

for forbidden in 'Restore' 'Reveal' 'Open Trash' 'Hold to restore' 'Open Full Disk Access'
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

awk '
    /else if commandName is "escape"/ { in_escape = 1 }
    in_escape && /repeat 3 times/ { retried = 1 }
    in_escape && /delay 0.3/ { settled = 1 }
    in_escape && /return "PASS: Escape sent"/ { exit (settled && retried) ? 0 : 1 }
    END { if (!settled || !retried) exit 1 }
' "$ROOT/scripts/ui-smoke.applescript" || \
    fail "Escape smoke must retry until egui publishes rail navigation"

if grep -Eiq 'click[^[:cntrl:]]*(Hold to reclaim|Review targets|Review this plan|Open Trash|Open Full Disk Access|Scan again|Scan now|Scan read-only|Stop|Refresh drives|Choose a folder|Move to SSD|Reveal in Finder|Restore to Mac|Hold to restore|Start review scan|button "Refresh"|button "Watch"|button "Unwatch")' \
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
