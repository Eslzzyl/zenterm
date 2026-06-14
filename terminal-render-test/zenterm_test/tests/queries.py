"""Terminal query/response tests.

Tests Device Attributes (DA1/DA2), Device Status Reports (DSR),
DECRQM mode queries, colour queries, and other terminal responses.
"""

import time

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestResult,
    TestStatus,
    TestCase,
    heading,
    info,
    register,
)


def _test_da1(term: Terminal) -> TestResultOrStatus:
    """Query primary Device Attributes (DA1: ESC[c)."""
    heading(term, "Device Attributes 1 (DA1)")

    info(term, "Sending primary device attributes query...")
    resp = term.query_da1(timeout=2.0)

    if resp:
        decoded = resp.decode("ascii", errors="replace")
        term.write(f"  Response: {decoded!r}\n".encode())
        term.flush()

        # Parse codes
        import re

        codes = re.findall(r"\d+", decoded)
        info(term, f"  Feature codes: {', '.join(codes)}")

        term.write(b"\n  Common DA1 codes:\n")
        term.write(b"    Code 1  = 132-column mode\n")
        term.write(b"    Code 4  = Sixel graphics\n")
        term.write(b"    Code 6  = Selective erase\n")
        term.write(b"    Code 7  = Soft fonts\n")
        term.write(b"    Code 12 = ANSI colour\n")
        term.write(b"    Code 62 = VT220\n")
        term.write(b"    Code 64 = Sixel with 256 colours\n")

        return TestResult(
            test_id="queries-da1",
            category="queries",
            name="DA1 Query",
            status=TestStatus.PASS,
            message=f"Responded: {codes}",
        )
    else:
        info(term, "  No DA1 response received (timeout).")
        return TestResult(
            status=TestStatus.FAIL,
            test_id="queries-da1",
            category="queries",
            name="DA1 Query",
            message="Timeout — no response",
        )


def _test_da2(term: Terminal) -> TestResultOrStatus:
    """Query secondary Device Attributes (DA2: ESC[>c)."""
    heading(term, "Device Attributes 2 (DA2)")

    info(term, "Sending secondary device attributes query...")
    resp = term.query_da2(timeout=2.0)

    if resp:
        decoded = resp.decode("ascii", errors="replace")
        term.write(f"  Response: {decoded!r}\n".encode())
        term.flush()
        info(term, "DA2 often includes terminal vendor and version info.")
        return TestResult(
            status=TestStatus.PASS,
            test_id="queries-da2",
            category="queries",
            name="DA2 Query",
            message=f"Responded: {decoded.strip()}",
        )
    else:
        info(term, "  No DA2 response received (timeout).")
        return TestResult(
            status=TestStatus.SKIP,
            test_id="queries-da2",
            category="queries",
            name="DA2 Query",
            message="Timeout — not all terminals support DA2",
        )


def _test_dsr(term: Terminal) -> TestResultOrStatus:
    """Query cursor position (DSR: ESC[6n)."""
    heading(term, "Device Status Report (DSR)")

    # Move to known position and query
    positions = [(1, 1), (5, 20), (10, 50), (3, 72)]
    all_correct = True

    for row, col in positions:
        term.write(f"\033[{row};{col}H".encode())
        pos = term.query_dsr(timeout=1.0)
        if pos:
            r, c = pos
            match = r == row and c == col
            status = "✓" if match else "✗"
            term.write(
                f"  \033[3{2 if match else 1}m{status}\033[0m  \033[90mDSR({row},{col})\033[0m".encode()
            )
            term.write(f" → ({r},{c})\n".encode())
            if not match:
                all_correct = False
        else:
            term.write(f"  ✗  DSR({row},{col}) → timeout\n".encode())
            all_correct = False
        term.flush()
        time.sleep(0.1)

    term.write(b"\n")

    if all_correct:
        return TestResult(
            status=TestStatus.PASS,
            test_id="queries-dsr",
            category="queries",
            name="DSR Cursor Position",
            message="All positions verified",
        )
    else:
        return TestResult(
            status=TestStatus.FAIL,
            test_id="queries-dsr",
            category="queries",
            name="DSR Cursor Position",
            message="Some positions mismatched",
        )


def _test_decrm_queries(term: Terminal) -> TestResultOrStatus:
    """Query various DEC private mode statuses via DECRQM (ESC[?mode$p)."""
    heading(term, "DECRQM Mode Queries")

    modes_to_check = [
        (1, "Application Cursor Keys"),
        (7, "Auto-Wrap Mode"),
        (25, "Cursor Visible"),
        (40, "Allow 132-column"),
        (69, "Left/Right Margins"),
        (1000, "Normal Mouse"),
        (1002, "Button-Event Mouse"),
        (1004, "Focus Events"),
        (1006, "SGR Mouse"),
        (2004, "Bracketed Paste"),
        (2026, "Sync Output"),
        (1049, "Save/Restore Screen"),
    ]

    any_response = False
    for mode, desc in modes_to_check:
        resp = term.query_decrm(mode, timeout=0.5)
        if resp:
            any_response = True
            decoded = resp.decode("ascii", errors="replace").strip()
            term.write(
                f"  \033[32m✓\033[0m  Mode {mode:4d} ({desc:30s}) → {decoded}\n".encode()
            )
        else:
            term.write(
                f"  \033[90m⊘\033[0m  Mode {mode:4d} ({desc:30s}) → no response\n".encode()
            )
        term.flush()

    if any_response:
        return TestResult(
            status=TestStatus.PASS,
            test_id="queries-decrm",
            category="queries",
            name="DECRQM Mode Queries",
            message="Modes responded",
        )
    else:
        return TestResult(
            status=TestStatus.SKIP,
            test_id="queries-decrm",
            category="queries",
            name="DECRQM Mode Queries",
            message="No DECRQM support detected",
        )


def _test_color_queries(term: Terminal) -> TestResultOrStatus:
    """Query colour palette values via OSC."""
    heading(term, "Colour Queries")

    # Query a few palette entries
    total = 0
    for n in [0, 1, 4, 7, 15, 16, 232]:
        resp = term.query_osc_color(n, timeout=0.5)
        if resp:
            term.write(f"  \033[32m✓\033[0m  Colour {n:3d}: {resp}\n".encode())
            total += 1
        else:
            term.write(f"  \033[90m⊘\033[0m  Colour {n:3d}: no response\n".encode())
        term.flush()

    if total > 0:
        return TestResult(
            status=TestStatus.PASS,
            test_id="queries-colors",
            category="queries",
            name="OSC Colour Queries",
            message=f"{total} colours responded",
        )
    else:
        return TestResult(
            status=TestStatus.SKIP,
            test_id="queries-colors",
            category="queries",
            name="OSC Colour Queries",
            message="No colour query support",
        )


def register_queries_tests():
    register(
        TestCase(
            "queries-da1",
            "queries",
            "Device Attributes 1 (DA1)",
            "Primary device attributes query",
            _test_da1,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "queries-da2",
            "queries",
            "Device Attributes 2 (DA2)",
            "Secondary device attributes query",
            _test_da2,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "queries-dsr",
            "queries",
            "DSR Cursor Position",
            "Verify cursor position report accuracy",
            _test_dsr,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "queries-decrm",
            "queries",
            "DECRQM Mode Queries",
            "Query private mode status for 12 modes",
            _test_decrm_queries,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "queries-colors",
            "queries",
            "OSC Colour Queries",
            "Query palette colours via OSC 4",
            _test_color_queries,
            auto_verify=True,
        )
    )
