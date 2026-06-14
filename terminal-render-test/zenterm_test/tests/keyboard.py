"""Kitty Keyboard Protocol tests.

Tests progressive enhancement flags: disambiguate (1), report events (2),
report alternates (4), report all keys (8), report text (16).
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
    prompt_visual,
    register,
)


def _test_keyboard_legacy(term: Terminal) -> TestResultOrStatus:
    """Test legacy keyboard encoding (default mode)."""
    heading(term, "Keyboard: Legacy Mode")

    info(term, "Testing basic key encoding in legacy mode.")
    info(term, "This test is interactive - press the following keys:")

    term.write(b"\n")
    tests = [
        ("Enter", "Press Enter and observe the response"),
        ("Escape", "Press Escape - should generate \\x1b"),
        ("Tab", "Press Tab - should generate \\x09"),
        ("Arrow Up", "Press Up Arrow - should generate \\x1b[A"),
        ("Arrow Down", "Press Down Arrow - should generate \\x1b[B"),
        ("Arrow Right", "Press Right Arrow - should generate \\x1b[C"),
        ("Arrow Left", "Press Left Arrow - should generate \\x1b[D"),
        ("Home", "Press Home - should generate \\x1b[H or \\x1b[1~"),
        ("End", "Press End - should generate \\x1b[F or \\x1b[4~"),
        ("Page Up", "Press Page Up - should generate \\x1b[5~"),
        ("Page Down", "Press Page Down - should generate \\x1b[6~"),
    ]

    for name, desc in tests:
        term.write(f"  {name:15s}: {desc}\n".encode())

    term.write(b"\n")
    info(term, "All these should work in any terminal emulator.")

    return prompt_visual(term, "Legacy key encodings work correctly?")


def _test_keyboard_kitty_disambiguate(term: Terminal) -> TestResultOrStatus:
    """Test Kitty keyboard protocol: disambiguate (flag 1)."""
    heading(term, "Kitty Keyboard: Disambiguate (flag=1)")

    info(term, "The disambiguate flag fixes overlapping escape codes.")
    info(term, "For example, Escape key produces \\x1b which also")
    info(term, "starts escape sequences. With flag 1, it's disambiguated.")

    # Enable disambiguate only
    info(term, "Enabling Kitty keyboard protocol (flag=1)...")
    term.write(b"\033[=1u")
    time.sleep(0.3)

    info(term, "")
    info(term, "Press some keys now. Responses should use CSI u encoding.")
    info(term, "Escape → \\e[27u  (instead of raw \\x1b)")
    info(term, "Tab    → \\e[9u   (instead of raw \\x09)")
    info(term, "Enter  → \\e[13u  (instead of raw \\x0d)")
    info(term, "Up     → \\e[1A   (unchanged in disambiguate mode)")
    info(term, "")

    try:
        result = prompt_visual(term, "Disambiguated key codes observed?")
    finally:
        # Restore default keyboard mode
        term.write(b"\033[<u")
        time.sleep(0.2)

    if result != TestStatus.PASS:
        return TestResult(
            test_id="keyboard-kitty-disambiguate",
            category="keyboard",
            name="Kitty Disambiguate (flag=1)",
            status=result,
        )

    # (cleanup in finally block above)
    return TestResult(
        test_id="keyboard-kitty-disambiguate",
        category="keyboard",
        name="Kitty Disambiguate (flag=1)",
        status=TestStatus.PASS,
    )


def _test_keyboard_kitty_report_events(term: Terminal) -> TestResultOrStatus:
    """Test Kitty keyboard protocol: report events (flag=3 = 1|2)."""
    heading(term, "Kitty Keyboard: Report Events (flag=3)")

    info(term, "Flag 2 adds repeat and release event reporting.")
    info(term, "Combined as flag=3 (1|2) for disambiguate + events.")

    # Enable disambiguate + report events
    term.write(b"\033[=3u")
    time.sleep(0.3)

    info(term, "")
    info(term, 'Press and HOLD a key (e.g., "a").')
    info(term, "You should see:")
    info(term, "  Press:   \\e[97u  (key a)")
    info(term, "  Repeat:  \\e[97;2u (with repeat count)")
    info(term, "  Release: \\e[97;3u (release event)")
    info(term, "")

    result = prompt_visual(term, "Repeat and release events observed?")
    if result != TestStatus.PASS:
        return TestResult(
            test_id="keyboard-kitty-report-events",
            category="keyboard",
            name="Kitty Report Events (flag=3)",
            status=result,
        )

    term.write(b"\033[<u")
    time.sleep(0.2)
    return TestResult(
        test_id="keyboard-kitty-report-events",
        category="keyboard",
        name="Kitty Report Events (flag=3)",
        status=TestStatus.PASS,
    )


def _test_keyboard_kitty_all_keys(term: Terminal) -> TestResultOrStatus:
    """Test Kitty keyboard protocol: report all keys (flag=8)."""
    heading(term, "Kitty Keyboard: Report All Keys (flag=8)")

    info(term, "Flag 8 reports ALL keys as CSI u sequences, including")
    info(term, "modifier+key combinations that would normally produce")
    info(term, "ASCII control characters (Ctrl+A, Ctrl+C, etc.).")

    # Enable disambiguate + all keys
    term.write(b"\033[=9u")  # 1 | 8
    time.sleep(0.3)

    info(term, "")
    info(term, "Press Ctrl+A, Ctrl+C, Ctrl+Z, Ctrl+[")
    info(term, "They should report as CSI u codes with modifiers:")
    info(term, "  Ctrl+A → \\e[97;5u  (key a, mod=ctrl)")
    info(term, "  Ctrl+C → \\e[99;5u  (key c, mod=ctrl)")
    info(term, "")

    result = prompt_visual(term, "All keys reported as CSI u with modifiers?")
    if result != TestStatus.PASS:
        return TestResult(
            test_id="keyboard-kitty-all-keys",
            category="keyboard",
            name="Kitty Report All Keys (flag=8)",
            status=result,
        )

    term.write(b"\033[<u")
    time.sleep(0.2)
    return TestResult(
        test_id="keyboard-kitty-all-keys",
        category="keyboard",
        name="Kitty Report All Keys (flag=8)",
        status=TestStatus.PASS,
    )


def _test_keyboard_modifiers(term: Terminal) -> TestResultOrStatus:
    """Test modifier reporting in Kitty keyboard protocol."""
    heading(term, "Keyboard: Modifier Reporting")

    info(term, "Test modifier keys and their combinations.")
    info(term, "Press these combinations and verify the output:")

    term.write(b"\n")
    term.write(b'  Shift+A    -> should produce "A" (uppercase)\n')
    term.write(b"  Ctrl+C     -> should break/not break the test\n")
    term.write(b"  Alt+Tab    -> should switch windows or be captured\n")
    term.write(b"  Ctrl+Shift+A -> uppercase with ctrl modifier\n")
    term.write(b"  Alt+F4     -> varies by platform\n")
    term.write(b"\n")

    # Enable full protocol for testing
    info(term, "Enabling Kitty keyboard protocol (flag=15 = 1|2|4|8)...")
    term.write(b"\033[=15u")
    time.sleep(0.3)

    info(term, "")
    info(term, "Now press Shift+A, Ctrl+Shift+F1, Alt+Up")
    info(term, "Observe the encoded responses.")
    info(term, "")

    result = prompt_visual(term, "Modifier combinations reported correctly?")
    if result != TestStatus.PASS:
        return TestResult(
            test_id="keyboard-modifiers",
            category="keyboard",
            name="Keyboard Modifiers",
            status=result,
        )

    term.write(b"\033[<u")
    time.sleep(0.2)
    return TestResult(
        test_id="keyboard-modifiers",
        category="keyboard",
        name="Keyboard Modifiers",
        status=TestStatus.PASS,
    )


def register_keyboard_tests():
    register(
        TestCase(
            "keyboard-legacy",
            "keyboard",
            "Legacy Keyboard Encoding",
            "Standard key codes: Enter, Escape, arrows, etc.",
            _test_keyboard_legacy,
            interactive=True,
        )
    )
    register(
        TestCase(
            "keyboard-kitty-disambiguate",
            "keyboard",
            "Kitty Disambiguate (flag=1)",
            "Escape code disambiguation: \\e[27u instead of raw \\x1b",
            _test_keyboard_kitty_disambiguate,
            interactive=True,
        )
    )
    register(
        TestCase(
            "keyboard-kitty-report-events",
            "keyboard",
            "Kitty Report Events (flag=3)",
            "Repeat count and release event reporting",
            _test_keyboard_kitty_report_events,
            interactive=True,
        )
    )
    register(
        TestCase(
            "keyboard-kitty-all-keys",
            "keyboard",
            "Kitty Report All Keys (flag=8)",
            "All keys as CSI u with modifiers",
            _test_keyboard_kitty_all_keys,
            interactive=True,
        )
    )
    register(
        TestCase(
            "keyboard-modifiers",
            "keyboard",
            "Keyboard Modifiers",
            "Shift, Ctrl, Alt combinations with Kitty protocol",
            _test_keyboard_modifiers,
            interactive=True,
        )
    )
