"""Programmatic test pattern generators for graphics protocol tests.

Produces raw RGBA pixel data that can be transmitted via Kitty graphics
protocol (f=32) without any external image files.
"""

import struct
import zlib


def make_test_card(width: int = 64, height: int = 64) -> bytes:
    """Return raw RGBA bytes of a self-describing test card.

    Layout (rows):
      0-7   8 colour bars: R, G, B, Y, C, M, White, Black
      8-15  Grayscale ramp (smooth black→white)
     16-23  Red gradient (dark→bright)
     24-31  Green gradient
     32-39  Blue gradient
     40-47  Checkerboard (black/white)
     48-55  Thin grid lines every 4 px on dark bg
     56-63  Border frame + crosshairs
    """
    pixels = bytearray()

    for y in range(height):
        for x in range(width):
            r = g = b = 0

            if y < 8:
                # ── 8 colour bars ──────────────────────────────────────
                bar = (x * 8) // width
                pal = [
                    (255, 0, 0),  # red
                    (0, 255, 0),  # green
                    (0, 0, 255),  # blue
                    (255, 255, 0),  # yellow
                    (0, 255, 255),  # cyan
                    (255, 0, 255),  # magenta
                    (255, 255, 255),  # white
                    (0, 0, 0),  # black
                ]
                r, g, b = pal[bar]

            elif y < 16:
                # ── Grayscale ramp ─────────────────────────────────────
                v = int(x / width * 255)
                r = g = b = v

            elif y < 24:
                # ── Red gradient ────────────────────────────────────────
                r = int(x / width * 255)
                g = b = 0

            elif y < 32:
                # ── Green gradient ──────────────────────────────────────
                g = int(x / width * 255)
                r = b = 0

            elif y < 40:
                # ── Blue gradient ───────────────────────────────────────
                b = int(x / width * 255)
                r = g = 0

            elif y < 48:
                # ── Checkerboard ────────────────────────────────────────
                cell = 8
                ix = x // cell
                iy = y // cell
                v = 255 if (ix + iy) % 2 == 0 else 32
                r = g = b = v

            elif y < 56:
                # ── Grid lines ──────────────────────────────────────────
                r = g = b = 16  # dark background
                spacing = 4
                if x % spacing == 0 or (y - 48) % spacing == 0:
                    r = g = b = 200  # grid line

            else:
                # ── Border frame + crosshairs ──────────────────────────
                frame = 2  # border width
                cx = width // 2
                cy = (height + 56) // 2  # centre of this band
                r = g = b = 24  # background

                # Outer frame
                if x < frame or x >= width - frame or (y >= height - frame):
                    r, g, b = 200, 200, 50  # yellow border

                # Crosshair vertical
                if abs(x - cx) <= 1 and y >= 56:
                    r, g, b = 255, 100, 100

                # Crosshair horizontal
                if abs(y - cy) <= 1:
                    r, g, b = 255, 100, 100

                # Corner marks
                if x < 8 and (y == 56 or y == height - 1):
                    r, g, b = 60, 200, 60
                if x >= width - 8 and (y == 56 or y == height - 1):
                    r, g, b = 60, 200, 60

            pixels.extend([r, g, b, 255])

    return bytes(pixels)


def make_small_card(width: int = 24, height: int = 24) -> bytes:
    """Minimal test card — single colour bars + centre mark."""
    pixels = bytearray()
    colours = [
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
        (255, 255, 0),
        (255, 0, 255),
        (0, 255, 255),
    ]

    for y in range(height):
        for x in range(width):
            bar = (x * len(colours)) // width
            if bar < len(colours):
                r, g, b = colours[bar]
            else:
                r = g = b = 0
            # centre cross
            cx, cy = width // 2, height // 2
            if abs(x - cx) <= 1 or abs(y - cy) <= 1:
                r, g, b = 255, 255, 255
            # border
            if x == 0 or x == width - 1 or y == 0 or y == height - 1:
                r, g, b = 255, 255, 0
            pixels.extend([r, g, b, 255])

    return bytes(pixels)


def encode_kitty_rgba(pixels: bytes) -> str:
    """Encode raw RGBA pixels as base64 for Kitty (f=32)."""
    import base64

    return base64.b64encode(pixels).decode("ascii")


def encode_kitty_png(pixels: bytes, width: int, height: int) -> str:
    """Build a minimal PNG from raw RGBA and return base64.

    Uses stdlib zlib + struct — no PIL dependency.
    """
    import base64

    def _chunk(chunk_type: bytes, data: bytes) -> bytes:
        c = chunk_type + data
        crc = struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
        return struct.pack(">I", len(data)) + c + crc

    signature = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)

    # Convert RGBA to raw for PNG (filter byte 0 per row)
    raw = b""
    for y in range(height):
        raw += b"\x00"  # filter: none
        row_start = y * width * 4
        raw += pixels[row_start : row_start + width * 4]

    idat = zlib.compress(raw)

    png = signature
    png += _chunk(b"IHDR", ihdr)
    png += _chunk(b"IDAT", idat)
    png += _chunk(b"IEND", b"")

    return base64.b64encode(png).decode("ascii")
