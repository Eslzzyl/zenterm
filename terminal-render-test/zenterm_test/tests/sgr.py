"""SGR (Select Graphic Rendition) attribute tests.

Tests all standard SGR text attributes: bold, dim, italic, underline,
blink, reverse, conceal, strikethrough, double-underline, overline,
and extended underline styles.
"""

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    subheading,
    prompt_visual,
    register,
)


def _test_sgr_basic(term: Terminal) -> TestResultOrStatus:
    """Test basic SGR attributes 1-9."""
    heading(term, "SGR Basic Attributes (1-9)")

    tests = [
        ("1", "Bold"),
        ("2", "Dim"),
        ("3", "Italic"),
        ("4", "Underline"),
        ("5", "Slow Blink"),
        ("6", "Rapid Blink"),
        ("7", "Reverse"),
        ("8", "Conceal"),
        ("9", "Strikethrough"),
    ]

    for param, name in tests:
        term.write(f"\033[{param}m".encode())
        term.write(f"  {name:20s} (SGR {param})  ".encode())
        term.write(b"\033[0m  (normal)\n")
    term.flush()

    # Test combined attributes
    subheading(term, "Combined Attributes")
    combos = [
        ("1;31", "Bold Red"),
        ("1;4;34", "Bold Underline Blue"),
        ("3;33", "Italic Yellow"),
        ("1;3;32", "Bold Italic Green"),
        ("4;9", "Underline Strikethrough"),
        ("1;7;35", "Bold Reverse Magenta"),
    ]
    for param, name in combos:
        term.write(f"\033[{param}m".encode())
        term.write(f"  {name:30s}  ".encode())
        term.write(b"\033[0m\n")
    term.flush()

    term.write(b"\n")
    return prompt_visual(term, "All SGR basic attributes rendered correctly?")


def _test_sgr_reset(term: Terminal) -> TestResultOrStatus:
    """Test SGR resets: 21-29, 22, 23, 24, 25, 27, 28."""
    heading(term, "SGR Resets (21-29)")

    term.write(b"  \033[1mBold \033[22mNot bold  (22 resets 1)\n")
    term.write(b"  \033[3mItalic \033[23mNot italic  (23 resets 3)\n")
    term.write(b"  \033[4mUnderline \033[24mNot underlined  (24 resets 4)\n")
    term.write(b"  \033[5mBlink \033[25mNot blink  (25 resets 5)\n")
    term.write(b"  \033[7mReverse \033[27mNot reversed  (27 resets 7)\n")
    term.write(b"  \033[8mConceal \033[28mNot concealed  (28 resets 8)\n")
    term.write(b"  \033[9mStrikethrough \033[29mNot strikethrough  (29 resets 9)\n")
    term.flush()

    # Double underline (21) — note: some terminals interpret 21 as double-underline
    # others as "bold off" — we test both semantics
    term.write(b"\n")
    term.write(b"  \033[4mSingle \033[21mDouble underline (SGR 21)\033[0m\n")
    term.flush()

    return prompt_visual(
        term, "SGR resets working correctly? Double underline (21) rendered?"
    )


def _test_sgr_extended(term: Terminal) -> TestResultOrStatus:
    """Test extended SGR: overline (53), overline off (55), underline colors (58)."""
    heading(term, "Extended SGR (53, 55, 58)")

    term.write(b"  \033[53mOverlined text (SGR 53)\033[0m\n")
    term.write(b"  \033[53mOverlined \033[55mNot overlined\033[0m  (55 resets 53)\n")
    term.write(b"\n")

    # Underline colour (SGR 58) — Kitty extension
    subheading(term, "Underline Colour (SGR 58)")
    term.write(b"  \033[4mDefault underline\033[0m\n")
    term.write(b"  \033[4;58:2::255:0:0mRed underline\033[0m\n")
    term.write(b"  \033[4;58:2::0:255:0mGreen underline\033[0m\n")
    term.write(b"  \033[4;58:2::0:0:255mBlue underline\033[0m\n")
    term.write(b"  \033[4;58:5:196mOrange underline (256-colour)\033[0m\n")
    term.write(b"  \033[4;59mDefault underline colour restored (SGR 59)\033[0m\n")
    term.flush()

    return prompt_visual(
        term, "Extended attributes (overline, underline colour) rendered?"
    )


def _test_underline_styles(term: Terminal) -> TestResultOrStatus:
    """Test Kitty extended underline styles: 4:1 through 4:5."""
    heading(term, "Underline Styles (4:1 – 4:5)")

    term.write(b"  \033[4:0m4:0  No underline\033[0m\n")
    term.write(b"  \033[4:1m4:1  Straight underline\033[0m\n")
    term.write(b"  \033[4:2m4:2  Double underline\033[0m\n")
    term.write(b"  \033[4:3m4:3  Curly underline (spell-check)\033[0m\n")
    term.write(b"  \033[4:4m4:4  Dotted underline\033[0m\n")
    term.write(b"  \033[4:5m4:5  Dashed underline\033[0m\n")
    term.write(b"\n")
    term.write(b"  Coloured variant:\n")
    term.write(b"  \033[4:1;58:2::255:0:0mRed straight\033[0m\n")
    term.write(b"  \033[4:3;58:2::0:255:0mGreen curly\033[0m\n")
    term.flush()

    return prompt_visual(term, "Underline styles 4:1-4:5 rendered correctly?")


def register_sgr_tests():
    register(
        TestCase(
            "sgr-basic",
            "sgr",
            "SGR Basic (1-9)",
            "Bold, dim, italic, underline, blink, reverse, conceal, strikethrough",
            _test_sgr_basic,
        )
    )
    register(
        TestCase(
            "sgr-reset",
            "sgr",
            "SGR Resets (21-29)",
            "Reset attributes, double underline",
            _test_sgr_reset,
        )
    )
    register(
        TestCase(
            "sgr-extended",
            "sgr",
            "Extended SGR (53/55/58/59)",
            "Overline, underline colour",
            _test_sgr_extended,
        )
    )
    register(
        TestCase(
            "sgr-underline-styles",
            "sgr",
            "Underline Styles (4:1-4:5)",
            "Straight, double, curly, dotted, dashed underline",
            _test_underline_styles,
        )
    )
