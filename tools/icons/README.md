# Zenterm icon assets

The canonical artwork is `../../assets/brand/zenterm.svg`. Generate every
platform asset with:

```text
uv run --project tools/icons python tools/icons/generate_icons.py
```

Generated outputs:

- `assets/runtime/zenterm.png`: embedded by the native eframe application.
- `assets/windows/zenterm.ico`: Windows icon sizes from 16px through 256px.
- `assets/macos/zenterm.icns`: static macOS ICNS with standard and Retina sizes.
- `assets/linux/hicolor/`: freedesktop icon-theme PNG sizes.
- `assets/linux/org.eu.eslzzyl.zenterm.desktop`: Linux desktop entry.

The local `tools/icons/.venv` directory is ignored. The committed `uv.lock`
keeps the conversion tool versions reproducible.
