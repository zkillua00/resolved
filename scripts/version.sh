#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
manifest="$project_dir/Cargo.toml"
lockfile="$project_dir/Cargo.lock"

usage() {
    cat >&2 <<'EOF'
usage: scripts/version.sh current
       scripts/version.sh next
       scripts/version.sh bump

current  Print the Cargo package version.
next     Print the next version required by commits since the latest vX.Y.Z tag.
bump     Update Cargo.toml and Cargo.lock to that next version.
EOF
    exit 2
}

current_version() {
    awk '
        /^\[package\]$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^version = "[0-9]+\.[0-9]+\.[0-9]+"$/ {
            value = $0
            sub(/^version = "/, "", value)
            sub(/"$/, "", value)
            print value
            exit
        }
    ' "$manifest"
}

is_semver() {
    printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
}

latest_release_tag() {
    tag="$(git -C "$project_dir" describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
    if [ -n "$tag" ] && is_semver "${tag#v}"; then
        printf '%s\n' "$tag"
    fi
}

next_version() {
    current="$(current_version)"
    if ! is_semver "$current"; then
        echo "error: Cargo package version must be MAJOR.MINOR.PATCH" >&2
        exit 1
    fi

    release_tag="$(latest_release_tag)"
    if [ -n "$release_tag" ]; then
        base_version="${release_tag#v}"
        commit_range="$release_tag..HEAD"
    else
        base_version="$current"
        commit_range="HEAD"
    fi

    history_file="$(mktemp "${TMPDIR:-/tmp}/api-tester-version.XXXXXX")"
    trap 'rm -f "$history_file"' EXIT HUP INT TERM
    git -C "$project_dir" log "$commit_range" --format='%s%n%b' >"$history_file"

    bump="none"
    if grep -Eq '^[[:alnum:]_-]+(\([^)]*\))?!:|^BREAKING[ -]CHANGE:' "$history_file"; then
        bump="breaking"
    elif grep -Eq '^feat(\([^)]*\))?:' "$history_file"; then
        bump="minor"
    elif grep -Eq '^(fix|perf|revert)(\([^)]*\))?:' "$history_file"; then
        bump="patch"
    fi

    old_ifs="$IFS"
    IFS=.
    set -- $base_version
    IFS="$old_ifs"
    major="$1"
    minor="$2"
    patch="$3"

    case "$bump" in
        breaking)
            if [ "$major" -eq 0 ]; then
                minor=$((minor + 1))
                patch=0
            else
                major=$((major + 1))
                minor=0
                patch=0
            fi
            ;;
        minor)
            minor=$((minor + 1))
            patch=0
            ;;
        patch)
            patch=$((patch + 1))
            ;;
        none)
            ;;
    esac

    next="$major.$minor.$patch"
    if [ "$current" != "$base_version" ] && [ "$current" != "$next" ]; then
        echo "error: Cargo version $current matches neither $release_tag nor the calculated next version $next" >&2
        exit 1
    fi
    printf '%s\n' "$next"
}

replace_manifest_version() {
    version="$1"
    output="$(mktemp "${TMPDIR:-/tmp}/api-tester-manifest.XXXXXX")"
    awk -v version="$version" '
        /^\[package\]$/ { in_package = 1 }
        in_package && !updated && /^version = "/ {
            print "version = \"" version "\""
            updated = 1
            next
        }
        { print }
        END { if (!updated) exit 1 }
    ' "$manifest" >"$output"
    chmod 644 "$output"
    mv "$output" "$manifest"
}

replace_lockfile_version() {
    version="$1"
    output="$(mktemp "${TMPDIR:-/tmp}/api-tester-lockfile.XXXXXX")"
    awk -v version="$version" '
        /^\[\[package\]\]$/ { is_project = 0 }
        /^name = "api-tester"$/ { is_project = 1 }
        is_project && !updated && /^version = "/ {
            print "version = \"" version "\""
            updated = 1
            next
        }
        { print }
        END { if (!updated) exit 1 }
    ' "$lockfile" >"$output"
    chmod 644 "$output"
    mv "$output" "$lockfile"
}

command="${1:-}"
case "$command" in
    current)
        current_version
        ;;
    next)
        next_version
        ;;
    bump)
        current="$(current_version)"
        next="$(next_version)"
        if [ "$current" = "$next" ]; then
            printf '%s (already current)\n' "$current"
            exit 0
        fi
        replace_manifest_version "$next"
        replace_lockfile_version "$next"
        printf '%s -> %s\n' "$current" "$next"
        ;;
    *)
        usage
        ;;
esac
