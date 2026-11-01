# Layered media playback and ownership

Status: normal 0.20.4 published and atomically installed from `9fa1ffb6f38b70e4b3ec9ebdb5ba4203ef086cf5`. Focused data/permission checks and multi-source headless measurement passed; native display/audio performance remains manual. Separate reports: #33 stack order,
#34 overlap/opacity playback, #35 layer-owned audio; #23 retains the wider timeline work.

## Contract and implementation order

1. Use the Layers panel's top-down hierarchy for timeline row ordering, including groups above
   children. Derive visual track position from its topmost member; preserve clip membership,
   time ranges and existing mute/lock state. Empty authored tracks remain deterministic. Rebuild
   the cached row index on authored edits, not every playback frame.
2. Preserve simultaneous source sampling and ordinary document compositing. Static opacity edits
   must persist across dense legacy snapshots. Explicit opacity animation continues to own its
   channel; untouched legacy documents retain their animation behavior. A missing source frame
   must hold a complete presentation rather than silently remove one overlapping layer.
3. Associate each movie's audio with its source and show it next to that source. Represent newly
   imported standalone audio with a real, lightweight document layer and serialized ownership.
   Audio-only layers allocate no canvas-sized pixels. Existing legacy audio tracks stay readable.
   Preserve independent clip gain/mute, trim/speed/fades and the audio master clock. Source opacity
   affects the picture, not sound volume. Image and editable Smart Object layers retain existing
   drawing/compositing behavior rather than being flattened into video proxies.
4. Integrate changes and build the normal app and CLI once from one committed source SHA, then
   measure the shipped binary's bounded multi-source decode/presentation path. Package and verify
   once, push/release, and atomically reinstall identical artifacts. Keep the scheduler suppressed.

Items 1/3 and 2 are independent implementation scopes. Primary owns architecture, instrumentation,
integration and release. No browser/app capture, broad QA suite or extra compiler build is planned.
The [GPT Image design](../design/2026-11-01/timeline-order/workflow.png) and
[exact prompt/inspection](../design/2026-11-01/timeline-order/prompt.md) preceded implementation.
The generated 100% opacity text is illustrative; actual authored values are authoritative.

## Resource and performance architecture

Keep source media linked and decode on demand. Cache keys include source revision, exact source
time, output geometry and format. Consumers include document/layer identity so overlapping source
requests do not cancel one another. Queues, resident derived data, decoded lookahead, stream
sessions and waveform work must remain bounded; prioritize current visible sources over filmstrips.
Use memory preferences and physical RAM, not a fixed 64 MB media-size gate. Large files never mean
decoding a whole movie into memory. Do not equate a cache allowance with total process memory.

4K concurrency depends on codec, source count, effects, masks, precision, working space and hardware.
Retain explicit proxy preview and original-quality export separation. CapCut's documentation says
proxy preview improves playback while reducing preview quality
([CapCut guidance](https://www.capcut.com/help/video-quality-change-after-exporting)). Photoshop
documents per-video audio controls and multiple video/image layers
([Adobe timeline guidance](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/organize-and-manage-video-groups/edit-video-clips-and-audio-in-the-timeline.html)).
These are workflow references, not evidence of PhotoStore performance parity.

Apple VideoToolbox provides compression/decompression and CoreVideo pixel buffers
([Apple documentation](https://developer.apple.com/documentation/videotoolbox)). A future native
GPU-resident decode adapter can avoid rawvideo pipe/readback costs, but existing FFmpeg CPU surfaces
must not be described as zero-copy or hardware-decoded without evidence. A scalable GPU-resident
multi-source graph is a distinct architectural step; raising a decoder count does not implement it.

The extended `media.performance.measure` accepts 1–16 source paths in an explicitly authorized
local CLI run. It reports all source dimensions, per-layer changing pixel hashes, visibility/opacity
preservation, decoder launches, latency percentiles and retained derived bytes. It excludes GPU
composition, display, OS input and sound-device output, so its throughput is **not native playback
FPS**. Ambient filesystem access remains blocked through untrusted automation. User media is never
uploaded for measurement; synthetic local 4K fixtures are sufficient for this bounded pass.

## Acceptance still requiring native evidence

Check matching Layers/timeline order after reorder/group edits; play overlapping sources at partial
opacity; mute and change gain per source; add standalone audio; save/reopen/copy/delete/undo owned
audio; compare real multi-source 4K playback and input responsiveness on the target machine.
Headless measurement and a successful compiler pass do not establish CapCut-like native performance.

Destination copies preserve associated audio tracks, owner/clip/catalog identities and layer ranges in one undo transaction. Different-project-rate copies containing speed curves fail with an actionable same-rate requirement rather than silently changing timing; Frame Animation destinations require explicit Video Timeline creation. New optional ownership is native v3 data; older readers can ignore it. Legacy shared movie audio is projected into separate per-owner lanes without rewriting the file on open. Paused decoder sessions reserve four RGBA8 frame-buffer equivalents each, with at most four sessions per worker/eight total and two active windows; codec-internal/process memory remains outside that estimate.
