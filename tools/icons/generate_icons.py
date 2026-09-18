"""Generate Zenterm's platform icon assets from the canonical SVG."""

from __future__ import annotations

from io import BytesIO
from pathlib import Path
from tempfile import TemporaryDirectory

from PIL import Image, ImageChops, ImageDraw
from icnsutil import IcnsFile
from resvg_py import svg_to_bytes


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "assets" / "brand" / "zenterm.svg"
RUNTIME = ROOT / "assets" / "runtime" / "zenterm.png"
WINDOWS = ROOT / "assets" / "windows" / "zenterm.ico"
MACOS = ROOT / "assets" / "macos" / "zenterm.icns"
LINUX = ROOT / "assets" / "linux"
ICON_NAME = "org.eu.eslzzyl.zenterm"

LINUX_SIZES = (16, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

# Separate the colored plate from the transparent canvas, following the same
# silhouette treatment used by WezTerm's Windows icon. The former 2px/6px
# masks left the full-bleed plate looking square and mechanically clipped.
# macOS remains unmasked so AppKit can apply its own Dock/Finder shape.
ICON_CORNER_RADIUS_AT_48 = 10.0
ICON_INSET_AT_48 = 2.0
MASK_SUPERSAMPLE = 8

# Use explicit ICNS keys so 16/32/48 px images are not ambiguous between
# legacy and modern representations.
ICNS_MEDIA = (
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 48),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
    ("ic11", 32),
    ("ic12", 64),
    ("ic13", 256),
    ("ic14", 512),
    ("icsb", 18),
    ("icsB", 36),
    ("sb24", 24),
    ("SB24", 48),
)


def render(size: int, *, rounded: bool = False) -> Image.Image:
    png = svg_to_bytes(
        svg_path=str(SOURCE),
        width=size,
        height=size,
        shape_rendering="geometric_precision",
    )
    image = Image.open(BytesIO(png)).convert("RGBA")

    if rounded:
        # Build the mask at a higher resolution so the small 16px and 24px
        # exports retain a clean, platform-consistent antialiased corner.
        mask_size = size * MASK_SUPERSAMPLE
        mask = Image.new("L", (mask_size, mask_size), 0)
        inset = round(ICON_INSET_AT_48 * size / 48 * MASK_SUPERSAMPLE)
        ImageDraw.Draw(mask).rounded_rectangle(
            (inset, inset, mask_size - inset - 1, mask_size - inset - 1),
            radius=round(
                (ICON_CORNER_RADIUS_AT_48 + ICON_INSET_AT_48)
                * size
                / 48
                * MASK_SUPERSAMPLE
            ),
            fill=255,
        )
        mask = mask.resize((size, size), Image.Resampling.LANCZOS)
        image.putalpha(ImageChops.multiply(image.getchannel("A"), mask))

    return image


def write_png(path: Path, size: int, *, rounded: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    render(size, rounded=rounded).save(path, format="PNG", optimize=True)


def write_ico() -> None:
    WINDOWS.parent.mkdir(parents=True, exist_ok=True)
    render(1024, rounded=True).save(
        WINDOWS,
        format="ICO",
        sizes=[(size, size) for size in ICO_SIZES],
        bitmap_format="png",
    )


def write_icns() -> None:
    MACOS.parent.mkdir(parents=True, exist_ok=True)
    icon = IcnsFile()

    with TemporaryDirectory(prefix="zenterm-icns-") as temp_dir:
        temp = Path(temp_dir)
        for key, size in ICNS_MEDIA:
            image_path = temp / f"{key}.png"
            render(size).save(image_path, format="PNG", optimize=True)
            icon.add_media(key=key, file=str(image_path))

        icon.write(str(MACOS))

    errors = list(IcnsFile.verify(str(MACOS)))
    if errors:
        raise RuntimeError("Invalid generated ICNS: " + "; ".join(errors))


def write_linux_assets() -> None:
    for size in LINUX_SIZES:
        write_png(
            LINUX / "hicolor" / f"{size}x{size}" / "apps" / f"{ICON_NAME}.png",
            size,
            rounded=True,
        )

    desktop = f"""[Desktop Entry]
Type=Application
Name=Zenterm
Exec=zenterm
Icon={ICON_NAME}
Terminal=false
Categories=System;TerminalEmulator;
StartupNotify=true
"""
    LINUX.mkdir(parents=True, exist_ok=True)
    (LINUX / f"{ICON_NAME}.desktop").write_bytes(desktop.encode("utf-8"))


def main() -> None:
    if not SOURCE.is_file():
        raise FileNotFoundError(f"Missing canonical SVG: {SOURCE}")

    write_png(RUNTIME, 1024, rounded=True)
    write_ico()
    write_icns()
    write_linux_assets()

    print(f"Generated icons from {SOURCE.relative_to(ROOT)}")
    print(f"  {RUNTIME.relative_to(ROOT)}")
    print(f"  {WINDOWS.relative_to(ROOT)}")
    print(f"  {MACOS.relative_to(ROOT)}")
    print(f"  {LINUX.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
