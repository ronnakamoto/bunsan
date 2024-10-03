#!/bin/bash

set -e

if [ $# -eq 0 ]; then
    echo "No version number provided. Usage: ./release.sh <version>"
    exit 1
fi

NEW_VERSION=$1

sed -i '' "s/^version = .*/version = \"$NEW_VERSION\"/" Cargo.toml

git add Cargo.toml
git commit -m "chore: Bump version to $NEW_VERSION"

git tag v$NEW_VERSION

git push origin master
git push origin v$NEW_VERSION

echo "Version $NEW_VERSION has been released. GitHub Actions will now build and upload the release assets."
