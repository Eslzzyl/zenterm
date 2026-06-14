"""Graphics protocol tests: Kitty graphics protocol and Sixel.

Tests image display, placement, chunked transfer, z-index compositing,
delete operations, and protocol queries.

All test images are generated programmatically — no external files needed.
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
from ..fixtures import (
    make_test_card,
    make_small_card,
    encode_kitty_rgba,
    encode_kitty_png,
)

# Image size for most tests
CARD_W, CARD_H = 80, 80
SMALL_W, SMALL_H = 24, 24


def _kitty_send(term: Terminal, params: str, payload: str = "") -> None:
    """Send a Kitty graphics protocol escape sequence."""
    if payload:
        term.write(f"\033_G{params};{payload}\033\\".encode("ascii"))
    else:
        term.write(f"\033_G{params}\033\\".encode("ascii"))
    term.flush()


def _test_kitty_query(term: Terminal) -> TestResultOrStatus:
    """Query Kitty graphics protocol support via a=q."""
    heading(term, "Kitty Graphics: Protocol Query (a=q)")

    info(term, "Sending Kitty graphics protocol query...")
    _kitty_send(term, "a=q")
    term.flush()

    try:
        term.enter_raw_mode()
        resp = term.read_until(b"\033\\", timeout=1.0)
        if resp:
            decoded = resp.decode("ascii", errors="replace")
            info(term, f"Query response: {decoded.strip()}")
            if "OK" in decoded:
                return TestResult(
                    test_id="kitty-query",
                    category="graphics",
                    name="Kitty Query",
                    status=TestStatus.PASS,
                    message="Kitty graphics supported",
                )
        else:
            info(term, "No response to query (timeout).")
    finally:
        term.exit_raw_mode()

    return TestResult(
        test_id="kitty-query",
        category="graphics",
        name="Kitty Query",
        status=TestStatus.SKIP,
        message="No response — terminal may not support Kitty graphics",
    )


def _test_kitty_basic(term: Terminal) -> TestResultOrStatus:
    """Basic Kitty graphics display via a=T (transmit+display)."""
    heading(term, "Kitty Graphics: Basic Display (a=T)")

    pixels = make_test_card(CARD_W, CARD_H)
    b64_rgba = encode_kitty_rgba(pixels)

    info(term, "Displaying 80x80 test card via a=T (RGBA, f=32)")
    info(term, "Pattern: colour bars, grayscale, gradients, grid, crosshairs")

    _kitty_send(term, f"a=T,i=1,f=32,q=2,s={CARD_W},v={CARD_H}", b64_rgba)
    time.sleep(0.3)

    info(term, "Image ID 1 should now be displayed inline.")
    return prompt_visual(term, "Kitty basic image display works?")


def _test_kitty_positioned(term: Terminal) -> TestResultOrStatus:
    """Display images at specific row/col positions."""
    heading(term, "Kitty Graphics: Positioned Display")

    pixels = make_small_card(SMALL_W, SMALL_H)
    b64 = encode_kitty_rgba(pixels)

    # Delete any previous images
    _kitty_send(term, "a=d,d=A")
    time.sleep(0.1)

    info(term, "Image at row=2, col=1")
    _kitty_send(term, f"a=T,i=10,f=32,q=2,s={SMALL_W},v={SMALL_H},C=1,r=2", b64)
    time.sleep(0.2)

    info(term, "Image at row=2, col=20")
    _kitty_send(term, f"a=T,i=11,f=32,q=2,s={SMALL_W},v={SMALL_H},C=20,r=2", b64)
    time.sleep(0.2)

    info(term, "Image at row=10, col=1")
    _kitty_send(term, f"a=T,i=12,f=32,q=2,s={SMALL_W},v={SMALL_H},C=1,r=10", b64)
    time.sleep(0.3)

    info(term, "Three small cards should appear at different positions.")
    return prompt_visual(term, "Images positioned at correct row/col?")


def _test_kitty_transmit_display(term: Terminal) -> TestResultOrStatus:
    """Two-step: transmit (a=t) then display (a=p)."""
    heading(term, "Kitty Graphics: Transmit then Display")

    pixels = make_test_card(CARD_W, CARD_H)
    b64 = encode_kitty_rgba(pixels)

    _kitty_send(term, "a=d,d=A")

    # Transmit only (a=t)
    info(term, "Transmitting image ID 20 (not displayed yet)...")
    _kitty_send(term, f"a=t,i=20,f=32,q=2,s={CARD_W},v={CARD_H}", b64)
    time.sleep(0.3)

    info(term, "Displaying image ID 20 now (a=p)...")
    _kitty_send(term, "a=p,i=20,q=2")
    time.sleep(0.3)

    # Display same image at another position
    info(term, "Displaying same image at col=30, row=5...")
    _kitty_send(term, "a=p,i=20,C=30,r=5,q=2")
    time.sleep(0.3)

    info(term, "Image should appear at two locations.")
    return prompt_visual(term, "Transmit-then-display works correctly?")


def _test_kitty_chunked(term: Terminal) -> TestResultOrStatus:
    """Chunked transfer via m=1 / m=0."""
    heading(term, "Kitty Graphics: Chunked Transfer")

    # Larger card so base64 exceeds one chunk
    pixels = make_test_card(160, 80)
    b64 = encode_kitty_rgba(pixels)

    _kitty_send(term, "a=d,d=A")

    # Split into chunks of ~1000 base64 chars
    chunk_size = 1000
    chunks = [b64[i : i + chunk_size] for i in range(0, len(b64), chunk_size)]
    total = len(chunks)

    info(term, f"Splitting {len(b64)} base64 chars into {total} chunks...")

    for idx, chunk in enumerate(chunks):
        is_last = idx == total - 1
        m = "0" if is_last else "1"
        params = f"a=T,i=30,f=32,q=2,m={m},s=160,v=80"
        _kitty_send(term, params, chunk)

    time.sleep(0.5)
    info(term, f"Image transmitted in {total} chunks.")
    return prompt_visual(term, "Chunked transfer rendered correctly?")


def _test_kitty_zindex(term: Terminal) -> TestResultOrStatus:
    """Z-index compositing — overlapping images."""
    heading(term, "Kitty Graphics: Z-Index Compositing")

    pixels = make_small_card(SMALL_W, SMALL_H)
    b64 = encode_kitty_rgba(pixels)

    _kitty_send(term, "a=d,d=A")

    info(term, "Image A (z=0) at (1,1), Image B (z=1) same position")
    info(term, "Image B should appear on top of Image A")

    _kitty_send(term, f"a=T,i=40,f=32,q=2,z=0,s={SMALL_W},v={SMALL_H}", b64)
    time.sleep(0.3)

    _kitty_send(term, f"a=T,i=41,f=32,q=2,z=1,s={SMALL_W},v={SMALL_H}", b64)
    time.sleep(0.5)

    info(term, "Image 41 (z=1) should appear above image 40 (z=0).")
    return prompt_visual(term, "Z-index works? Image 41 on top?")


def _test_kitty_delete(term: Terminal) -> TestResultOrStatus:
    """Delete specific image via d=i."""
    heading(term, "Kitty Graphics: Delete Image (d=i)")

    pixels = make_small_card(SMALL_W, SMALL_H)
    b64 = encode_kitty_rgba(pixels)
    params = f"f=32,q=2,s={SMALL_W},v={SMALL_H}"

    _kitty_send(term, "a=d,d=A")

    info(term, "Displaying images ID 50, 51, 52...")
    _kitty_send(term, f"a=T,i=50,{params},C=1,r=2", b64)
    time.sleep(0.2)
    _kitty_send(term, f"a=T,i=51,{params},C=1,r=8", b64)
    time.sleep(0.2)
    _kitty_send(term, f"a=T,i=52,{params},C=1,r=14", b64)
    time.sleep(0.3)

    info(term, "Deleting image ID 50 (d=i=50)...")
    _kitty_send(term, "a=d,d=i,i=50,q=2")
    time.sleep(0.3)

    info(term, "Image 50 should disappear. Images 51, 52 remain.")
    return prompt_visual(term, "Specific image deleted (50 gone, 51/52 remain)?")


def _test_kitty_delete_all(term: Terminal) -> TestResultOrStatus:
    """Delete all images via d=A."""
    heading(term, "Kitty Graphics: Delete All (d=A)")

    pixels = make_small_card(SMALL_W, SMALL_H)
    b64 = encode_kitty_rgba(pixels)
    params = f"f=32,q=2,s={SMALL_W},v={SMALL_H}"

    info(term, "Displaying images...")
    _kitty_send(term, f"a=T,i=60,{params},C=1,r=2", b64)
    time.sleep(0.2)
    _kitty_send(term, f"a=T,i=61,{params},C=20,r=5", b64)
    time.sleep(0.3)

    info(term, "Deleting ALL images (d=A)...")
    _kitty_send(term, "a=d,d=A,q=2")
    time.sleep(0.3)

    info(term, "All images should now be gone.")
    return prompt_visual(term, "All images deleted? Screen clear of graphics?")


def _test_kitty_png_format(term: Terminal) -> TestResultOrStatus:
    """Test PNG format transmission (f=100) via programmatic PNG."""
    heading(term, "Kitty Graphics: PNG Format (f=100)")

    pixels = make_test_card(CARD_W, CARD_H)
    b64_png = encode_kitty_png(pixels, CARD_W, CARD_H)

    info(term, "Displaying test card as in-memory PNG (f=100)")
    info(term, "Should look identical to the RGBA version above.")

    _kitty_send(term, f"a=T,i=5,f=100,q=2,s={CARD_W},v={CARD_H}", b64_png)
    time.sleep(0.4)

    return prompt_visual(term, "PNG format (f=100) renders correctly?")


def _test_kitty_clear_screen(term: Terminal) -> TestResultOrStatus:
    """Test image behaviour after screen clear."""
    heading(term, "Kitty Graphics: Screen Clear Interaction")

    pixels = make_small_card(SMALL_W, SMALL_H)
    b64 = encode_kitty_rgba(pixels)

    _kitty_send(term, "a=d,d=A")

    info(term, "Displaying image...")
    _kitty_send(term, f"a=T,i=70,f=32,q=2,s={SMALL_W},v={SMALL_H}", b64)
    time.sleep(0.3)

    info(term, "Clearing screen with \\e[2J...")
    term.write(b"\033[2J")
    time.sleep(0.3)

    info(term, "Does the image survive screen clear?")
    info(term, "Kitty protocol: images are independent of text cells.")
    info(term, "Some terminals clear images with ED, some do not.")

    return prompt_visual(
        term,
        "Image visible after screen clear? (Kitty: yes, others: varies)",
    )


def register_graphics_tests():
    register(
        TestCase(
            "kitty-query",
            "graphics",
            "Kitty Graphics Query (a=q)",
            "Query Kitty graphics protocol support",
            _test_kitty_query,
            auto_verify=True,
        )
    )
    register(
        TestCase(
            "kitty-basic",
            "graphics",
            "Kitty Basic Display (a=T)",
            "Transmit+display 80x80 programmatic test card",
            _test_kitty_basic,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-positioned",
            "graphics",
            "Kitty Positioned Display",
            "Display at specific row/col via C/r params",
            _test_kitty_positioned,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-tx-display",
            "graphics",
            "Kitty Transmit+Display (a=t+a=p)",
            "Two-step: transmit then display",
            _test_kitty_transmit_display,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-chunked",
            "graphics",
            "Kitty Chunked Transfer",
            "Split base64 into multiple chunks (m=1/m=0)",
            _test_kitty_chunked,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-png",
            "graphics",
            "Kitty PNG Format (f=100)",
            "In-memory PNG generation + transmission",
            _test_kitty_png_format,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-zindex",
            "graphics",
            "Kitty Z-Index",
            "Overlapping images with different z-index",
            _test_kitty_zindex,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-delete",
            "graphics",
            "Kitty Delete Image (d=i)",
            "Delete specific image by ID",
            _test_kitty_delete,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-delete-all",
            "graphics",
            "Kitty Delete All (d=A)",
            "Delete all placed images",
            _test_kitty_delete_all,
            required_caps=["kitty_graphics"],
        )
    )
    register(
        TestCase(
            "kitty-clear",
            "graphics",
            "Kitty Screen Clear Interaction",
            "Does image survive \\e[2J?",
            _test_kitty_clear_screen,
            required_caps=["kitty_graphics"],
        )
    )
