# Smooth preview and remaining input issues — 2026-11-02

Scope: all three open non-PR issues at inventory: #23 timeline/performance, #30 Space-drag panning, #31 pen secondary-click. The supplied 10.9-second private recording was inspected locally; its media is not a repository/release asset. The exact running version and source codecs are not established by that recording.

## Findings and implementation

- The conditional Buffering preview text changed toolbar minimum width, shifting the time ruler/gutter. Reserve a fixed status slot, keeping ordinary in-flight requests quiet until they have waited 100 ms.
- Paused decode completions still ran synchronous video/CMS/Smart refresh on the UI thread. Reuse the generation-safe presentation worker, preserve authored history/clean state, wake the native event loop immediately when a candidate completes, and overlap the next clock-selected preparation with current canvas presentation.
- Synchronization and linked readiness disagreed about hidden ancestors and muted/out-of-range layers. Use the same requested-frame visibility predicate, so invisible media does not decode or block visible readiness. Effective track mute must also reach rendered visibility.
- Decoder ownership used a 64-entry expiring assignment map despite worker-local warm streams. Retain affinity while queued/running/warm, reconcile on completion/retirement, and bound the table at 128 with backpressure.
- Raw decode/skip allocated frame vectors repeatedly. A fixed pool of four reusable vectors covers the reader, two queued packets and current/recycled output within the existing four-RGBA-equivalent estimate. Codec-internal memory remains outside that estimate.
- Eligible axis-aligned downscaled linked video decodes at its placed dimensions in PreviewScope instead of decoding 4K and bicubic-warping it on CPU. Eligibility requires checked source fingerprints, original video metadata, positive reductions and integral dimensions/translation. Preserve original placement and source tiles; normalize only bounded ratio roundoff. Integer translations bypass the resampler. Other transforms and explicit output keep full-source evaluation.
- Readiness no longer color-manages a frame that actual synchronization will color-manage again. Full-source placement still validates the decoded footprint; reduced previews validate trusted original geometry.
- A late Space chord could use an old press origin and jump the camera or steal a live stroke. Start late pans from the current pointer, preserve existing capture to release, and retain the original tool through the stroke's Up event.
- A pen secondary press inside canvas could be rejected when no preceding hover event was delivered. Use the press position and topmost canvas layer; retain popup/overlay protection and the existing erase/Alt-resize/modifier preferences.

The built-in GPT Image [design](../design/2026-11-02/smooth-preview/workflow.png) and [exact prompt/inspection](../design/2026-11-02/smooth-preview/prompt.md) preceded implementation. Its Auto selector is conceptual; this pass does not add a disconnected quality control.

## Delivery and limits

One exact-source normal app/CLI production build, required artifact verification, immutable source release and atomic reinstall. Scheduler stays disabled/unloaded with no pending work through final documentation push. No GUI recording, screenshots, broad suites or QA agents. A bounded final-binary original-4K paced measurement can reproduce the affected pipeline's throughput and frame preservation, but cannot prove native window scanout, pen-driver delivery, focus or audio-device output.

The original timeline/keyframe/shortcut requirements are implemented by the prior cited releases. Unsupported perspective animation, HDR/codec limits and the separate Smart affine footprint guard remain explicit product boundaries; they are not fabricated new parity claims. Close only landed source corrections with actual evidence. Keep any unverified native performance acceptance accurately distinguished from implemented/compiler-verified behavior.


## Measured preparation failure

The first unpublished 0.20.7 candidate passed pixel/state preservation but failed the unchanged eight-original-4K paced target at 15.19 frames/s. A targeted two-second sample identified repeated Surface::content_bounds scans in presentation readiness and synchronization. Identity transforms need neither footprint computation nor a destination allocation: skip those scans before touching pixels, retaining nontrivial transform validation. Eligible placed-size previews likewise avoid a decoded-bounds scan when their effective transform is identity. Returning an already-correct RGBA8 decode avoids another tile-map clone. Re-run only the failed bounded playback measurement on the corrected final binary, plus the required placement preservation boundary. [Failure receipt](../performance/0.20.7-before.json).
