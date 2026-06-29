#!/usr/bin/env bash
#
# package.sh - Build and package Zenterm for the current platform.
#
# Usage:
#   ./scripts/package.sh               # default (release)
#   ./scripts/package.sh --debug       # debug build
#   ./scripts/package.sh --format dmg  # specific format only
#
# Requirements: cargo-packager (install via `cargo install cargo-packager`)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_DIR"

# ── Parse arguments ──────────────────────────────────────────────────
DEBUG=false
FORMATS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            DEBUG=true
            shift
            ;;
        --format)
            FORMATS=("${FORMATS[@]}" "$2")
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--debug] [--format <format>...]"
            echo ""
            echo "Options:"
            echo "  --debug           Build and package debug binaries"
            echo "  --format <fmt>    Package format(s) to produce"
            echo "                    (default: platform-appropriate formats)"
            echo "  --help, -h        Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--debug] [--format <format>...]"
            exit 1
            ;;
    esac
done

# ── Detect platform ──────────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
    Darwin)  PLATFORM="macos" ;;
    Linux)   PLATFORM="linux" ;;
    MINGW*|MSYS*|CYGWIN*)  PLATFORM="windows" ;;
    *)
        echo "Unsupported platform: $OS"
        exit 1
        ;;
esac

echo "========================================"
echo " Zenterm Packager"
echo " Platform : $PLATFORM"
echo " Profile  : $([[ "$DEBUG" == true ]] && echo "debug" || echo "release")"
echo " Formats  : ${FORMATS[*]:-(platform default)}"
echo "========================================"
echo ""

PACKAGER_ARGS=(-p zenterm)

if [[ "$DEBUG" == false ]]; then
    PACKAGER_ARGS=("${PACKAGER_ARGS[@]}" --release)
fi

if [[ ${#FORMATS[@]} -gt 0 ]]; then
    IFS=, join_fmts="${FORMATS[*]}"
    PACKAGER_ARGS=("${PACKAGER_ARGS[@]}" --formats "$join_fmts")
fi

echo "→ Running: cargo packager ${PACKAGER_ARGS[*]}"
echo ""

cargo packager "${PACKAGER_ARGS[@]}"

echo ""
echo "✔ Done! Packages are in the output directory."
