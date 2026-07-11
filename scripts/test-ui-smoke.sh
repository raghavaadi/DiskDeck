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

if grep -Eiq 'click[^[:cntrl:]]*(Hold to reclaim|Review targets|Move to SSD|Reveal in Finder)' \
    "$ROOT/scripts/ui-smoke.applescript" "$ROOT/scripts/test-signed-ui.sh"
then
    fail "UI smoke runner must not click a cleanup or storage action"
fi

echo "UI smoke tooling checks passed"
