"""Screen mode and terminal protocol tests.

Tests alternate screen buffer, bracketed paste, focus events,
mouse tracking modes, synchronized output, and scroll regions.
"""

import os
import time

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    subheading,
    info,
    prompt_visual,
    prompt_continue,
    register,
)


def _test_alternate_screen(term: Terminal) -> TestResultOrStatus:
    """Test alternate screen buffer (DEC 1049)."""
    heading(term, "Alternate Screen Buffer (DECSET 1049)")

    info(term, "Full-screen programs (vim, less) use the alternate screen.")
    info(term, "Switching to alternate screen...")
    term.write(b"\033[?1049h")
    time.sleep(0.5)

    # Write content in alt screen
    term.write(b"\033[2J\033[H")
    term.write("  ┌─────────────────────────────────────────┐\n".encode("utf-8"))
    term.write("  │     ALTERNATE SCREEN BUFFER              │\n".encode("utf-8"))
    term.write("  │                                           │\n".encode("utf-8"))
    term.write("  │  This is the alternate screen.            │\n".encode("utf-8"))
    term.write("  │  The main screen content is preserved.    │\n".encode("utf-8"))
    term.write("  │                                           │\n".encode("utf-8"))
    term.write("  │  Press Enter to return to main screen...  │\n".encode("utf-8"))
    term.write("  └─────────────────────────────────────────┘\n".encode("utf-8"))
    term.flush()

    prompt_continue(term)

    # Exit alternate screen
    term.write(b"\033[?1049l")
    time.sleep(0.3)

    info(term, "Back in main screen. Previous content should still be here.")
    return prompt_visual(
        term, "Alternate screen switched correctly? Main screen preserved?"
    )


def _test_bracketed_paste(term: Terminal) -> TestResultOrStatus:
    """Test bracketed paste mode (DEC 2004)."""
    heading(term, "Bracketed Paste Mode (DECSET 2004)")

    info(term, "When enabled, pasted text is surrounded with \\e[200~ ... \\e[201~")
    info(term, "This lets applications distinguish typed from pasted input.")

    term.write(b"\n  Enabling bracketed paste mode...\n")
    term.write(b"\033[?2004h")

    info(term, "Now copy some text and paste it with Ctrl+Shift+V or Cmd+V.")
    info(term, "You should see the brackets around pasted text.")

    prompt_continue(term)

    term.write(b"\033[?2004l")
    info(term, "Bracketed paste mode disabled.")

    return prompt_visual(term, "Pasted text showed \\e[200~ ... \\e[201~ markers?")


def _test_focus_events(term: Terminal) -> TestResultOrStatus:
    """Test focus in/out events (DEC 1004)."""
    heading(term, "Focus Events (DECSET 1004)")

    info(term, "Focus events report \\e[I (focused) and \\e[O (unfocused).")

    term.write(b"\033[?1004h")
    term.write(b"\n  Focus events enabled.\n")
    info(term, "Click on another window (unfocus), then click back (focus).")

    # Poll for focus events - we read raw input to see them
    import select
    import sys

    term.write(b"\n  Watching for events (3 seconds)...\n")
    term.flush()

    # Read any focus events
    try:
        term.enter_raw_mode()
        deadline = time.monotonic() + 3.0
        buf = bytearray()
        while time.monotonic() < deadline:
            if select.select([sys.stdin], [], [], 0.1)[0]:
                chunk = os.read(sys.stdin.fileno(), 1024)
                if chunk:
                    buf.extend(chunk)
        if buf:
            info(term, f"Received {len(buf)} raw bytes during focus check.")
        else:
            info(term, "No focus events detected (or terminal doesn't support them).")
    finally:
        term.exit_raw_mode()

    term.write(b"\033[?1004l")
    term.flush()

    return prompt_visual(
        term, "Focus events received (\\e[I / \\e[O) when switching windows?"
    )




def _test_sync_output(term: Terminal) -> TestResultOrStatus:
    """Test synchronized output (DEC 2026)."""
    heading(term, "Synchronized Output (DEC 2026)")

    info(term, "Sync output groups drawing commands to prevent tearing.")
    info(term, "If supported, the block below should appear at once without flicker.")

    # Without sync (reference — may flicker)
    subheading(term, "Without Sync (reference)")
    for i in range(5):
        term.write(f"\033[48;2;{255 if i % 2 == 0 else 0};0;0m  \033[0m".encode())
        time.sleep(0.05)
    term.write(b"\n")

    # With sync
    subheading(term, "With Sync Output")
    term.write(b"\033[?2026h")  # Begin sync
    for i in range(5):
        term.write(f"\033[48;2;{255 if i % 2 == 0 else 0};0;0m  \033[0m".encode())
        time.sleep(0.05)
    term.write(b"\033[?2026l")  # End sync - should render all at once
    term.write(b"\n")

    # Rapid colour cycling in sync mode
    subheading(term, "Rapid cycling in sync mode")
    term.write(b"\033[?2026h")
    for i in range(40):
        r = (i * 6) % 256
        term.write(f"\033[48;2;{r};{128};{255 - r}m \033[0m".encode())
    term.write(b"\033[?2026l")
    term.write(b"\n\n")

    info(term, "Without sync: each update appears individually (flicker possible).")
    info(term, "With sync: entire block should appear at once, no tearing.")
    info(term, "Supported in: kitty, WezTerm, iTerm2, Ghostty, foot, alacritty?")

    return prompt_visual(term, "Sync output renders smoothly without tearing?")


