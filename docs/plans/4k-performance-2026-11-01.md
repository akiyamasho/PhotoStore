# Concurrent 4K playback and compact audio — #23

Status: implementation in progress after the 0.20.4 user report. The user explicitly requested
completion of remaining performance work, many smooth 4K sources, compact audio within each video
row, and push/release/reinstall of the latest normal app. Publication is authorized.

## Outcomes and dependencies

1. **Compact timeline:** 48-point video rows with a thin internal linked waveform strip, one row
   header with source mute, no duplicated Movie Audio row. A standalone sound file keeps one
   compact editable waveform row. Source order, layer ownership and linked edit semantics stay
   intact. This UI scope is independent of decode work.
2. **Concurrent source pipeline:** remove the full-frame Rec.709 conversion temporary, write the
   exact CMS-derived byte ramps directly into tiles, and borrow byte-compatible tiles for GPU
   upload rather than cloning every tile. Keep color/depth fallback and original-source export.
   Size the preview worker pool to the available host cores (1/2/4), with memory-based admission,
   a global eight-session ceiling, shared retained accounting, bounded current queues and fair
   exact-frame-first work. Sleeping workers must not imply extra decode work or reservations.
3. **Rendering and cadence evidence:** one final normal app+CLI release build, then measure the
   same signed CLI with the production hardware wgpu compositor and completion wait. Compare the
   old six-source decode workload; target eight distinct 3840×2160 SDR/30 fps originals at at least
   30 completed timeline frames/s in steady state, plus a paced 180-frame/30 fps run with p95
   completion lateness at most 5 ms. Check every source's changing sampled hashes, authored
   opacity/visibility, clean/history state, resource bounds and final composed pixels against the
   CPU reference. Cold decoder/shader startup is reported separately rather than hidden in warm
   cadence. No screenshots or GUI automation are needed for this headless renderer measurement.

The [new GPT Image reference](../design/2026-11-01/compact-4k/workflow.png) and
[exact prompt/inspection](../design/2026-11-01/compact-4k/prompt.md) were generated and inspected
before implementation. Its Auto quality control and viewport preview branch are conceptual,
not a claim of a new shipped preview-quality feature. The current pass first targets full
original geometry; do not pass the throughput gate by silently lowering source resolution.

## Architecture and safety contracts

- One serializable document/layer/timeline model, editable Smart Objects, masks, precision and
  existing command registry. No rasterization or authored-pixel changes for performance.
- Sequential decoder sessions are source-revision/timestamp/format/geometry qualified. Retain
  shared cache/backpressure, original export paths, cancellation and actionable source errors.
- The shared retained target remains derived from Memory Usage and physical RAM; don't increase
  the allowance or write private preferences during installation. Decoder working-set estimates
  are not a total process/GPU/codec memory cap.
- Audio drawing uses a bounded texture cache and a single image draw per visible waveform.
  Reuse cached waveforms during playback; do not launch cold waveform extraction or stat source
  files just to draw every frame. Nonlinear speed and trim sampling must remain aligned.
- Host-sized workers must use their actual successfully started count for assignments so a thread
  creation failure cannot leave a source permanently queued for a nonexistent worker.
- For plain Normal RGBA8 stacks, fuse source-over composition into one FP32 pass per chunk only
  after recognizing the exact planner slot graph. Unsupported features keep the general path.
  Pack mapped transfers in row order; keep source/current-frame and GPU bounds authoritative.
- GPU measurement must reject software/CPU adapters, include queue completion, use a bounded wait,
  report adapter/format/uploads and compare a few final pixels. Avoid per-frame CPU readback.
  This is stronger than decode-only timing but does not prove display scanout, sound-device output,
  native input latency or parity with an unmeasured CapCut session.

CapCut documents proxy preview as a performance feature that lowers preview quality
([official guidance](https://www.capcut.com/help/video-quality-change-after-exporting)). FFmpeg
warns that hardware acceleration can incur GPU-to-CPU copy costs
([official documentation](https://ffmpeg.org/ffmpeg.html)). Apple's CoreVideo texture cache can
share GPU-based pixel buffers with Metal
([Apple documentation](https://developer.apple.com/documentation/corevideo/cvmetaltexturecache-q3j)).
These inform future native decode adapters; this pass does not label an FFmpeg rawvideo pipe
zero-copy or hardware decode. A measured bounded workload defines delivery evidence, not a claim
that any number of 4K clips/effects/codecs can play in real time on every host.

## Delivery

Keep the local scheduler disabled/unloaded and prove no pending work before the build and after
the final documentation-only push. Build app+CLI once from one committed SHA; use the same
verified artifacts for push/release/tag and immediate atomic normal installation. Preserve the
running app, open documents, preferences, recovery and rollback. No dev app or watcher build.
