#!/usr/bin/env bash
set -euo pipefail

pnpm changeset version
VERSION=$(node -p "require('./npm/package.json').version")

sed -i.bak -E "s/^version = \"[^\"]+\"/version = \"${VERSION}\"/" Cargo.toml
sed -i.bak -E "s/(drag = \{ version = \")[^\"]+/\1${VERSION}/" crates/drag-cli/Cargo.toml
rm -f Cargo.toml.bak crates/drag-cli/Cargo.toml.bak

cargo generate-lockfile
git add package.json npm Cargo.toml crates/drag-cli/Cargo.toml Cargo.lock CHANGELOG.md .changeset
