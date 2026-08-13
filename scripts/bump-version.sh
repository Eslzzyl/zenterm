#!/usr/bin/env bash
# Bump the workspace version, update Cargo.lock, and create a release tag.
#
# Usage: ./scripts/bump-version.sh 0.2.0
#
# Requirements: cargo-edit (for `cargo set-version`).

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <new-version>"
  echo "  e.g. $0 0.2.0"
  exit 1
fi

NEW_VERSION="$1"

if [[ ! "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]]; then
  echo "Error: version must be in semver format (e.g. 0.2.0, 0.2.0-beta.1)" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

if [[ -n "$(git status --short)" ]]; then
  echo "Error: working tree is not clean; commit or stash changes first." >&2
  exit 1
fi

command -v cargo-set-version >/dev/null 2>&1 || {
  echo "Error: cargo-edit is required. Install it with: cargo install cargo-edit --locked" >&2
  exit 1
}

echo "==> Bumping workspace version to $NEW_VERSION ..."
cargo set-version --workspace "$NEW_VERSION"

echo ""
echo "==> Regenerating Cargo.lock ..."
cargo generate-lockfile

echo ""
echo "==> Verifying workspace metadata ..."
cargo metadata --no-deps --format-version 1 >/dev/null

echo ""
echo "==> Staging version changes ..."
git add -- Cargo.toml crates/*/Cargo.toml Cargo.lock

echo ""
echo "==> Creating commit and tag ..."
git commit -m "chore: bump version to $NEW_VERSION"
git tag -a "v$NEW_VERSION" -m "Zenterm v$NEW_VERSION"

echo ""
echo "============================================"
echo "  Version bumped to $NEW_VERSION"
echo "  Commit : $(git rev-parse HEAD)"
echo "  Tag    : v$NEW_VERSION"
echo "============================================"
echo ""
echo "Next step — push to remote:"
echo "  git push origin master --follow-tags"
