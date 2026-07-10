#!/usr/bin/env python3
"""
zenterm-test — Terminal Emulator Rendering Test Suite

Tests SGR attributes, colours (16/256/truecolor), cursor operations,
Unicode/CJK/emoji rendering, programming ligatures, OSC sequences,
screen modes, graphics protocols, and rendering performance.

Usage:
  python3 zenterm-test.py                    # Interactive mode (all tests)
  python3 zenterm-test.py --quick            # Quick automated tests
  python3 zenterm-test.py --auto             # Skip visual checks (CI)
  python3 zenterm-test.py --include sgr,colors  # Run specific categories
  python3 zenterm-test.py --json report.json    # Save JSON report
  python3 zenterm-test.py --list-tests          # List available tests
"""

import argparse
import sys
import os
import time
from typing import Optional

# Add parent to path for the module
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from zenterm_test.terminal import Terminal, get_terminal
from zenterm_test.capability import detect_capabilities, format_capabilities
from zenterm_test.runner import TestRunner
from zenterm_test.tests.all import register_all
from zenterm_test.reporter import Report, tests_by_category
from zenterm_test.selector import interactive_select


_term: Optional["Terminal"] = None


def parse_args(argv=None):
    parser = argparse.ArgumentParser(
        description="Terminal Emulator Rendering Test Suite",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )

    parser.add_argument(
        "--list-tests",
        action="store_true",
        help="List all available test cases and exit",
    )

    parser.add_argument(
        "--include", type=str, default="", help="Comma-separated categories to include"
    )
    parser.add_argument(
        "--exclude", type=str, default="", help="Comma-separated categories to exclude"
    )

    parser.add_argument(
        "--quick", action="store_true", help="Run only quick, auto-verifiable tests"
    )
    parser.add_argument(
        "--auto", action="store_true", help="Skip interactive/visual checks (for CI)"
    )
    parser.add_argument(
        "--json", type=str, default="", help="Write JSON report to file"
    )
    parser.add_argument("--verbose", action="store_true", help="Detailed log output")
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="Suppress terminal output (for CI, use with --json)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=2000,
        help="DSR/query timeout in milliseconds (default: 2000)",
    )

    return parser.parse_args(argv)


def main():
    global _term
    args = parse_args()

    # Register all test cases
    register_all()

    # ── List mode ──────────────────────────────────────────────────────
    if args.list_tests:
        cats = tests_by_category()
        print(f"\n  Available test categories ({len(cats)}):")
        for cat, tests in sorted(cats.items()):
            print(f"\n  \033[33;1m{cat}\033[0m ({len(tests)} tests):")
            for t in sorted(tests, key=lambda x: x.test_id):
                icon = "\033[36m[V]\033[0m" if t.interactive else "\033[32m[A]\033[0m"
                auto = "\033[90m(auto)\033[0m" if t.auto_verify else ""
                req = (
                    f" \033[90m[requires: {', '.join(t.required_caps)}]\033[0m"
                    if t.required_caps
                    else ""
                )
                print(f"    {icon} {t.test_id:30s} {t.name}{auto}{req}")
        print()
        return 0

    # ── Run mode ───────────────────────────────────────────────────────
    include_set = set(args.include.split(",")) if args.include else None
    exclude_set = set(args.exclude.split(",")) if args.exclude else None

    # ── Banner ─────────────────────────────────────��────────────────
    if not args.quiet:
        print("\033[36;1m")
        print("  ╔══════════════════════════════════════════════╗")
        print("  ║      Terminal Render Test Suite v0.1         ║")
        print("  ║    Test your terminal emulator's rendering   ║")
        print("  ╚══════════════════════════════════════════════╝")
        print("\033[0m")

    # Detect capabilities
    if not args.quiet:
        print("\033[33;1m  ── Detecting terminal capabilities ──\033[0m")
    global _term
    _term = get_terminal()
    term = _term
    time.sleep(0.2)

    caps = detect_capabilities(term)
    if not args.quiet:
        print(format_capabilities(caps))
        print()

    # ── Interactive test selection ────────────────────────────────────
    test_ids = None  # None means "no filter, run everything"
    has_cli_filters = bool(args.include or args.exclude or args.quick or args.auto)
    is_tty = hasattr(term, '_is_tty') and term._is_tty
    if not has_cli_filters and is_tty and not args.quiet:
        selected = interactive_select(term)
        if selected is None:
            term.write(b"\n  \033[33;1mTest selection cancelled.\033[0m\n")
            term.flush()
            return 0
        test_ids = selected
        # clear screen after selector
        term.write(b"\033[H\033[J")
        term.flush()

    # Build and run the suite
    runner = TestRunner(
        term=term,
        caps=caps,
        include=include_set,
        exclude=exclude_set,
        test_ids=test_ids,
        auto_mode=args.auto or args.quiet,
        quick_mode=args.quick,
        quiet_mode=args.quiet,
    )

    if not args.quiet:
        print("\033[33;1m  ── Running tests ──\033[0m\n")

    report = runner.run()
    term.reset_modes()

    return _print_summary(report, args, term)


def _print_summary(report: Report, args, term: Terminal) -> int:
    """Print results summary and write JSON output."""
    total, passed, failed, skipped, errors = report.summary()

    if not args.quiet:
        # Clear the screen and move to home so summary appears cleanly
        term.write(b"\033[2J\033[H")
        term.write("  \033[33;1m═══════════════════════════════════════\033[0m\r\n".encode())
        term.write(b"  \033[1m              RESULTS SUMMARY\033[0m\r\n")
        term.write(b"\r\n")
        term.write(f"    Total tests:  {total}\r\n".encode())
        term.write(f"    \033[32mPassed:\033[0m       {passed}\r\n".encode())
        term.write(f"    \033[31mFailed:\033[0m       {failed}\r\n".encode())
        term.write(f"    \033[33mSkipped:\033[0m      {skipped}\r\n".encode())
        term.write(f"    \033[31;1mErrors:\033[0m      {errors}\r\n".encode())
        term.write(f"    Duration:     {report.duration_seconds:.1f}s\r\n".encode())

        if total > 0:
            score = (passed / total) * 100
            colour = "32" if score >= 80 else "33" if score >= 50 else "31"
            term.write(f"    \033[{colour};1mScore:\033[0m        {score:.0f}%\r\n".encode())
        term.write(b"\r\n")

    # ── JSON output ────────────────────────────────────────────────────
    if args.json:
        json_output = report.to_json()
        with open(args.json, "w") as f:
            f.write(json_output)
        if not args.quiet:
            term.write(f"  \033[32m✓\033[0m Report written to {args.json}\r\n".encode())

    term.flush()

    # ── Exit code ──────────────────────────────────────────────────────
    if failed > 0 or errors > 0:
        return 1
    return 0


if __name__ == "__main__":
    exit_code = 1
    try:
        exit_code = main() or 0
        sys.exit(exit_code)
    except KeyboardInterrupt:
        print("\n\n  \033[33;1mTests interrupted by user.\033[0m")
    except Exception as e:
        print(f"\n  \033[31;1mFatal error: {e}\033[0m")
        if "--verbose" in sys.argv:
            import traceback

            traceback.print_exc()
    finally:
        # Always reset terminal modes on exit, even after crash/interrupt
        if _term is not None:
            _term.reset_modes()
            _term.write(b"\033[2J\033[H\r\n")
        sys.exit(exit_code)