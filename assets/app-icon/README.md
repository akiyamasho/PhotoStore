# PhotoStore app icon

A folded-corner **Ps** monogram: a navy rounded tile with a bright cyan PhotoStore mark and folded
corner. The supplied PNG is the canonical generated artwork for the application icon.

## Provenance

Generated from the prompt in `photostore-icon-prompt.txt`; `photostore-1024.png` is the canonical
source bitmap. The SVG wrappers embed that bitmap as a data URI so they work from every packaging
working directory. License and provenance are recorded in `LICENSE.txt`; no upstream marks or
third-party logo assets are used.

## Files

- `photostore.svg`: canonical SVG wrapper around the source bitmap.
- `photostore-small.svg`: compact SVG wrapper; also the hicolor scalable icon.
- `photostore-1024.png`: 1024 px render on the macOS grid.
- `photostore.icns`: macOS bundle icon (`CFBundleIconFile`).
- `photostore.ico`: Windows icon, 16–256 px, embedded in the `.exe` by `apps/photostore/build.rs`.
- `hicolor/<size>/apps/ai.photostore.app.png` (16–512) and `hicolor/scalable/...svg`:
  Linux icon theme.

The app also sets the window icon and Wayland app ID at runtime (`apps/photostore/src/app_icon.rs`,
`main.rs`). On Windows the taskbar shows the icon of the Start Menu shortcut that launches the
exe, so the MSI's `<Icon Id>` keeps the `.exe` extension (ICE50); `app_icon.rs` documents the
details and tests the `.ico` sizes and the WiX icon references.

## Regenerate

Run `packaging/icons.sh` after replacing the canonical source bitmap. It rewrites the self-contained
SVG wrappers, uses installed `resvg` for raster derivatives, Python's standard library for ICO,
and `iconutil` when available for ICNS. Committed derived formats remain usable without regeneration.
