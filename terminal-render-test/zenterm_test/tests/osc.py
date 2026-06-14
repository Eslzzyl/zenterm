"""OSC (Operating System Command) tests.

Tests window title, clipboard, hyperlinks, colour palette changes,
notifications, and semantic prompts.
"""

import time

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    subheading,
    info,
    prompt_visual,
    register,
)


def _test_osc_title(term: Terminal) -> TestResultOrStatus:
    """Test OSC 0/1/2: window title and icon name."""
    heading(term, "Window Title & Icon (OSC 0/1/2)")

    term.write(b"  Setting window title...\n")
    term.write(b"\033]2;zenterm-test: Window Title Test\033\\")
    info(term, 'Title set to: "zenterm-test: Window Title Test"')
    info(term, "Look at your terminal window/tab title bar above.")

    term.write(b"\n  Press Enter when ready to restore...")
    term.flush()
    time.sleep(0.5)

    # Restore original title (or just test)
    term.write(b"\033]2;zenterm-test\033\\")
    return prompt_visual(term, "Window title changed to test string?")


def _test_osc8_hyperlinks(term: Terminal) -> TestResultOrStatus:
    """Test OSC 8 hyperlinks."""
    heading(term, "OSC 8 Hyperlinks")

    info(term, "Clickable links using OSC 8 protocol.")

    term.write(b"\n")
    term.write(b"  Regular URL: ")
    term.write(b"\033]8;;https://github.com\033\\")
    term.write(b"https://github.com")
    term.write(b"\033]8;;\033\\")
    term.write(b"\n")

    term.write(b"  Named link:  ")
    term.write(b"\033]8;;https://www.rust-lang.org\033\\")
    term.write(b"Rust Programming Language")
    term.write(b"\033]8;;\033\\")
    term.write(b"\n")

    term.write(b"  File link:   ")
    term.write(b"\033]8;;file:///etc/hostname\033\\")
    term.write(b"/etc/hostname")
    term.write(b"\033]8;;\033\\")
    term.write(b"\n\n")

    # Link with parameters
    subheading(term, "Link with params")
    term.write(b"  ")
    term.write(b"\033]8;params=color:blue;underline:1;https://example.com\033\\")
    term.write(b"Styled link (params)")
    term.write(b"\033]8;;\033\\")
    term.write(b"\n")

    # Multiple links on one line
    subheading(term, "Multiple links")
    term.write(b"  ")
    term.write(b"\033]8;;https://a.com\033\\Link A\033]8;;\033\\")
    term.write(b" | ")
    term.write(b"\033]8;;https://b.com\033\\Link B\033]8;;\033\\")
    term.write(b" | ")
    term.write(b"\033]8;;https://c.com\033\\Link C\033]8;;\033\\")
    term.write(b"\n\n")

    info(term, "Hover and click the links with Ctrl/Cmd+click.")
    info(term, "OSC 8 hyperlinks are supported in: kitty, WezTerm, iTerm2,")
    info(term, "Ghostty, Windows Terminal, VSCode terminal.")

    return prompt_visual(term, "Links clickable? Underlined and styled correctly?")


def _test_osc_colors(term: Terminal) -> TestResultOrStatus:
    """Test OSC colour manipulation: palette (4), fg (10), bg (11), cursor (12)."""
    heading(term, "Dynamic Colour Control (OSC 4/10/11/12)")

    info(term, "These sequences may modify your terminal colour scheme temporarily.")

    # Query current colours
    subheading(term, "Colour Queries")
    term.write(b"  Querying colour 1 (red):  \033]4;1;?\033\\\n")
    term.write(b"  Querying colour 4 (blue): \033]4;4;?\033\\\n")
    term.write(b"  Querying cursor colour:   \033]12;?\033\\\n")
    term.flush()
    time.sleep(0.3)

    # Temporarily change cursor colour
    subheading(term, "Temporary Cursor Colour Change")
    info(term, "Changing cursor colour to bright green...")
    # Save current cursor colour, set to green
    term.write(b"\033]12;#00ff00\033\\")
    time.sleep(1.0)
    term.write(b"  (Cursor should be green now)\n")

    # Restore cursor colour
    term.write(b"\033]12;?\033\\")  # Could restore, but we just query
    term.flush()

    # Reset cursor to default
    term.write(b"\033]112\033\\")  # Reset cursor colour

    info(term, "Note: OSC colour changes affect the current terminal session.")
    info(term, "Some terminals persist these across sessions.")

    return prompt_visual(term, "Colour queries responded? Cursor colour changed?")


