# PhotoStore 0.14.0 screenshot evidence

- Source runtime: `/private/tmp/photostore-preview-package-0.14.0-20261011/PhotoStore.app/Contents/MacOS/PhotoStore` (bundle `PhotoStoreVersion` 0.14.0).
- Isolated configuration: `/private/tmp/photostore-native-evidence-20261011/config`.
- Isolated control listener: PID 63701, `127.0.0.1:17814`; control token was read privately from `/private/tmp/photostore-native-evidence-20261011/control.token`.
- Demo document: disposable 64×64 `Native Timeline Demo`; recording remained disabled (default new-document setting).
- Native artifact: `/private/tmp/photostore-native-evidence-20261011/work/native-demo.pcraft`, 8,314 bytes, ZIP magic `50 4b 03 04`; manifest generator `photostore-format 0.14.0`, `format_version: 1`.
- Manifest proof before seeking: `Video Layer 1` has 3 frames at 3 fps; frame 0 has one tile (`tx:0,ty:0`, hash `34d46b68ba60aed15eee114f163f902068dc94958c6dda8a7cf3f469ff42fc99`), frames 1 and 2 have no tiles. Timeline manifest is video mode, duration 3, current frame 0.

## Captures

- `unsaved-quit-cancel.webp` — native runtime screenshot, 1440×900. The disposable dirty document is visible behind the unsaved-change guard with Save, Don’t Save, and Cancel. The control flow invoked `app.quit`; Escape was sent and the document remained present during the control session.
- `timeline-saved-reopened.webp` — native runtime screenshot, 1000×700. The saved `.pcraft` was reopened through `app.open`, sought to frame 2 and back to frame 0, zoom was set to 400%, and the Timeline panel was opened. The screenshot shows Video Timeline, frame 1/3, 3.0 fps, duration 3, the playhead, Video Layer 1, and the painted video-layer thumbnail; the canvas is largely clipped.

## Limits

- These are isolated preview-runtime captures from the 0.14.0 package. No build, tests, release packaging, or Git operation was performed.
- The runtime used CPU canvas rendering under `--safe-gpu`; this does not assess GPU rendering parity.
- Only the disposable demo document was saved, closed/reopened, and inspected. Existing preview instances and recovered/private documents were left untouched.