def _test_scroll_regions(term: Terminal) -> TestResultOrStatus:
    """Test scroll regions (DECSTBM), horizontal margins (DECSET 69), and scrolling."""
    heading(term, "Scroll Regions & Margins")

    # Fill screen with numbered lines
    subheading(term, "Scroll Region (DECSTBM)")
    info(term, "Setting scroll region to rows 5-10...")
    term.write(b"\033[5;10r")  # DECSTBM: top=5, bottom=10
    term.write(b"\033[H")

    # Write 15 lines — only rows 5-10 should scroll
    for i in range(15):
        term.write(f"  Line {i:2d}: ABCDEFGHIJKLMNOPQRSTUVWXYZ\n".encode())

    term.write(b"\033[r")  # Reset margins
    term.write(b"\n\n")

    # Scrolling test
    subheading(term, "Scrolling (SU/SD)")
    term.write(b"\033[12;1H")
    term.write(b"  Scroll up test:\n")
    for i in range(5):
        term.write(f"  Row {15 + i}\n".encode())
    term.write(b"\033[15;1H")
    term.write(b"\033[3S")  # Scroll up 3 lines (SU)
    info(term, "Line range should have scrolled up by 3.")
    time.sleep(0.5)

    # Reverse scroll (SD)
    term.write(b"\033[15;1H")
    term.write(b"\033[3T")  # Scroll down 3 lines (SD)
    info(term, "Line range should have scrolled down by 3.")

    # Reset
    term.write(b"\033[r\033[H")
    time.sleep(0.3)
    term.reset()
    return prompt_visual(term, "Scroll regions and scrolling commands work correctly?")


def _test_mouse_normal(term: Terminal) -> TestResultOrStatus:
    """Test normal mouse tracking mode (DECSET 1000)."""
    heading(term, "Mouse: Normal Tracking (DECSET 1000)")

    info(term, "Normal mode: reports button press events.")
    info(term, "Enable and click around the screen.")
    info(term, "Press Ctrl+C or press 'q' to exit when done.")

    term.write(b"\033[?1000h\033[?1006h")  # Enable SGR mouse
    term.write(b"\n  Click anywhere in the terminal...\n")
    term.write(b"  Expected: \\e[<row;col;M (press) and \\e[<row;col;m (release)\n")
    term.flush()

    prompt_continue(term)

    term.write(b"\033[?1000l\033[?1006l")
    term.flush()
    return prompt_visual(term, "Mouse clicks reported with SGR encoding?")


def _test_mouse_motion(term: Terminal) -> TestResultOrStatus:
    """Test mouse motion tracking mode (DECSET 1002)."""
    heading(term, "Mouse: Button-Event Motion (DECSET 1002)")

    info(term, "Motion mode: reports button press + drag events.")
    info(term, "Enable and drag with button held down.")

    term.write(b"\033[?1002h\033[?1006h")
    term.write(b"\n  Click and drag within the terminal...\n")
    term.write(b"  Expected: continuous \\e[<row;col;M sequences\n")
    term.flush()

    prompt_continue(term)

    term.write(b"\033[?1002l\033[?1006l")
    term.flush()
    return prompt_visual(term, "Mouse drag events reported correctly?")


def _test_mouse_sgr_pixels(term: Terminal) -> TestResultOrStatus:
    """Test SGR pixels mouse mode (DECSET 1016)."""
    heading(term, "Mouse: SGR Pixels Mode (DECSET 1016)")

    info(term, "SGR pixels mode: coordinates in pixels, not cells.")
    info(term, "Only supported by: kitty, WezTerm, Ghostty.")
    info(term, "Enable and click — coordinates should be pixel-precise.")

    term.write(b"\033[?1002h\033[?1006h\033[?1016h")
    term.write(b"\n  Click at various positions...\n")
    term.write(b"  Expected: \\e[<px_x>;<px_y>;M (pixel coords)\n")
    term.flush()

    prompt_continue(term)

    term.write(b"\033[?1002l\033[?1006l\033[?1016l")
    term.flush()
    return prompt_visual(
        term, "Pixel-precise coordinates reported? (kitty/WezTerm/Ghostty?)"
    )


def register_modes_tests():
    register(
        TestCase(
            "modes-alt-screen",
            "modes",
            "Alternate Screen Buffer",
            "Enter/exit alternate screen (DECSET 1049)",
            _test_alternate_screen,
            interactive=True,
        )
    )
    register(
        TestCase(
            "modes-bracketed-paste",
            "modes",
            "Bracketed Paste",
            "Enable/disable paste markers (DECSET 2004)",
            _test_bracketed_paste,
            interactive=True,
        )
    )
    register(
        TestCase(
            "modes-focus-events",
            "modes",
            "Focus Events",
            "Track focus in/out (DECSET 1004)",
            _test_focus_events,
            interactive=True,
        )
    )
    register(
        TestCase(
            "modes-sync-output",
            "modes",
            "Synchronized Output",
            "Tear-free rendering (DEC 2026)",
            _test_sync_output,
        )
    )
    register(
        TestCase(
            "modes-scroll-regions",
            "modes",
            "Scroll Regions",
            "DECSTBM, SU, SD, margins",
            _test_scroll_regions,
        )
    )
    register(
        TestCase(
            "modes-mouse-normal",
            "modes",
            "Mouse: Normal Tracking",
            "SGR mouse button press/report (DECSET 1000+1006)",
            _test_mouse_normal,
            interactive=True,
        )
    )
    register(
        TestCase(
            "modes-mouse-motion",
            "modes",
            "Mouse: Motion Tracking",
            "Drag events with button held (DECSET 1002)",
            _test_mouse_motion,
            interactive=True,
        )
    )
    register(
        TestCase(
            "modes-mouse-pixels",
            "modes",
            "Mouse: SGR Pixels",
            "Pixel-precise coordinates (DECSET 1016)",
            _test_mouse_sgr_pixels,
            interactive=True,
        )
    )