def _test_osc_clipboard(term: Terminal) -> TestResultOrStatus:
    """Test OSC 52 clipboard access."""
    heading(term, "OSC 52 Clipboard")

    info(term, "OSC 52 allows the terminal to read/write the system clipboard.")
    info(
        term, "Most modern terminals support it; some require a security confirmation."
    )

    # Write to clipboard
    test_text = "Hello from zenterm-test! 🚀"
    import base64

    encoded = base64.b64encode(test_text.encode()).decode()
    term.write(f"\033]52;c;{encoded}\033\\".encode())
    info(term, f'Copied to clipboard: "{test_text}"')

    # Read from clipboard (query)
    term.write(b"\033]52;c;?\033\\")
    info(term, "Clipboard query sent. Response should appear above.")
    term.flush()

    return prompt_visual(
        term,
        "Text copied to clipboard? Paste elsewhere to verify.\n"
        "  Clipboard query responded?",
    )


def _test_osc_notifications(term: Terminal) -> TestResultOrStatus:
    """Test notification sequences: OSC 9 (iTerm2) and OSC 777 (wezterm)."""
    heading(term, "Terminal Notifications (OSC 9 / 777)")

    info(term, "Sending test notifications...")

    # iTerm2 notification (OSC 9)
    term.write(b"\033]9;Hello from zenterm-test! This is a test notification.\033\\")
    info(term, "Sent: iTerm2 notification (OSC 9)")

    # wezterm notification (OSC 777)
    term.write(
        b"\033]777;notify;zenterm-test;This is a wezterm test notification\033\\"
    )
    info(term, "Sent: wezterm notification (OSC 777)")

    # Terminal.app / others (OSC 9 with title)
    term.write(b"\033]9;zenterm-test\x1bNotification body text\033\\")
    info(term, "Sent: generic notification")
    term.flush()

    time.sleep(1.0)

    return prompt_visual(term, "Notifications appeared? (Check notification centre)")


def _test_osc_semantic_prompt(term: Terminal) -> TestResultOrStatus:
    """Test FinalTerm semantic prompt markers (OSC 133)."""
    heading(term, "Semantic Prompt Markers (OSC 133)")

    info(term, "OSC 133 (FinalTerm) markers let the terminal understand")
    info(term, "where prompts, commands, and outputs begin/end.")

    term.write(b"\n")
    term.write(b"  Prompt start:  ")
    term.write(b"\033]133;A\033\\$ \033]133;B\033\\")
    term.write(b"\n")
    term.write(b"  Command start: ")
    term.write(b"\033]133;C\033\\")
    term.write(b"ls -la")
    term.write(b"\n")
    term.write(b"  Output start:  ")
    term.write(b"\033]133;D\033\\")
    term.write(b"total 42\n")
    term.write(b"  Output end:    ")
    term.write(b"\033]133;B\033\\")
    term.write(b"\n")

    info(term, "Enabled in: WezTerm, kitty (partial), iTerm2 (ghostty?).")
    info(term, "Supports scroll-to-prompt and prompt-relative cursor movement.")

    return prompt_visual(term, "Semantic prompt markers render (no visible garbage)?")


def register_osc_tests():
    register(
        TestCase(
            "osc-title",
            "osc",
            "Window Title (OSC 0/2)",
            "Set and verify window/tab title",
            _test_osc_title,
            interactive=True,
        )
    )
    register(
        TestCase(
            "osc-hyperlinks",
            "osc",
            "OSC 8 Hyperlinks",
            "Clickable links, params, multiple links per line",
            _test_osc8_hyperlinks,
            interactive=True,
        )
    )
    register(
        TestCase(
            "osc-colors",
            "osc",
            "Dynamic Colours (OSC 4/10/11/12)",
            "Colour queries, cursor colour change",
            _test_osc_colors,
        )
    )
    register(
        TestCase(
            "osc-clipboard",
            "osc",
            "OSC 52 Clipboard",
            "Read/write system clipboard",
            _test_osc_clipboard,
        )
    )
    register(
        TestCase(
            "osc-notifications",
            "osc",
            "Notifications (OSC 9/777)",
            "iTerm2 and wezterm notifications",
            _test_osc_notifications,
            interactive=True,
        )
    )
    register(
        TestCase(
            "osc-semantic-prompt",
            "osc",
            "Semantic Prompt (OSC 133)",
            "FinalTerm prompt markers",
            _test_osc_semantic_prompt,
        )
    )
