# PhotoStore 0.12.0 screenshot evidence

- Source: PhotoStore 0.12.0 arm64 preview build, marker `preview-0.12.0-default-off`, built 2026-10-11 UTC; bundle identifier `ai.photostore.app`.
- Demo documents: disposable blank `Recording-default-demo` and `Recording-proof-demo` canvases, each 1920 × 1080 px at 72 ppi. Any existing user/recovered tabs were preserved; the evidence images contain only disposable demo UI.
- `new-document-recording-default-off.webp`: Cropped New Document flow after naming the disposable document `Recording-default-demo`. The `Record canvas actions` checkbox is visibly present and unchecked (`Value: off` in the accessibility tree); Create is visibly available.
- `recording-settings-zero-frames.webp`: After creating the disposable `Recording-proof-demo` document, Actions › Video › Recording Settings was opened without enabling capture. The visible dialog shows `Frames: 0` and the global `Disable Time-lapse` preference unchecked; the accessibility tree also verified the unchecked `Time-lapse Recording` menu item and disabled Replay/Export items before opening settings.
- Limits: These screenshots establish the visible New/Create flow and zero-frame recording settings state only. They do not establish export correctness or full Photoshop parity.
- Published source/tag: `0ac6e812f1ded55e9a12c3a1fa0230c8fdbe3c6d`, `photostore-recording-default-off-2026-10-11`.
- The captures were confirmed visible in Chrome/CUA rendering from the published prerelease. They remain preview captures and do not prove final-app runtime behavior.
- Post-install targeted confirmation later passed for disposable `0.12-install-demo` (64×64, recording off, canvas/background/timeline controls visible, no error). This does not upgrade the preview captures into final-app runtime captures.

![New Document with recording unchecked](new-document-recording-default-off.webp)

![Recording Settings with zero frames](recording-settings-zero-frames.webp)
