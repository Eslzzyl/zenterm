"""Color rendering tests.

Tests 16 standard colours, 256-colour palette, 24-bit truecolor,
colour combinations, and colour queries.
"""

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


def _test_colors_16(term: Terminal) -> TestResultOrStatus:
    """16 standard foreground/background colours (SGR 30-37, 90-97, 40-47, 100-107)."""
    heading(term, "16 Standard Colours")

    fg_colors = [
        ("30", "Black"),
        ("31", "Red"),
        ("32", "Green"),
        ("33", "Yellow"),
        ("34", "Blue"),
        ("35", "Magenta"),
        ("36", "Cyan"),
        ("37", "White"),
    ]
    bg_colors = [
        ("40", "Black"),
        ("41", "Red"),
        ("42", "Green"),
        ("43", "Yellow"),
        ("44", "Blue"),
        ("45", "Magenta"),
        ("46", "Cyan"),
        ("47", "White"),
    ]
    fg_bright = [
        ("90", "Black"),
        ("91", "Red"),
        ("92", "Green"),
        ("93", "Yellow"),
        ("94", "Blue"),
        ("95", "Magenta"),
        ("96", "Cyan"),
        ("97", "White"),
    ]
    bg_bright = [
        ("100", "Black"),
        ("101", "Red"),
        ("102", "Green"),
        ("103", "Yellow"),
        ("104", "Blue"),
        ("105", "Magenta"),
        ("106", "Cyan"),
        ("107", "White"),
    ]

    term.write(b"  Foreground (30-37):\n")
    for param, name in fg_colors:
        term.write(f"\033[{param}m {name:8s}\033[0m".encode())
    term.write(b"\n")

    term.write(b"  Foreground bright (90-97):\n")
    for param, name in fg_bright:
        term.write(f"\033[{param}m {name:8s}\033[0m".encode())
    term.write(b"\n\n")

    term.write(b"  Background (40-47) on white foreground:\n")
    term.write(b"\033[97m")
    for param, name in bg_colors:
        term.write(f"\033[{param}m {name:8s}\033[0m\033[97m".encode())
    term.write(b"\033[0m\n")

    term.write(b"  Background bright (100-107) on black foreground:\n")
    term.write(b"\033[30m")
    for param, name in bg_bright:
        term.write(f"\033[{param}m {name:8s}\033[0m\033[30m".encode())
    term.write(b"\033[0m\n\n")

    # Colour combinations with attributes
    subheading(term, "Attribute + Colour Combinations")
    for attr, attr_name in [
        ("1", "Bold"),
        ("2", "Dim"),
        ("3", "Italic"),
        ("7", "Reverse"),
    ]:
        term.write(f"  {attr_name:8s}: ".encode())
        for param, _ in fg_colors:
            term.write(f"\033[{attr};{param}m XXX\033[0m ".encode())
        term.write(b"\n")

    return prompt_visual(term, "16 colours rendered correctly? All attributes visible?")


def _test_colors_256(term: Terminal) -> TestResultOrStatus:
    """256-colour palette: system colours, 6x6x6 cube, greyscale."""
    heading(term, "256-Colour Palette (SGR 38;5;n / 48;5;n)")

    # System colours 0-15
    subheading(term, "System Colours (0-15)")
    for i in range(0, 16, 8):
        for j in range(8):
            n = i + j
            term.write(f"\033[48;5;{n}m  {n:3d}  \033[0m".encode())
        term.write(b"\n")
    term.write(b"\n")

    # 6x6x6 RGB cube (16-231)
    subheading(term, "6×6×6 RGB Cube (16-231)")
    for row in range(36):
        n = 16 + row * 6
        for col in range(6):
            c = n + col
            if c > 231:
                break
            term.write(f"\033[48;5;{c}m  \033[0m".encode())
        row += 1
        if (row + 1) % 6 == 0:
            term.write(b"\n")
    term.write(b"\n\n")

    # Greyscale 232-255
    subheading(term, "Greyscale Ramp (232-255)")
    for i in range(232, 256):
        # Alternate foreground for readability
        fg = "30" if i < 244 else "37"
        term.write(f"\033[{fg};48;5;{i}m  \033[0m".encode())
    term.write(b"\n\n")

    # Foreground variant
    subheading(term, "Foreground Palette")
    out = bytearray()
    for i in range(0, 256, 8):
        for j in range(8):
            n = i + j
            if n < 256:
                out += f"\033[38;5;{n}m###\033[0m ".encode()
        out += b"\n"
    term.write(bytes(out))
    term.flush()

    return prompt_visual(
        term, "256-colour palette displayed correctly? Smooth cube without banding?"
    )


