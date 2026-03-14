#!/bin/bash

# Version synchronization script for unitree-webrtc-rs
# Usage: ./scripts/sync-version.sh 0.2.0

set -e

NEW_VERSION="$1"

if [ -z "$NEW_VERSION" ]; then
    echo "❌ Error: Version argument required"
    echo "Usage: $0 0.2.0"
    exit 1
fi

# Validate version format (semver)
if ! echo "$NEW_VERSION" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' > /dev/null; then
    echo "❌ Error: Invalid version format. Use semantic versioning (e.g., 0.2.0)"
    exit 1
fi

echo "🔄 Syncing version to $NEW_VERSION..."

# Update Cargo.toml
sed -i.bak "s/^version = .*/version = \"$NEW_VERSION\"/" Cargo.toml
rm Cargo.toml.bak

# Update pyproject.toml  
sed -i.bak "s/^version = .*/version = \"$NEW_VERSION\"/" pyproject.toml
rm pyproject.toml.bak

echo "✅ Version synchronized:"
echo "   Cargo.toml:    $(grep '^version = ' Cargo.toml)"
echo "   pyproject.toml: $(grep '^version = ' pyproject.toml)"

echo ""
echo "🎯 Next steps:"
echo "   make deploy    # Build for release"
echo "   git add ."
echo "   git commit -m \"v$NEW_VERSION\""
echo "   git tag v$NEW_VERSION"
echo "   git push origin v$NEW_VERSION"
