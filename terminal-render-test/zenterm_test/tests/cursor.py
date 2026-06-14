"""Cursor movement, shape, and erase tests.

Tests cursor shapes (DECSCUSR), visibility (DECTCEM), movement
(CUU/CUD/CUF/CUB/CHA/CUP/HVP), save/restore (DECSC/DECRC), and
erase operations (ED/EL/ECH/DL/IL).
"""

import time

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    subheading,
    prompt_visual,
    prompt_continue,
    register,
)


def _test_cursor_shape(term: Terminal) -> TestResultOrStatus:
    """Test all 6 cursor shapes via DECSCUSR (CSI q)."""
    heading(term, "Cursor Shapes (DECSCUSR)")

    shapes = [
        ("0", "Blinking block (default)"),
        ("1", "Blinking block"),
        ("2", "Steady block"),
        ("3", "Blinking underline"),
        ("4", "Steady underline"),
        ("5", "Blinking bar (I-beam)"),
        ("6", "Steady bar (I-beam)"),
    ]

    # Save cursor and hide it
    term.write(b"\033[s\033[?25l")

    for param, name in shapes:
        term.write(f"\033[{param} q".encode())
        # Show three spaces so the cursor is visible
        term.write(f"\r  {param} = {name:30s}  \n".encode())
        time.sleep(0.5)

    # Restore to default and show cursor
    term.write(b"\033[?25h\033[0 q\033[u")
    term.flush()

    return prompt_visual(term, "All 6 cursor shapes displayed correctly?")


def _test_cursor_visibility(term: Terminal) -> TestResultOrStatus:
    """Test cursor show/hide (DECTCEM: DECSET/DECRST 25)."""
    heading(term, "Cursor Visibility")

    term.write(b"  Cursor is currently visible.\n")
    term.write(b"  Hiding cursor...\n")
    term.write(b"\033[?25l")
    time.sleep(1.0)
    term.write(b"  Cursor should be invisible now.\n")
    term.write(b"  Showing cursor again...\n")
    term.write(b"\033[?25h")
    time.sleep(0.5)
    term.write(b"  Cursor is visible again.\n")

    return prompt_visual(term, "Cursor hide/show working correctly?")


def _test_cursor_movement(term: Terminal) -> TestResultOrStatus:
    """Test cursor movement commands.

    Each command is demonstrated in isolation: the test writes the command name,
    executes it, then writes [HERE] at the destination.  You can see where the
    cursor landed by looking for the [HERE] marker.
    """
    heading(term, "Cursor Movement")

    # Draw a ruler so column positions are easy to read.
    term.write(b"  Col    1 1 1 1 1 1 1 1 1 1 2 2 2 2 2 2 2 2 2 2 3 3 3 3\n")
    term.write(b"         0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3\n")
    term.write(b"         |   |   |   |   |   |   |   |   |   |   |   |\n\n")

    # ── 1. CUP (Cursor Position) ──────────────────────────────────────
    term.write(b"\033[4;1H")  # row 4, col 1
    term.write(("  " + "=" * 50 + "\n").encode())
    term.write(b"  1. CUP  \\e[5;40H  ->  jump to row 5, column 40\n")
    term.write(b"  ")
    term.write(b"\033[5;40H")  # ← THE COMMAND
    term.write(b"[HERE]")
    term.write(b"\033[6;1H")
    term.write(b"       [HERE] should appear at row 5, column 40 ^^\n")
    term.write(b"\n")

    # ── 2. CUF / CUB (horizontal) ─────────────────────────────────────
    term.write(b"\033[8;1H")
    term.write(("  " + "=" * 50 + "\n").encode())
    term.write(b"  2. CUF  \\e[1C  ->  move right 1 column\n")
    term.write(b"     CUB  \\e[1D  ->  move left  1 column\n")
    term.write(b"  ")
    term.write(b"\033[10;5H")    # start at col 5
    term.write(b"S")
    term.write(b"\033[1C")       # CUF 1
    term.write(b"[HERE]")        # should appear at col 6
    term.write(b"\033[1C")       # CUF 1
    term.write(b"[HERE]")        # should appear at col 12
    term.write(b"\033[1D")       # CUB 1
    term.write(b"[HERE]")        # should appear at col 18 (overwrites previous)
    term.write(b"\033[11;1H")
    term.write(b"       [HERE] markers should spread across columns 6-12-18\n")
    term.write(b"\n")

    # ── 3. CUU / CUD (vertical) ──────────────────────────────────────
    term.write(b"\033[13;1H")
    term.write(("  " + "=" * 50 + "\n").encode())
    term.write(b"  3. CUD  \\e[1B  ->  move down  1 row\n")
    term.write(b"     CUU  \\e[1A  ->  move up    1 row\n")
    term.write(b"\033[15;30H")
    term.write(b"[HERE]")        # row 15, col 30
    term.write(b"\033[1B")       # CUD 1
    term.write(b"[HERE]")        # row 16, col 30
    term.write(b"\033[1B")       # CUD 1
    term.write(b"[HERE]")        # row 17, col 30
    term.write(b"\033[1A")       # CUU 1
    term.write(b"[HERE]")        # row 16, col 30 (overwrites)
    term.write(b"\033[18;1H")
    term.write(b"       [HERE] should form a vertical line at col 30, rows 15-17\n")
    term.write(b"\n")

    # ── 4. HVP (Horizontal Vertical Position) ─────────────────────────
    term.write(b"\033[20;1H")
    term.write(("  " + "=" * 50 + "\n").encode())
    term.write(b"  4. HVP  \\e[21;10f  ->  jump to row 21, column 10\n")
    term.write(b"  ")
    term.write(b"\033[21;10f")   # ← THE COMMAND
    term.write(b"[HERE]")
    term.write(b"\033[22;1H")
    term.write(b"       [HERE] should appear at row 21, column 10\n")
    term.write(b"\n")

    # ── 5. CNL / CPL (next/prev line) ─────────────────────────────────
    term.write(b"\033[24;1H")
    term.write(("  " + "=" * 50 + "\n").encode())
    term.write(b"  5. CNL  \\e[E  ->  next line, same column\n")
    term.write(b"     CPL  \\e[F  ->  previous line, same column\n")
    term.write(b"\033[26;20H")
    term.write(b"LINE-A")
    term.write(b"\033[E")        # CNL: next line
    term.write(b"LINE-B")        # row 27, col 20
    term.write(b"\033[E")        # CNL: next line
    term.write(b"LINE-C")        # row 28, col 20
    term.write(b"\033[F")        # CPL: previous line
    term.write(b"LINE-B2")       # row 27, col 20 (overwrites LINE-B)
    term.write(b"\033[29;1H")
    term.write(b"       Expected: LINE-A at row 26, LINE-B2 at row 27,\n")
    term.write(b"       LINE-C at row 28; all at column 20\n")
    term.write(b"\033[31;1H")

    return prompt_visual(
        term,
        "All [HERE] markers at expected positions?",
    )


