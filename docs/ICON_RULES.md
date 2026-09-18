# Zenterm icon rules and evidence

This document records which icon resource is used in each path and the
evidence behind that choice. It is intentionally separate from the artwork
source so a future icon change does not silently reintroduce a platform
integration bug.

## Short conclusion

The runtime PNG is the correct input for Windows and Linux X11. Windows and
Linux exports use a transparent 2px safety inset and a 10px outer radius at
the 48x48 icon grid, keeping the colored plate visibly separated from the
desktop surface. macOS must use the unmasked bundle ICNS at
runtime so AppKit can apply the system Dock/Finder mask. Linux Wayland does
not consume the winit window-icon API; it relies on the matching desktop
entry, hicolor icon name, and application ID.

## Evidence

### macOS

Apple's [App Icon Human Interface Guidelines](https://developer.apple.com/design/Human-Interface-Guidelines/app-icons)
say that macOS app icons are square and that the system applies masking to
produce rounded corners. Apple also instructs developers to provide unmasked
square layers because predefined masking can produce unwanted edge effects.

The project packages `assets/macos/zenterm.icns`. eframe's viewport icon is
optional, and [egui's `ViewportBuilder::with_icon` documentation](https://docs.rs/egui/latest/egui/viewport/struct.ViewportBuilder.html)
says that `IconData::default()` selects the operating-system default. On
macOS, the project therefore leaves the runtime icon unset. This preserves
the bundle ICNS rather than replacing it with `assets/runtime/zenterm.png`.

### Windows

[winit's `Window::set_window_icon` documentation](https://docs.rs/winit/latest/winit/window/struct.Window.html)
lists Windows support and describes the small title-bar icon and the base
16x16 size, with larger multiples recommended for scaling. The project keeps
the PNG as eframe's runtime input and packages a multi-layer
`assets/windows/zenterm.ico` for native Windows packaging paths.

The ICO contains 16, 24, 32, 48, 64, 128, and 256 pixel layers. The [Tauri
icon guide](https://v2.tauri.app/develop/icons/) independently documents the
same native format split (`ico` for Windows, `icns` for macOS, and PNG for
Linux) and requires common Windows ICO layers including 16, 24, 32, 48, 64,
and 256 pixels.

The development Windows executable follows a separate native-resource path:
`crates/zenterm/build.rs` embeds the same ICO into the PE executable through
`winresource`. This covers `cargo run`, whose taskbar and Explorer identity is
read from the executable resource rather than from eframe's viewport PNG.

The process also sets the fixed AppUserModelID
`org.eu.eslzzyl.zenterm`, matching the installed package identifier. This
keeps Windows taskbar grouping and icon fallback on the same application
identity for both `cargo run` and packaged builds.

Windows does not apply macOS's Dock mask to arbitrary application artwork.
The Windows/Linux PNG and ICO exports therefore carry their own transparent
rounded corners. The generator uses a fixed `2 / 48` transparent inset and
`10 / 48` outer radius so the rounding remains visible in the 16px, 24px,
32px, and 40px runtime representations.

### Linux X11 and Wayland

winit documents `set_window_icon` as supported on X11 and unsupported on
Wayland. The runtime PNG therefore covers the X11 window-icon path.

The [FreeDesktop Icon Theme Specification](https://specifications.freedesktop.org/icon-theme/latest/)
requires implementations to fall back to the `hicolor` theme and describes
the `48x48/apps` directory layout. It accepts PNG, XPM, and optionally SVG
files; it also says that an application author should install at least a
48x48 PNG in hicolor. Zenterm supplies multiple hicolor PNG sizes from 16
through 1024 pixels.

On Wayland, `egui::ViewportBuilder::with_app_id` is the relevant application
identity setting. The [egui viewport documentation](https://docs.rs/egui/latest/egui/viewport/struct.ViewportBuilder.html)
identifies `app_id` as Wayland-only. The root and settings viewports use
`zenterm`, matching cargo-packager's generated `zenterm.desktop` entry.

The repository also keeps a standalone reverse-domain desktop entry at
`assets/linux/org.eu.eslzzyl.zenterm.desktop`. That file intentionally uses
`Icon=org.eu.eslzzyl.zenterm` and is not evidence that cargo-packager emits
that exact desktop filename.

## Project resource graph

```text
assets/brand/zenterm.svg
        |
        +--> assets/runtime/zenterm.png       eframe runtime (Windows, X11)
        +--> assets/windows/zenterm.ico       Windows package/installer
        +--> crates/zenterm/build.rs           cargo run PE icon resource
        +--> assets/macos/zenterm.icns        macOS bundle/AppKit
        +--> assets/linux/hicolor/*/*.png     Linux package/icon theme
        +--> assets/linux/*.desktop            standalone FreeDesktop input
```

`tools/icons/generate_icons.py` creates the derived resources. It is the only
generator; `tools/icons/verify_icons.py` is read-only and checks the committed
outputs, including the transparent corner mask.

## cargo-packager detail

The checked-in package metadata in `crates/zenterm/Cargo.toml` lists the ICNS,
ICO, runtime PNG, and Linux PNG glob. cargo-packager 0.11.8's source code
shows the relevant behavior:

- macOS prefers an existing `.icns` input;
- Windows prefers an existing `.ico` input;
- Debian/Linux considers `.png` inputs and writes them under hicolor using the
  main binary name;
- the generated Linux desktop entry uses that same main binary name for
  `Icon=`.

This is why the package metadata can use reverse-domain filenames as source
inputs while the generated Debian package uses `zenterm.png` and
`Icon=zenterm`.

## Reproducible static checks

```text
cargo fmt --all -- --check
git diff --check
uv run --project tools/icons python tools/icons/verify_icons.py
cargo check --package zenterm
```

These checks do not launch Zenterm, run a simulator, perform a real package
build, cross-compile, or prove how a particular installed desktop shell
renders the icon. Those remain platform installation/runtime checks.
