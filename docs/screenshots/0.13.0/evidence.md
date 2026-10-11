# PhotoStore 0.13.0 shortcut evidence

Provenance: actual CUA screenshots from `/Users/computer/Desktop/dev/PhotoStore/target/macos-package/PhotoStore.app`, version 0.13.0, marker `PhotoStoreBuildCommit=preview-0.13.0-tool-shortcuts`, arm64 strict-ad-hoc preview. The preview used isolated `PHOTOSTORE_CONFIG_DIR=/private/tmp/photostore-shortcuts-demo-20261011/config`; the canonical `/Applications/PhotoStore.app` process and its documents were not operated.

The disposable blank document shown is `Untitled-1`, 64 px × 64 px. It was created with the New Document `Record canvas actions` checkbox visibly off. Three separate plain `M` presses were performed before customization; the canvas view screenshot shows the rectangular marquee tool selected, with the rectangular icon highlighted and the status bar showing 64 px × 64 px. `default-rect-flyout.webp` captures the actual marquee flyout before customization with the rectangular marquee row selected.

`tool-editor.webp` shows the actual Toolbar tab after editing: Rectangular Marquee remains `M`, Elliptical Marquee is `Unassigned`, Brush is `K` with `Shift+B` as an alternate, and the editor's Export PhotoStore Set button is visible. The editor's observed sort was `Default tool order`. The native save sheet displayed a malformed path (`/Users/computer/Documents/PhotoStore-shor:private:tmp:PhotoStore-shortcuts.jsontcuts.json`); that actual toolbar output was copied to `/private/tmp/PhotoStore-shortcuts.json` for inspection, then the misplaced generated file was removed. The inspected JSON contained the brush `K` override, ellipse tombstone, and `Shift+B` alternate.

In the second isolated preview copy at `/private/tmp/PhotoStore-0.13.0-evidence2.app` with config `/private/tmp/photostore-shortcuts-demo-20261011/config2`, the actual Keyboard Shortcuts search view showed all three eraser rows initially on `E`. Background Eraser and Auto Background Eraser were individually cleared and applied. `live-eraser-flyout.webp` shows the eraser flyout with ordinary Eraser carrying `E` while the background variants carry no key. After an `E` press, `live-eraser-after-e.webp` shows ordinary Eraser options. The prior mouse-selected state was not conclusively established; no exhaustive transition claim is made. `live-brush-flyout.webp` shows the Brush flyout with `K / ⇧B` on the Brush row; `live-brush.webp` shows the Brush tool selected after `K`.

Limits: these captures document the targeted fresh-config shortcut flow and editor state only. Native JSON import followed by Cancel was not completed in the second copy. They do not claim full shortcut parity across every tool or platform, and no source files, README, release metadata, or Git state were changed.

Files:

- `default-rect.webp` — 1440×900, 85,978 bytes; fresh 64×64 canvas after repeated `M` presses, rectangular marquee visibly selected.
- `default-rect-flyout.webp` — 1440×900, 91,034 bytes; fresh marquee flyout with rectangular row selected.
- `tool-editor.webp` — 1440×900, 167,740 bytes; Toolbar editor with the changed bindings and export control visible.
- `live-eraser-flyout.webp` — 1440×900, 93,498 bytes; eraser flyout showing ordinary `E` only.
- `live-eraser-after-e.webp` — 1440×900, 86,040 bytes; ordinary Eraser options are visible after `E`.
- `live-brush.webp` — 1440×900, 84,610 bytes; Brush selected after `K`.
- `live-brush-flyout.webp` — 1440×900, 91,878 bytes; Brush flyout visibly shows `K / ⇧B`.

Readable crops of the same pixels are also included: `tool-editor-crop.webp` (740×660), `default-rect-flyout-crop.webp` (250×105), `live-eraser-flyout-crop.webp` (240×115), and `live-brush-flyout-crop.webp` (240×115).
