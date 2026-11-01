# Layer copy, movie drop and canvas input — 2026-11-01

Implementation/acceptance record for separate issues #29–#32. This pass follows normal 0.20.2.
The user's corrected shortcuts are Cmd+R for direct duplication in the same document, and
Cmd+Ctrl+R for the Copy Layer popup. Cmd+Shift+R retains Lens Correction.

## Requirements and implementation

- #29: Copy popup captures source/destination DocIds and source/insertion LayerIds. It can copy
  selected layers or groups above a chosen layer in the same or another open document, with an
  optional name for a single copy. Registry `layer.copyToDocument` owns conversion, insertion,
  identity rebinding and one destination undo step. `allowSame:true` adds same-document support;
  its prior default rejects same-document calls. Same-document Smart content remains shared;
  cross-document Smart and media catalog IDs are rebound as a batch. Invalid/closed targets
  cannot silently redirect a copy. A pending canvas gesture must finish before opening the modal.
- Cmd+R's previous Rulers default moves to Cmd+Alt+Shift+R; the conflicting timeline speed-inspector
  default is unassigned. Explicit existing shortcut preferences remain authoritative and are not
  edited by installation. Cmd+Ctrl+R is a macOS default; other platforms retain menu access because
  Command maps to Control there and cannot form two independent modifier keys.
- #32: Video dropped on the active canvas (or a native drop lacking coordinates with an open
  document) queues a linked movie layer against its original destination DocId. Multiple imports
  serialize and wait for jobs/gestures; closing a target reports an error. Known tab-strip drops,
  explicit File Open, OS Open requests, images and audio keep their prior routing. Movie bytes
  are streamed/probed by the existing background registry job, not read wholesale in the UI.
- `createTimeline:false` preserves no timeline and Frame Animation. The optional persisted
  `VideoPlayback.awaiting_video_timeline` marker holds the preview static, including across
  Frame Animation seeks and native save/reopen. New Video Timeline creation or explicit mode
  conversion clears that marker, derives useful ranges/duration/work area and linked audio from
  persisted catalog metadata, and commits atomically with one undo. Existing Video timelines
  receive an active movie range/audio; hiding a timeline panel does not destroy timeline state.
  Native v3 serialization remains; old files default the optional flag false. Older readers may
  ignore the new optional playback intent, so this behavior requires the updated app.
- #30: Resolve camera navigation before constructing the transform used for rendering. Hand
  panning captures actual primary/middle press displacement before egui's drag threshold, tracks
  the pointer across canvas edges, includes release, and applies overscroll bounds before paint.
  Rotation, flip and device-pixel scaling use the existing inverse view transform. Canvas-owned
  Space does not toggle timeline playback; timeline key repeat toggles once per physical press.
- #31: Open the existing brush picker on a delivered secondary-button press, independent of
  release click classification/hover jitter. Alt resize, Command/Control layer picking, transform
  menus and the erase preference retain their routes. Preserve any already sampled brush stroke
  before consuming the remaining pen-contact/button gesture. Ignore the opening press for
  outside-click dismissal, and request a repaint immediately. No Wacom-specific fake clicks or
  device-driver configuration changes are introduced.

## Design and diagnostic evidence

The [corrected layer workflow](../design/2026-11-01/layer-workflows/workflow.png) and
[canvas input workflow](../design/2026-11-01/canvas-input/workflow.png) were generated with built-in
GPT Image and inspected before their affected implementation. Exact prompts/provenance accompany
both images. They use synthetic geometry and no private artwork/known character inputs. They are
design references, not product screenshots or evidence of native input latency.

The supplied five-second 2560×1440 recording was inspected locally only. It shows canvas navigation
alongside a video timeline, with the Zoom/Hand cursor changing. Capture cadence cannot establish
input or UI latency. No private recording frame is a repository asset or issue upload.

Two focused required data boundaries cover deferred video save/reopen, static Frame Animation,
Video activation/range/audio/work-area/undo and failed activation atomicity; and source-preserving
same/cross-document copies, media catalog rebinding and invalid placement. They are not a broad
UI QA sweep. Results, single production compiler/build and artifact delivery belong in the
[0.20.3 release record](../releases/0.20.3.md).

Native Wacom delivery, Space-drag feel, physical shortcut routing and OS file-drop acceptance
remain manual. The code can respond immediately to an event the driver delivers; it cannot
prove or repair a driver profile that emits no secondary-button event. Existing drawing,
original-source export, codec/format and resource bounds remain in force. #23 remains open for
prior native timeline acceptance; this pass adds these four bounded workflows.
