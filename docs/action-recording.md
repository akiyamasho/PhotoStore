# PhotoStore 0.7.0 action recording

Native action recording was implemented for the PhotoStore 0.7.0 milestone. The current 0.12.0
policy defaults recording off. The public behavior reference is Procreate's [Video handbook](https://help.procreate.com/procreate/handbook/actions/actions-video), [Canvas handbook](https://help.procreate.com/procreate/handbook/actions/actions-canvas), and [recording-quality guidance](https://help.procreate.com/articles/drvany-adjust-time-lapse-recording-quality): recording is configured when a canvas is created, can be paused, replayed, and exported, and records committed artwork changes rather than a screen capture.

PhotoStore records committed canvas actions through the session edit commit hook. The menu path is **Actions › Video**. This is canvas-action frame storage, separate from command-macro recording and replay. Undo, UI-only commands, cursor or selection movement, and wall-clock sampling are excluded. Recording settings and captured frames persist as native `.pcraft` document data and as PhotoStore metadata inside PSD/PSB files; the command Actions list is unrelated. Reopening a PhotoStore-saved PSD restores its stored recording. A foreign document without that metadata cannot recreate progress recorded before it was opened in PhotoStore.

New canvases default to recording disabled and reset the New Document checkbox to off each time the
dialog opens. The explicit API and presets can opt in, and existing stored enabled documents retain
their recording settings and frames. When enabled, settings include 1080p (1920×1080), 2K
(2732×2048), or 4K (3840×2160), with Low, Good, Studio, or Lossless quality and optional HEVC
export. Replay and export run at 30 fps. Export supports the full recording or a 30-second
selection. The final original sampler uses 16 temporal bins, keeps the first and last frames, and
weights early action transitions and time coverage. It does not claim Procreate's private algorithm
or exact proprietary visual-importance behavior.

Capture is synchronous and bounded on each committed edit, so large canvases and 4K settings may add latency to the edit. Recording pauses when the 256 MiB encoded recording cap or frame cap is reached. The document keeps the pause/error state until the recording is purged; purge is available from **Actions › Video**. While paused, already captured frames remain available for replay and export; purge removes that recorded history so capture can resume. Turning recording off does not retroactively recover omitted frames.

Layers marked **Private Time-lapse Layer** are excluded from future captured frames while remaining visible on the canvas. Private File/Photo layers are excluded before a frame is inserted. Frames already recorded retain the pixels that were captured before the marking changed. A global recording-disable flag applies independently to existing and newly created documents, while per-document flags remain independent.

The native encoders use JPEG quality 45, 75, or 94 for Low, Good, and Studio, PNG for Lossless, and HEVC export settings with CRF 28, 21, 16, or RGB lossless. A codec proof established one-frame 16×16 gbrp HEVC Lossless pixel equivalence. These are codec and compiler checks, not a claim of broad UI acceptance.

The implementation is a native, action-driven recording path with bounded memory, deterministic replay/export, and explicit privacy behavior. The current 0.12.0 source pass defaults recording off, has approved preview captures, and is published and installed. Full native visual QA, UI parity, and full Procreate parity remain unverified.

## PSD/PSB recording metadata (2026-10-31)

PhotoStore stores recording settings, enabled state, pause/error state, action labels, and encoded
JPEG/PNG frames inside the PSD/PSB image-resources section. The PhotoStore resource also keeps
per-layer recording exclusions using the PSD's layer IDs, including when recording is not enabled.
Saving a purged recording writes its current empty frame list. Existing artwork, layer data, and
XMP remain independent of this resource.

This uses the plug-in resource range in [Adobe's public PSD specification](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/).
The resource has a PhotoStore name, signature, and version so unrelated plug-in metadata is not
treated as a recording. Recognized recordings are validated against the existing frame, byte, and
text limits. An invalid recording cannot be silently discarded by the normal PSD save path.
Malformed or newer metadata produces an import warning and remains preserved as opaque metadata;
the artwork still opens. Such metadata is not replayable by this version.

The file remains a standard layered PSD/PSB; Photoshop has no PhotoStore timelapse playback UI.
Opening and re-saving this resource in Photoshop has not been manually verified, and another
application may remove custom metadata when it re-saves a file. Keep an original PhotoStore save
when round-tripping through other software. Recording remains off by default.

## Conditional default assessment (2026-10-12)

Issue #10 asks whether recording could turn on automatically when memory overhead is low. The answer
is currently deferred and recording remains off by default. Each recording stores encoded JPEG or PNG
frames in `Arc<Vec<u8>>`, with a per-recording cap of 256 MiB and 100,000 frames. Capture composites
synchronously on committed edits. The default settings are 1080p and Good quality.

Raw RGBA8 storage alone is approximately 7.91 MiB per 1920×1080 frame and 31.64 MiB per
3840×2160 frame, before f32 render buffers, RGB conversion, encoder allocations, existing frames,
document state, history, and other process memory. The 256 MiB limit bounds encoded recording data;
it is not a global or process RSS bound. These deterministic pixel-size calculations therefore do
not establish negligible total memory overhead, and no representative RSS measurement was performed
for this sweep. No new recording, measurement, benchmark,
or default enablement is introduced until a future request supplies a memory budget and accepts
representative measurement work.

For this sweep, recording remains off by default and the conditional enhancement remains deferred.
No runtime acceptance was run, and no automatic enablement or release verification is claimed.
