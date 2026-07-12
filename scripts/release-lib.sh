#!/bin/sh

# Pure release-policy helpers. This file is sourced by build/release scripts;
# keep it POSIX and free of side effects.

diskdeck_validate_tag() {
    printf '%s\n' "${1-}" | LC_ALL=C grep -Eq \
        '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
}

diskdeck_tag_version() {
    diskdeck_validate_tag "${1-}" || return 1
    printf '%s\n' "${1#v}"
}

diskdeck_package_version() {
    root=${1:?repository root required}
    awk '
        $0 == "[package]" { in_package = 1; next }
        in_package && /^\[/ { exit }
        in_package && $1 == "version" && $2 == "=" {
            version = $3
            sub(/^"/, "", version)
            sub(/"$/, "", version)
            print version
            found = 1
            exit
        }
        END { if (!found) exit 1 }
    ' "$root/Cargo.toml"
}

diskdeck_is_distribution_identity() {
    case ${1-} in
        'Developer ID Application: '?*) return 0 ;;
        *) return 1 ;;
    esac
}

diskdeck_extract_release_notes() {
    changelog=${1:?changelog path required}
    tag=${2:?release tag required}
    output=${3:?output path required}
    version=$(diskdeck_tag_version "$tag") || return 1
    temporary="$output.tmp.$$"

    if awk -v version="$version" '
        $0 == "## [" version "]" || index($0, "## [" version "] - ") == 1 {
            found = 1
            next
        }
        found && index($0, "## [") == 1 { exit }
        found {
            print
            if ($0 !~ /^[[:space:]]*$/) body = 1
        }
        END { if (!found || !body) exit 1 }
    ' "$changelog" > "$temporary"; then
        mv "$temporary" "$output"
        return 0
    fi

    rm -f "$temporary"
    return 1
}
