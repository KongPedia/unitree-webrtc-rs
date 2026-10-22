#!/bin/bash

# Update project versions and refresh existing locks without upgrading dependencies.
# Usage: ./scripts/sync-version.sh 0.2.0
set -euo pipefail

if [[ $# -ne 1 || ! ${1:-} =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "Error: Supply one semantic version (e.g., 0.2.0)" >&2
    exit 1
fi
NEW_VERSION="$1"

backup=$(mktemp -d)
cp Cargo.toml pyproject.toml Cargo.lock uv.lock "$backup/" || {
    rm -rf "$backup"
    exit 1
}

cleanup() {
    status=$?
    trap - EXIT
    if [[ $status -ne 0 ]]; then
        # A failed lock refresh must not leave a partially bumped release.
        cp "$backup/Cargo.toml" "$backup/pyproject.toml" "$backup/Cargo.lock" "$backup/uv.lock" .
    fi
    rm -rf "$backup"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

update_version() {
    awk -v section="$2" -v version="$NEW_VERSION" '
        /^\[/ { in_section = ($0 == "[" section "]") }
        in_section && /^[[:space:]]*version[[:space:]]*=/ {
            $0 = "version = \"" version "\""
            count++
        }
        { print }
        END { if (count != 1) exit 1 }
    ' "$1" > "$backup/updated"
    cat "$backup/updated" > "$1"
}

echo "Syncing version to $NEW_VERSION..."
update_version Cargo.toml package
update_version pyproject.toml project
uv lock
cargo update --workspace

echo "Version synchronized to $NEW_VERSION in manifests and lockfiles."
echo "Review the four-file diff, then use make deploy to build for release."
