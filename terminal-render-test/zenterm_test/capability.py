"""Terminal capability detection.

Probes the terminal for supported features by sending query escape
sequences (DA1, DA2, DECRQM, DSR) and inspecting the environment.
"""

import os
import re
from dataclasses import dataclass, field
from typing import Set

from .terminal import Terminal


@dataclass
class Capabilities:
    """Detected capabilities of the terminal under test."""

    # ── Identity ──────────────────────────────────────────────────────
    term_env: str = ""
    colorterm_env: str = ""
    terminal_name: str = ""  # from DA2 or CONSOLE_NAME

    # ── Colors ────────────────────────────────────────────────────────
    color_depth: int = 8  # 8, 256, or 16777216 (24-bit)
    has_truecolor: bool = False
    has_256color: bool = False
    has_16color: bool = True

    # ── SGR ────────────────────────────────────────────────────────────
    has_italic: bool = False
    has_blink: bool = False
    has_strikethrough: bool = False
    has_overline: bool = False
    has_underline_style: bool = False  # 4:1, 4:2, 4:3, 4:4, 4:5
    has_underline_color: bool = False  # SGR 58

    # ── Protocols ──────────────────────────────────────────────────────
    has_sixel: bool = False
    has_kitty_graphics: bool = False
    has_iterm2_images: bool = False
    has_kitty_keyboard: bool = False
    has_sync_output: bool = False  # DEC 2026
    has_bracketed_paste: bool = False
    has_focus_events: bool = False
    has_sgr_mouse: bool = False
    has_sgr_pixels_mouse: bool = False

    # ── OSC ────────────────────────────────────────────────────────────
    has_osc8_hyperlinks: bool = False
    has_osc52_clipboard: bool = False
    has_notifications: bool = False

    # ── Unicode ────────────────────────────────────────────────────────
    has_cjk: bool = False
    has_emoji: bool = False
    has_zwj_emoji: bool = False
    has_bidi: bool = False

    # ── DA1 decoded ───────────────────────────────────────────────────
    da1_codes: Set[int] = field(default_factory=set)
    da2_codes: str = ""


def detect_capabilities(term: Terminal) -> Capabilities:
    """Probe the terminal and return a Capabilities struct."""
    caps = Capabilities()

    # ── Environment ────────────────────────────────────────────────────
    caps.term_env = os.environ.get("TERM", "")
    caps.colorterm_env = os.environ.get("COLORTERM", "")

    # ── DA1 / DA2 ─────────────────────────────────────────────────────
    da1 = term.query_da1(timeout=1.0)
    da2 = term.query_da2(timeout=1.0)

    if da1:
        caps.da1_codes = set(re.findall(r"\d+", da1.decode("ascii", errors="replace")))
        # DA1 code 4 => Sixel support
        if "4" in caps.da1_codes:
            caps.has_sixel = True

    if da2:
        raw = da2.decode("ascii", errors="replace")
        caps.da2_codes = raw.strip()

    # ── Truecolor detection ────────────────────────────────────────────
    if caps.colorterm_env in ("truecolor", "24bit"):
        caps.has_truecolor = True
        caps.color_depth = 16777216
        caps.has_256color = True
    else:
        # Fallback: try a DECRQM on the 24-bit colour extension
        resp = term.query_decrm(1071, timeout=0.5)
        if resp and b"1" in resp:
            caps.has_truecolor = True
            caps.color_depth = 16777216
            caps.has_256color = True
        else:
            # Assume 256 colour from terminfo
            try:
                n = int(os.environ.get("COLORS", "8") or "8")
                if n >= 16777216:
                    caps.has_truecolor = True
                    caps.color_depth = 16777216
                    caps.has_256color = True
                elif n >= 256:
                    caps.has_256color = True
                    caps.color_depth = 256
            except ValueError:
                pass

    # ── Kitty graphics protocol ────────────────────────────────────────
    try:
        term.enter_raw_mode()
        term.write(b"\033_Ga=q\033\\")
        resp = term.read_until(b"\033\\", timeout=0.8)
        if resp and b"OK" in resp:
            caps.has_kitty_graphics = True
    finally:
        term.exit_raw_mode()

    # ── Synchronized output ────────────────────────────────────────────
    resp = term.query_decrm(2026, timeout=0.5)
    caps.has_sync_output = resp is not None

    # ── Bracketed paste ────────────────────────────────────────────────
    resp = term.query_decrm(2004, timeout=0.5)
    caps.has_bracketed_paste = resp is not None

    # ── Focus events ───────────────────────────────────────────────────
    resp = term.query_decrm(1004, timeout=0.5)
    caps.has_focus_events = resp is not None

    # ── SGR mouse ──────────────────────────────────────────────────────
    resp = term.query_decrm(1006, timeout=0.5)
    caps.has_sgr_mouse = resp is not None

    # ── SGR pixels mouse ───────────────────────────────────────────────
    resp = term.query_decrm(1016, timeout=0.5)
    caps.has_sgr_pixels_mouse = resp is not None

    # ── Kitty keyboard protocol ────────────────────────────────────────
    # Progressive enhancement flag 1: disambiguate escape codes
    try:
        term.enter_raw_mode()
        term.write(b"\033[=1u")
        resp = term.read_until(b"u", timeout=0.5)
        # If terminal responds with CSI ? u it's a pushback; implement if needed
        caps.has_kitty_keyboard = resp is not None
    finally:
        term.exit_raw_mode()

    # ── OSC 8 hyperlinks ───────────────────────────────────────────────
    try:
        term.enter_raw_mode()
        term.write(b"\033]8;;test\033\\linked\033]8;;\033\\")
        # We can't auto-verify this, but we can check if the terminal
        # supports it by trying a DSR query around it
        caps.has_osc8_hyperlinks = True  # Assume, user will confirm visually
    finally:
        term.exit_raw_mode()

    return caps


def format_capabilities(caps: Capabilities) -> str:
    """Return a human-readable summary of detected capabilities."""
    lines = []
    lines.append(f"  TERM env:          {caps.term_env or '(not set)'}")
    lines.append(f"  COLORTERM env:     {caps.colorterm_env or '(not set)'}")
    lines.append(
        f"  DA1 codes:         {', '.join(str(x) for x in sorted(caps.da1_codes)) or '(none)'}"
    )

    color_str = f"{caps.color_depth} colors"
    if caps.has_truecolor:
        color_str += " (truecolor)"
    elif caps.has_256color:
        color_str += " (256-color)"
    lines.append(f"  Color depth:       {color_str}")

    flags = []
    if caps.has_sixel:
        flags.append("Sixel")
    if caps.has_kitty_graphics:
        flags.append("Kitty Graphics")
    if caps.has_kitty_keyboard:
        flags.append("Kitty Keyboard")
    if caps.has_sync_output:
        flags.append("Sync Output")
    if caps.has_bracketed_paste:
        flags.append("Bracketed Paste")
    if caps.has_focus_events:
        flags.append("Focus Events")
    if caps.has_sgr_mouse:
        flags.append("SGR Mouse")
    if caps.has_sgr_pixels_mouse:
        flags.append("SGR Pixels Mouse")
    if caps.has_osc8_hyperlinks:
        flags.append("OSC 8 Hyperlinks")
    lines.append(f"  Protocols:         {', '.join(flags) or '(none detected)'}")

    return "\n".join(lines)