def _test_truecolor(term: Terminal) -> TestResultOrStatus:
    """24-bit truecolor gradients (SGR 38;2;r;g;b / 48;2;r;g;b)."""
    heading(term, "Truecolor (24-bit) Gradients")

    # Single-channel gradients: 72 columns → ~3.5 levels/step (smooth)
    width = 72

    # Red gradient
    subheading(term, "Red Gradient (R 0→255, G=0, B=0)")
    for i in range(width):
        r = int(i / width * 255)
        term.write(f"\033[48;2;{r};0;0m \033[0m".encode())
    term.write(b"\n")

    # Green gradient
    subheading(term, "Green Gradient (R=0, G 0→255, B=0)")
    for i in range(width):
        g = int(i / width * 255)
        term.write(f"\033[48;2;0;{g};0m \033[0m".encode())
    term.write(b"\n")

    # Blue gradient
    subheading(term, "Blue Gradient (R=0, G=0, B 0→255)")
    for i in range(width):
        b = int(i / width * 255)
        term.write(f"\033[48;2;0;0;{b}m \033[0m".encode())
    term.write(b"\n")

    # Full spectrum gradient — 3 rows × 72 cols, covering 120° each
    # Ordered dithering breaks up 8-bit contouring.
    subheading(term, "Spectrum (Hue Wheel, 3 rows)")
    import math
    # 2×2 Bayer threshold matrix
    bayer = [[0, 2], [3, 1]]
    labels = ["0°–120°", "120°–240°", "240°–360°"]
    for row_idx, (label, hue_start) in enumerate(zip(labels, [0, 120, 240])):
        term.write(f"  {label:10s}: ".encode())
        for i in range(width):
            hue = (hue_start + i / width * 120) / 360
            phase = hue * 2 * math.pi
            # Raw float RGB values
            rf = 255 * (math.cos(phase) * 0.5 + 0.5)
            gf = 255 * (math.cos(phase + 2.094) * 0.5 + 0.5)
            bf = 255 * (math.cos(phase + 4.189) * 0.5 + 0.5)
            # Ordered dither: offset by [-0.375, +0.375] based on position
            thr = bayer[row_idx % 2][i % 2] * 0.25 - 0.375
            r = min(255, max(0, round(rf + thr)))
            g = min(255, max(0, round(gf + thr)))
            b = min(255, max(0, round(bf + thr)))
            term.write(f"\033[48;2;{r};{g};{b}m \033[0m".encode())
        term.write(b"\n")
    term.write(b"\n")

    # Foreground gradient
    subheading(term, "Foreground Truecolor")
    for i in range(width):
        r = int(i / width * 255)
        term.write(f"\033[38;2;{r};{255 - r};128m█\033[0m".encode())
    term.write(b"\n\n")

    # Perceptual check: should be SMOOTH with no visible banding
    info(term, "Visible colour banding = terminal downsampling to 256-colour palette.")
    info(term, "Gradients should be smooth from left to right.")

    return prompt_visual(term, "Truecolor gradients smooth without visible banding?")


def _test_color_reset(term: Terminal) -> TestResultOrStatus:
    """Test colour resets SGR 39 (fg) / 49 (bg)."""
    heading(term, "Colour Reset (39 / 49)")

    term.write(b"  \033[31mRed foreground \033[39m(default foreground)\033[0m\n")
    term.write(b"  \033[41mRed background \033[49m(default background)\033[0m\n")
    term.write(b"\n")
    term.write(b"  \033[31;41mRed on red \033[39;49mfully reset\033[0m\n")
    term.flush()

    return prompt_visual(term, "Colour resets (39/49) restore defaults correctly?")


def _test_color_foreground_background(term: Terminal) -> TestResultOrStatus:
    """Test default foreground/background queries (OSC 10 / 11)."""
    heading(term, "Default Colour Queries (OSC 10 / 11)")

    queries = [
        (10, "Default foreground"),
        (11, "Default background"),
        (12, "Cursor colour"),
    ]

    try:
        term.enter_raw_mode()
        for n, label in queries:
            term.write(f"  {label:20s}: ".encode())
            term.write(f"\033]{n};?\033\\".encode())
            term.flush()
            resp = term.read_until(b"\033\\", timeout=1.0)
            if resp:
                decoded = resp.decode("ascii", errors="replace").strip()
                # Extract colour value after "rgb:" — e.g. "rgb:4c4c/4f4f/6969"
                if "rgb:" in decoded:
                    rgb = decoded.split("rgb:")[1].rstrip("\033\\").strip()
                    term.write(f"\033[32m{rgb}\033[0m\n".encode())
                else:
                    term.write(f"\033[33m{decoded}\033[0m\n".encode())
            else:
                term.write("\033[90m(no response — not supported)\033[0m\n".encode())
            term.flush()
    finally:
        term.exit_raw_mode()

    term.write(b"\n")
    return prompt_visual(term, "Colour responses parsed and displayed correctly?")


def register_color_tests():
    register(
        TestCase(
            "colors-16",
            "colors",
            "16 Standard Colours",
            "SGR 30-37, 90-97, 40-47, 100-107 with attributes",
            _test_colors_16,
        )
    )
    register(
        TestCase(
            "colors-256",
            "colors",
            "256-Colour Palette",
            "System colours, 6x6x6 cube, greyscale ramp",
            _test_colors_256,
        )
    )
    register(
        TestCase(
            "colors-truecolor",
            "colors",
            "Truecolor Gradients",
            "24-bit RGB gradients, spectrum, foreground",
            _test_truecolor,
            required_caps=["truecolor"],
        )
    )
    register(
        TestCase(
            "colors-reset",
            "colors",
            "Colour Reset (39/49)",
            "Restore default foreground/background",
            _test_color_reset,
        )
    )
    register(
        TestCase(
            "colors-queries",
            "colors",
            "Default Colour Queries (OSC 10/11)",
            "Query default foreground, background, cursor colours",
            _test_color_foreground_background,
        )
    )
