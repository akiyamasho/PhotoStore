# PhotoStore 0.17.1 Generative Fill captures

Captured from the final installed ARM64 release, source `9b96ad80ad2d5222ca6fa790754c62e48dc733a1`,
executable SHA-256 `0c13ae81827ac4bf121c561ecfcc35897fd96be0defa76be9afbd02dbc513e7d`. Recording was disabled and preferences were
isolated. The disposable source is an original 1024×768 seaside SVG, with selection
`[400,320,320,320]`. No private artwork or third-party reference media appears.

| File | Feature and capture method |
|---|---|
| `generative-fill-dialog.png` | Prompt-only dialog with local setup available. |
| `generative-fill-settings.png` | Provider, executable, timeout, boundary matching, and setup check in Preferences. |
| `generative-fill-setup.png` | Missing executable: Open Settings link and disabled Generate. |
| `generative-fill-result.png` | Actual Codex-generated sailboat, clipped to the selection and boundary-matched on a new layer. |

These are real PhotoStore UI captures. Demo/selection setup used the control API and synthetic input;
provider path entry and Apply/OK also used native accessibility input.
The Open Settings link was activated through synthetic input to show the correct Preferences
section. This scoped capture does not independently verify all native OS menu/pointer routing. The Codex output
was generated on candidate source `284283c03b3a8ebd0b8a6bdf0011c9cc200ce051`, saved, then
replayed through the final configurable adapter to isolate boundary matching from random model
variation. Capture hashes and dimensions are in [evidence.json](evidence.json).

| Asset provenance | Source | Distribution |
|---|---|---|
| All four UI captures | Original PhotoStore demo, actual local Codex generation, PhotoStore compositing | Repository project licensing; no third-party reference media. |

This is scoped generation/capture evidence, not a general test suite or a guarantee of semantic
matching for every scene. Initial scope: sRGB RGB, at most 25 million pixels, 8-bit PNG interchange.
