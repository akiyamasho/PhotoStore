# PhotoStore 0.17.0 Generative Fill captures

Captured from the final installed ARM64 release binary, source `7bcc810ace56320943da02c5c40b3dc4f72ecb4d`,
executable SHA-256 `817e393b9390c6ff9f427e05f3660a01e1ef61dcc14d9b29caf7b352a0bc194e`. Recording was disabled and settings were
isolated from production preferences. The source image is an original 1024×768 geometric seaside
SVG, with selection `[400,320,320,320]`; no private artwork or external source media was used.

| File | Visible feature | Method and limits |
|---|---|---|
| `generative-fill-dialog.png` | Prompt, context disclosure, boundary matching, provider settings | Real UI populated through `ui.dialog.open` and synthetic control input; does not independently verify native menu/mouse routing. |
| `generative-fill-result.png` | Sailboat on a top-level Generative Fill layer, 320×320 at X=400/Y=320, with matched boundaries | Real Codex-generated patch, replayed through the configurable adapter into the final boundary matcher; final document reopened in PhotoStore. |

The local Codex producer succeeded on candidate source
`284283c03b3a8ebd0b8a6bdf0011c9cc200ce051`. Its saved 320×320 output was reused to isolate
boundary matching from random model variation. The final capture uses the release source above.
This is scoped generation/capture evidence, not a general test suite or a guarantee that all
model results will match every scene naturally. Hashes are in [evidence.json](evidence.json).

| Asset provenance | Source | Distribution |
|---|---|---|
| Both UI captures | Original PhotoStore demo; actual local Codex image generation and PhotoStore compositing | Included under this repository's project licensing; no third-party reference media. |
