# Zenterm icon assets

The canonical artwork is `assets/brand/zenterm.svg`. All checked-in raster and
native icon formats are derived from it.

Generate assets only when the SVG changes:

```text
uv run --project tools/icons python tools/icons/generate_icons.py
```

Verify checked-in assets without generating or packaging anything:

```text
uv run --project tools/icons python tools/icons/verify_icons.py
```

## Platform mapping

| Asset | Use | Rule |
| --- | --- | --- |
| `assets/runtime/zenterm.png` | eframe runtime icon on Windows and X11 | Square, RGBA, 1024x1024. The current artwork intentionally has an opaque blue background. |
| `assets/windows/zenterm.ico` | Windows package, installer, shortcut, and native icon layers | Contains 16, 24, 32, 48, 64, 128, and 256 pixel layers. |
| `assets/macos/zenterm.icns` | macOS application bundle | AppKit receives the bundle ICNS and applies the macOS Dock/Finder mask. |
| `assets/linux/hicolor/` | Linux package icon inputs | PNGs in hicolor size directories, using the independent reverse-domain icon name. |
| `assets/linux/org.eu.eslzzyl.zenterm.desktop` | Standalone FreeDesktop resource | Uses `Icon=org.eu.eslzzyl.zenterm` and is kept separate from cargo-packager's generated entry. |

## Why the PNG is platform-correct

Apple's [App Icon Human Interface Guidelines](https://developer.apple.com/design/Human-Interface-Guidelines/app-icons)
require square, unmasked macOS icon layers so the system can apply its own
rounded-corner mask. Therefore macOS deliberately passes
`egui::IconData::default()` to eframe; supplying the raw PNG there would let
eframe replace the bundle icon at runtime.

The [winit window icon documentation](https://docs.rs/winit/latest/winit/window/struct.Window.html)
states that `set_window_icon` is supported on Windows and X11, but unsupported
on macOS and Wayland. This is why the runtime PNG remains enabled for Windows
and Linux/X11 and is omitted on macOS. Wayland uses the desktop-entry/app-ID
path instead of this window-icon API.

The [FreeDesktop icon theme specification](https://specifications.freedesktop.org/icon-theme/latest/)
defines hicolor directories such as `48x48/apps` and accepts PNG icon files.
The project's Linux resources follow that layout. The [Tauri icon guide](https://v2.tauri.app/develop/icons/)
also documents the common desktop mapping `icns = macOS`, `ico = Windows`,
and `png = Linux`, including square RGBA PNG requirements.

The PNG's opaque square background is intentional artwork, not a platform
format error. Windows and Linux do not universally apply Apple's Dock mask;
their desktop shells render the supplied square image according to their own
icon presentation rules.

## Linux naming contexts

There are two valid Linux resource contexts in this repository:

1. `assets/linux/org.eu.eslzzyl.zenterm.desktop` is a standalone
   reverse-domain FreeDesktop resource and refers to the matching
   `org.eu.eslzzyl.zenterm.png` icon name.
2. The cargo-packager package uses the binary name `zenterm`. Its Linux
   backend accepts PNG inputs, copies them into `hicolor`, and normalizes the
   installed filename and generated desktop entry to `zenterm.png` and
   `Icon=zenterm`. The glob in `crates/zenterm/Cargo.toml` supplies the
   source PNG sizes; it does not cause the standalone desktop file to be
   shipped verbatim.

The Linux root and settings viewports set `app_id=zenterm` to match the
cargo-packager desktop entry on Wayland. `ViewportBuilder::app_id` is a
Wayland-only setting; X11 continues to use the runtime PNG window icon.

## Verification boundaries

`verify_icons.py` checks the source file, PNG dimensions/modes, ICO layers,
ICNS validity, Linux hicolor paths, and desktop-entry keys. It is read-only.

The repository checks in generated native formats so macOS builds can consume
the ICNS without requiring icon conversion on the target machine. The current
verification does not run the application, perform a real package build,
cross-compile, or assert the visual result of an installed desktop shell.

The local `tools/icons/.venv` directory is ignored. The committed `uv.lock`
keeps the conversion and verification tool versions reproducible.