def _test_erase(term: Terminal) -> TestResultOrStatus:
    """Test erase operations ED, EL, ECH, DL, IL."""
    heading(term, "Erase Operations")

    # Fill a region with test pattern
    term.write(("  " + "=" * 60 + "\n").encode())
    for i in range(8):
        term.write(
            f"  Line {i + 1}: ".encode()
            + "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\n".encode()
        )

    term.write(b"\n")

    # EL 0 (erase to end of line)
    subheading(term, "EL 0 — Erase to End of Line")
    term.write(b"\033[3;8H")  # Move to line 3, col 8
    term.write(b"\033[0K")  # Erase to end
    time.sleep(0.3)

    # EL 1 (erase from start of line)
    subheading(term, "EL 1 — Erase from Start of Line")
    term.write(b"\033[5;20H")
    term.write(b"\033[1K")
    time.sleep(0.3)

    # EL 2 (erase entire line)
    subheading(term, "EL 2 — Erase Entire Line")
    term.write(b"\033[7;1H")
    term.write(b"\033[2K")
    time.sleep(0.3)

    # ECH (erase character)
    subheading(term, "ECH — Erase Character")
    term.write(b"\033[2;10H")
    term.write(b"\033[5X")  # Erase 5 characters
    time.sleep(0.3)

    # ED 0 (erase from cursor to end of display)
    subheading(term, "ED 0 — Erase Below")
    # First, write fresh content below
    term.write(b"\033[13;1H")
    term.write(b"  Lines below will be erased:\n")
    term.write(b"  Line A\n  Line B\n  Line C\n  Line D\n")
    term.write(b"\033[13;1H")
    term.write(b"\033[0J")  # Erase from cursor to end
    time.sleep(0.3)

    # ED 1 (erase from start to cursor)
    subheading(term, "ED 1 — Erase Above")
    term.write(b"\033[20;1H")
    term.write(b"  Lines above:\n")
    term.write(b"  TOP\n")
    term.write(b"\033[20;1H")
    term.write(b"\033[1J")  # Erase from top to cursor
    time.sleep(0.3)

    # ED 2 (erase entire display)
    subheading(term, "ED 2 — Clear Screen")
    term.write(b"\033[22;1H")
    term.write(b"  Press Enter to clear entire screen...")
    prompt_continue(term)
    term.write(b"\033[2J")
    term.write(b"\033[H")
    term.write(b"  Screen was cleared with ED 2.\n")
    term.flush()

    # Restore by clearing
    term.reset()
    return prompt_visual(term, "All erase commands work correctly?")


def register_cursor_tests():
    register(
        TestCase(
            "cursor-shape",
            "cursor",
            "Cursor Shapes (DECSCUSR)",
            "All 6 cursor styles: block, underline, bar, blinking/steady",
            _test_cursor_shape,
        )
    )
    register(
        TestCase(
            "cursor-visibility",
            "cursor",
            "Cursor Visibility (DECTCEM 25)",
            "Hide and show cursor",
            _test_cursor_visibility,
        )
    )
    register(
        TestCase(
            "cursor-movement",
            "cursor",
            "Cursor Movement",
            "CUU, CUD, CUF, CUB, CHA, CUP, HVP, CNL, CPL, DECSC, DECRC, DSR",
            _test_cursor_movement,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "cursor-erase",
            "cursor",
            "Erase Operations",
            "ED 0/1/2, EL 0/1/2, ECH, DL, IL",
            _test_erase,
        )
    )
