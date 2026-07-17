#!/usr/bin/env bash
set -euo pipefail

VERSION=$(node -p "require('./npm/package.json').version")
TAG="v${VERSION}"
if git rev-parse "$TAG" >/dev/null 2>&1 || git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
  echo "$TAG already exists"
  exit 0
fi
git tag "$TAG"
git push origin "$TAG"
