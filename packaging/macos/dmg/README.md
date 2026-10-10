# macOS DMG

The PhotoStore DMG is intentionally plain: it contains `PhotoStore.app` and an `Applications`
symlink and is created by `packaging/macos/package.sh` with `hdiutil makehybrid`. No custom Finder
background, generated `.DS_Store`, or external artwork is required.
