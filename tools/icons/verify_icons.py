"""Verify Zenterm's checked-in platform icon assets without generating them."""

from __future__ import annotations

from pathlib import Path

from PIL import Image
from icnsutil import IcnsFile


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "assets" / "brand" / "zenterm.svg"
RUNTIME = ROOT / "assets" / "runtime" / "zenterm.png"
WINDOWS = ROOT / "assets" / "windows" / "zenterm.ico"
MACOS = ROOT / "assets" / "macos" / "zenterm.icns"
LINUX = ROOT / "assets" / "linux"
ICON_NAME = "org.eu.eslzzyl.zenterm"

LINUX_SIZES = (16, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


class VerificationError(RuntimeError):
    """Raised when a checked-in icon asset violates a project invariant."""


def require_file(path: Path) -> None:
    if not path.is_file():
        raise VerificationError(f"missing file: {path.relative_to(ROOT)}")


def verify_rounded_alpha(image: Image.Image, label: str) -> None:
    rgba = image.convert("RGBA")
    alpha = rgba.getchannel("A")
    minimum, maximum = alpha.getextrema()
    if minimum == 255:
        raise VerificationError(f"icon has no transparent corners: {label}")
    if maximum != 255:
        raise VerificationError(f"icon center is not opaque: {label}")

    corner_alpha = [
        alpha.getpixel((0, 0)),
        alpha.getpixel((rgba.width - 1, 0)),
        alpha.getpixel((0, rgba.height - 1)),
        alpha.getpixel((rgba.width - 1, rgba.height - 1)),
    ]
    if max(corner_alpha) == 255:
        raise VerificationError(
            f"icon corners are fully opaque: {label}: {corner_alpha}"
        )


def verify_png(
    path: Path, expected_size: tuple[int, int], *, rounded: bool = False
) -> None:
    require_file(path)
    try:
        with Image.open(path) as image:
            image.verify()
        with Image.open(path) as image:
            actual_size = image.size
            actual_mode = image.mode
    except Exception as error:  # Pillow exposes format-specific exceptions.
        raise VerificationError(f"invalid PNG: {path.relative_to(ROOT)}: {error}") from error

    if actual_size != expected_size:
        raise VerificationError(
            f"wrong PNG dimensions for {path.relative_to(ROOT)}: "
            f"expected {expected_size[0]}x{expected_size[1]}, "
            f"got {actual_size[0]}x{actual_size[1]}"
        )
    if actual_mode != "RGBA":
        raise VerificationError(
            f"wrong PNG mode for {path.relative_to(ROOT)}: "
            f"expected RGBA, got {actual_mode}"
        )
    if rounded:
        with Image.open(path) as image:
            verify_rounded_alpha(image, str(path.relative_to(ROOT)))


def verify_ico() -> None:
    require_file(WINDOWS)
    try:
        with Image.open(WINDOWS) as image:
            sizes = image.ico.sizes()
    except Exception as error:  # Pillow exposes format-specific exceptions.
        raise VerificationError(f"invalid ICO: {error}") from error

    expected = {(size, size) for size in ICO_SIZES}
    missing = sorted(expected - sizes)
    if missing:
        formatted = ", ".join(f"{width}x{height}" for width, height in missing)
        raise VerificationError(f"ICO is missing layers: {formatted}")

    with Image.open(WINDOWS) as image:
        for size in ICO_SIZES:
            verify_rounded_alpha(
                image.ico.getimage(size=size),
                f"{WINDOWS.relative_to(ROOT)}:{size}x{size}",
            )


def verify_icns() -> None:
    require_file(MACOS)
    with MACOS.open("rb") as icon:
        if icon.read(4) != b"icns":
            raise VerificationError("macOS icon does not have an ICNS header")

    errors = list(IcnsFile.verify(str(MACOS)))
    if errors:
        raise VerificationError("invalid ICNS: " + "; ".join(errors))


def verify_desktop_entry() -> None:
    path = LINUX / f"{ICON_NAME}.desktop"
    require_file(path)

    entries: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        entries[key] = value

    expected = {
        "Type": "Application",
        "Exec": "zenterm",
        "Icon": ICON_NAME,
    }
    for key, value in expected.items():
        if entries.get(key) != value:
            raise VerificationError(
                f"desktop entry has {key}={entries.get(key)!r}; expected {value!r}"
            )


def main() -> None:
    require_file(SOURCE)
    verify_png(RUNTIME, (1024, 1024), rounded=True)
    verify_ico()
    verify_icns()

    for size in LINUX_SIZES:
        path = LINUX / "hicolor" / f"{size}x{size}" / "apps" / f"{ICON_NAME}.png"
        verify_png(path, (size, size), rounded=True)

    verify_desktop_entry()
    print("Icon assets verified:")
    print(f"  canonical artwork: {SOURCE.relative_to(ROOT)}")
    print(f"  runtime PNG: {RUNTIME.relative_to(ROOT)}")
    print(f"  Windows ICO: {WINDOWS.relative_to(ROOT)} ({len(ICO_SIZES)} layers)")
    print(f"  macOS ICNS: {MACOS.relative_to(ROOT)}")
    print(f"  Linux hicolor PNGs: {len(LINUX_SIZES)} sizes")
    print(f"  desktop entry: {(LINUX / f'{ICON_NAME}.desktop').relative_to(ROOT)}")


if __name__ == "__main__":
    try:
        main()
    except VerificationError as error:
        raise SystemExit(f"icon verification failed: {error}") from error
