#!/usr/bin/env bash
#
# package.sh - Build and package Zenterm for a target platform.
#
# Usage:
#   ./scripts/package.sh
#   ./scripts/package.sh --target aarch64-apple-darwin
#   ./scripts/package.sh --format dmg
#
# Requirements: cargo-packager (install via `cargo install cargo-packager`)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_DIR"

DEBUG=false
FORMATS=()
TARGET=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            DEBUG=true
            shift
            ;;
        --format)
            [[ $# -ge 2 ]] || { echo "--format requires a value" >&2; exit 1; }
            FORMATS+=("$2")
            shift 2
            ;;
        --target)
            [[ $# -ge 2 ]] || { echo "--target requires a value" >&2; exit 1; }
            TARGET="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--debug] [--target <triple>] [--format <format>...]"
            echo ""
            echo "Options:"
            echo "  --debug            Build and package debug binaries"
            echo "  --target <triple>  Build for the specified Rust target"
            echo "  --format <format>  Package format(s) to produce"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

if [[ -z "$TARGET" ]]; then
    TARGET="$(rustc -vV | awk '/^host:/ { print $2 }')"
fi

case "$TARGET" in
    *-windows-*) PLATFORM="windows" ;;
    *-apple-darwin) PLATFORM="macos" ;;
    *-unknown-linux-*) PLATFORM="linux" ;;
    *)
        echo "Unsupported target: $TARGET" >&2
        exit 1
        ;;
esac

PROFILE="release"
if [[ "$DEBUG" == true ]]; then
    PROFILE="debug"
fi

echo "========================================"
echo " Zenterm Packager"
echo " Platform : $PLATFORM"
echo " Target   : $TARGET"
echo " Profile  : $PROFILE"
echo " Formats  : ${FORMATS[*]:-(platform default)}"
echo "========================================"
echo ""

BUILD_ARGS=(--package zenterm --locked --target "$TARGET")
PACKAGER_ARGS=(
    --packages zenterm
    --target "$TARGET"
    --out-dir dist
    --binaries-dir "target/$TARGET/$PROFILE"
)

if [[ "$DEBUG" == false ]]; then
    BUILD_ARGS+=(--release)
    PACKAGER_ARGS+=(--release)
fi

if [[ ${#FORMATS[@]} -gt 0 ]]; then
    IFS=, join_fmts="${FORMATS[*]}"
    PACKAGER_ARGS+=(--formats "$join_fmts")
fi

echo "→ Running: cargo build ${BUILD_ARGS[*]}"
cargo build "${BUILD_ARGS[@]}"

echo "→ Running: cargo packager ${PACKAGER_ARGS[*]}"
cargo packager "${PACKAGER_ARGS[@]}"

echo ""
echo "✔ Done! Packages are in dist/."
