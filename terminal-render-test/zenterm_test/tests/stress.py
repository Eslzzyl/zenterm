"""Stress and performance tests.

Tests the terminal's ability to handle high-throughput output, rapid
escape sequences, scrolling performance, window resize behaviour,
and rendering stability.
"""

import time

from ..terminal import Terminal
from ..reporter import (
    TestResultOrStatus,
    TestCase,
    heading,
    info,
    prompt_continue,
    prompt_visual,
    register,
)


def _test_scroll_perf(term: Terminal) -> TestResultOrStatus:
    """Test scrolling throughput with large output."""
    heading(term, "Scrolling Performance")

    info(term, "Generating 500 lines of output — evaluating scroll smoothness.")

    t0 = time.perf_counter()
    # Burst-write a large block
    buf = bytearray()
    for i in range(500):
        buf += f"{i:4d}: The quick brown fox jumps over the lazy dog. ".encode()
        buf += b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\n"
    term.write(bytes(buf))
    term.flush()
    elapsed = time.perf_counter() - t0

    info(
        term,
        f"Wrote 500 lines in {elapsed * 1000:.0f}ms ({500 / elapsed:.0f} lines/sec)",
    )

    # Reset
    term.reset()
    time.sleep(0.3)

    return prompt_visual(term, f"Scrolling was smooth? ({500 / elapsed:.0f} lps)")


def _test_color_barrage(term: Terminal) -> TestResultOrStatus:
    """Rapid colour changes across full screen — GPU/texture stress."""
    heading(term, "Colour Barrage (Full Screen Update)")

    rows, cols = term.get_size()
    info(term, f"Terminal size: {rows}x{cols}")

    t0 = time.perf_counter()
    # Fill entire screen with coloured blocks
    for row in range(min(rows, 50)):
        for col in range(min(cols, 80)):
            r = (row * 5) % 256
            g = (col * 5) % 256
            b = ((row + col) * 3) % 256
            term.write(
                f"\033[{row + 1};{col + 1}H\033[48;2;{r};{g};{b}m \033[0m".encode()
            )
    term.flush()
    elapsed = time.perf_counter() - t0

    info(term, f"Full screen colour update in {elapsed * 1000:.0f}ms")
    time.sleep(0.5)

    term.reset()
    return prompt_visual(term, "Colour barrage rendered without tearing/lag?")


def _test_rapid_erase(term: Terminal) -> TestResultOrStatus:
    """Rapid clear/repaint cycles — screen buffer stress test."""
    heading(term, "Rapid Erase / Repaint")

    info(term, "Running 20 rapid fill-clear cycles...")
    t0 = time.perf_counter()

    for cycle in range(20):
        term.write(b"\033[2J\033[H")  # Clear screen
        for i in range(80):
            r = (i * 12 + cycle * 7) % 256
            term.write(f"\033[48;2;{r};{64};{255 - r}m \033[0m".encode())
        term.flush()

    elapsed = time.perf_counter() - t0
    info(term, f"20 cycles in {elapsed * 1000:.0f}ms ({20 / elapsed:.1f} cycles/sec)")

    term.reset()
    return prompt_visual(term, "No flicker or corruption during rapid erase cycles?")


def _test_unicode_mixed_stress(term: Terminal) -> TestResultOrStatus:
    """Render mixed ASCII/CJK/emoji at high speed — shaping pipeline stress."""
    heading(term, "Mixed Unicode Stress Test")

    info(term, "Generating mixed unicode content rapidly...")

    t0 = time.perf_counter()
    buf = bytearray()
    for i in range(100):
        line = (
            f"{i:3d}: The quick brown fox 敏捷的棕色狐狸 "
            "được ưa chuộng 점프 跳过了 the lazy dog "
            "🐶🦊🐱🐰🐼🐨🦁🐮🐷🐸🐵🐔🐧🐦🐤🐣🐥 "
            "ÀÁÂÃÄÅÆ ÇÈÉÊËÌÍÎÏ ÐÑÒÓÔÕÖ ØÙÚÛÜÝÞß\n"
        )
        buf += line.encode("utf-8")
    term.write(bytes(buf))
    term.flush()
    elapsed = time.perf_counter() - t0

    info(term, f"100 mixed-unicode lines in {elapsed * 1000:.0f}ms")
    time.sleep(0.3)
    term.reset()

    return prompt_visual(term, "Mixed unicode rendered without garbled characters?")


def _test_resize_stability(term: Terminal) -> TestResultOrStatus:
    """Test terminal rendering after resize events."""
    heading(term, "Window Resize Stability")

    info(term, "This test checks that rendering remains stable after resize.")
    info(term, "Content should reflow correctly.")

    # Fill screen with a grid pattern
    rows, cols = term.get_size()
    buf = bytearray()

    # Top ruler
    buf += b"  "
    for c in range(min(cols - 2, 78)):
        buf += str(c % 10).encode()
    buf += b"\n"

    # Fill rows with alternating pattern
    for r in range(min(rows - 3, 25)):
        buf += f"{r:2d} ".encode()
        for c in range(min(cols - 3, 77)):
            char = chr(ord("A") + (r + c) % 26)
            buf += char.encode()
        buf += b"\n"

    term.write(bytes(buf))
    term.flush()

    info(term, "Now manually resize the terminal window.")
    info(term, "After resize, the grid should be redrawn correctly.")
    info(term, "Press Enter when done resizing...")
    prompt_continue(term)

    term.reset()
    return prompt_visual(term, "Content reflowed correctly after resize?")


def register_stress_tests():
    register(
        TestCase(
            "stress-scroll",
            "stress",
            "Scrolling Performance",
            "500 lines of text, measure throughput",
            _test_scroll_perf,
            interactive=True,
        )
    )
    register(
        TestCase(
            "stress-color-barrage",
            "stress",
            "Colour Barrage",
            "Full-screen colour update stress test",
            _test_color_barrage,
            interactive=True,
        )
    )
    register(
        TestCase(
            "stress-rapid-erase",
            "stress",
            "Rapid Erase/Repaint",
            "20 fill-clear cycles per second",
            _test_rapid_erase,
            interactive=True,
        )
    )
    register(
        TestCase(
            "stress-unicode-mixed",
            "stress",
            "Mixed Unicode Stress",
            "100 lines of mixed ASCII/CJK/emoji",
            _test_unicode_mixed_stress,
            interactive=True,
        )
    )
    register(
        TestCase(
            "stress-resize",
            "stress",
            "Resize Stability",
            "Rendering correctness after window resize",
            _test_resize_stability,
            interactive=True,
        )
    )
