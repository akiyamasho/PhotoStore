# Smooth playback and scalable editing — issue #23 (2026-11-01)

Status: plan/design landed before code changes; implementation and 11 focused critical checks completed, production delivery and native acceptance pending. The user rejected 0.20.0 playback as too slow, supplied a 4.44-second recording, asked for CapCut-like responsiveness, and explicitly asked to remove the 64 MB limit, preserve Photoshop/CapCut-style editing plus drawing and editable Smart Objects, plan thoroughly, then sweep. Prior push/release/tag/normal-app installation authorization continues. #23 remains the sole open issue in the complete paginated inventory.

## Evidence and diagnosis

The current host is MacBookPro18,2, Apple M1 Max, 32 GiB unified memory. The user recording shows a single movie, linked audio, a moving timeline and a repeated Loading frame indicator while the canvas holds. Its 3010×1512 capture is variable-rate (206 sampled video frames over 4.375 seconds); that is capture cadence, not PhotoStore playback FPS. A coarse grayscale canvas crop changes substantially only three times by a stated mean-difference threshold. This supports the reported long holds but cannot establish source FPS, exact presentation latency, hardware decode, or a fair CapCut comparison. Recording frames remain temporary local diagnostic material, not repository assets or generation references.

Source-level findings to address:

1. `Surface::convert` always converts every sample through float vectors, even when input/output formats match. `decoded_surface` and `movie_surface_for_document` repeatedly take this identity path for full frames. A 1080p frame can incur millions of avoidable allocations and copies several times.
2. Preview opens another FFmpeg process for each single frame or short batch. A batch is retained only after every requested frame is decoded and converted. At 1080p the transient window cap limits each batch to about three frames; at 4K often one. Startup, repeated seeking, and batch-first publication prevent sustained decoding.
3. Every not-ready `timeline.presentFrame` poll clones the document and runs layer/Smart synchronization, source checks and repeated requests. Only a completed presentation needs a candidate document. Exact source requests must remain generation-safe, but polling must be cheap.
4. The UI waits on decoded document frames for its visible playhead/timecode, couples auxiliary work to render revisions, and permits cold filmstrip work during playback. The audio/transport clock must remain authoritative; queued source-frame work must be bounded and late output handled explicitly.
5. The canvas has an existing wgpu compositor, including editable layer/Smart content and display transforms. Some documents use its CPU fallback. A full changing movie requires refreshed pixels, but static layers, graph preparation, source probes, thumbnails, masks and color transforms should be reused. Do not presume the user's current backend or claim all full refreshes are CPU composition.

## Generated design reference

The fresh [GPT Image workflow board](../design/2026-11-01/smooth-playback/workflow.png) and its [exact prompt/provenance](../design/2026-11-01/smooth-playback/prompt.json) were saved and inspected before implementation. It is synthetic geometry, with no private inputs. Its duplicated Canvas/Preview panes, clock/key markers and branch arrows are illustrative: retain the existing editor/canvas and original-export contract. It provides no achieved-FPS or zero-copy claim.

## Product contract retained

- One serializable PhotoStore document, source catalog, layer hierarchy, timeline range/track model and registry. No separate video project database, flattened Smart substitutes, or second timing authority.
- Preserve Photoshop-like drawing/layer/mask/blend/ICC behavior, CapCut-like tracks/trim/split/keyframes/scoped shortcuts, linked audio, exact rational project rate, VFR held-frame sampling and editable Smart Object sources/affine animation.
- Source files remain read-only. Original export and explicit pixel materialization continue to bypass proxies. Native saves remain possible offline. Relink/cache generations, undo and dirty state keep their current contracts.
- Automatic caches and preview quality are disposable presentation state. They cannot lower authored document precision, alter layer geometry, silently become saved/baked pixels or change exported originals.

## Playback architecture for this sweep

### Cheap copies and conversion

Identity conversion returns a shared tiled Surface clone with no per-pixel conversion. Preserve exact encoded bytes, HDR values and copy-on-write isolation; converting to a genuinely different format still uses the existing conversion semantics. Optimize common tagged Rec.709-to-sRGB video conversion with a cached transfer LUT only when equivalent primary/channel/bit-depth conditions hold; retain ICC conversion for other working spaces. Cache profile work by source/profile identity rather than reprocessing it during every pending poll.

Authored frame snapshot banks also use copy-on-write ownership with the existing array serialization. Otherwise lifting the 64 MiB gate would make each document/presentation clone copy the entire large animation bank. History accounts each shared bank once by stable allocation identity with a cached estimate, alongside shared pixel tiles. Editing detaches; playback/history clones share. No native-format version change is required for this in-memory representation. The native writer uses compact JSON with the same schema, reducing large snapshot-manifest whitespace; the separate configurable native-loader manifest/decompression quotas remain input guards and must be recorded honestly.

### Persistent sequential decoder and early publication

Use a long-lived sequential FFmpeg preview stream for regular forward source-time windows, keyed by source revision, output dimensions, pixel format and timestamp step. Decode continuously from a preceding keyframe, select held frames at the exact requested clock, and publish each ready frame immediately. Reuse the stream across adjacent window requests instead of spawning/seeking again. A discontinuous seek, source change, quality/geometry change, reverse/irregular speed mapping or decoder failure starts a new generation/falls back to exact sampling. Never label an old timestamp as the requested frame.

Backpressure bounds resident decoded frames and queued raw data. Pause pipe reading when enough frames are ahead; do not decode the whole movie, retain one decoder per layer without limit, or let thumbnails evict the active playback window. Keep at most two active derived decoders, background work yielding to foreground. Idle streams must relinquish slots/resources; clearing/relinking retires only owned streams. Stream errors must be surfaced with bounded stderr and owned child teardown. Software remains the reliable fallback.

Hardware decode is an optimization to measure, not a magic flag. FFmpeg documents that GPU-to-CPU frame copies can erase hardware gains. Apple's native CVPixelBuffer/Metal path is the longer-term route to GPU-resident decode, but this sweep must not falsely call a rawvideo pipe zero-copy. A future platform adapter should slot behind the same timestamped frame/source contract without changing the document format or registry.

### Presentation and UI scheduling

Readiness checks request/look up exact frames without constructing a candidate document on every miss. Clone/synchronize/commit once a complete presentation is available. Decode completions are presentation revisions only; no authored history or transport restart. Pending work uses a source/document generation and bounded latest request. Preserve exact manual seeks and export.

During play, keep transport/timecode/playhead driven by the audio/monotonic clock. A held preview frame remains identified as buffering/loading rather than pretending to be current; drop obsolete intermediate requests to catch up without an unbounded queue. Preserve end/loop/stop/seek semantics, the audio master clock, variable-delay Frame Animation and reverse guards. Throttle/cancel auxiliary filmstrip work while playing, reuse already-cached thumbnails/source status, and avoid invalidating static UI content for every frame. Retain GPU layer composition and color management; measure CPU fallback separately.

## Removing fixed 64 MiB editing limits

The user request is a capability change, not permission to allocate without bounds. This sweep replaces fixed 64 MiB product rejections with checked estimates and the existing user-configurable Preferences → Performance → Memory Usage allowance. Errors must name the requested allocation and configured allowance and leave the document unchanged. Large loops run as cancellable background edit jobs, reporting progress and committing one undo transaction only on success. No global memory preference or private state is modified as a side effect of installation.

| Current occurrence | Treatment |
| --- | --- |
| Frame Animation/Video conversion property snapshots | Remove 64 MiB rejection; checked estimate against configured Memory Usage, fallible reservation, background/cancel checks and atomic commit. Preserve source/owned-key/snapshot semantics. |
| Make Frames From Layers | Same policy; avoid repeated unnecessary document/materialized-pixel copies; preserve visibility and original layers/Smart sources. |
| Tween frames | Same policy; retain frame/count/key limits and checked arithmetic, cancel safely, one history step. |
| 64 MiB thumbnail partition within the 256 MiB media cache | Replace fixed partitions with an adaptive shared budget derived from Memory Usage and physical RAM. Categories can borrow idle space; eviction/backpressure regulates optional cache data, not media import/edit capability. |
| Recording frame encode/decode/archive 64 MiB checks | Replace with geometry/codec/format-derived and configurable resource limits where the shared recording interfaces permit; retain allocation/decompression validation. Recording stays off by default. Record any distinct format limitation honestly. |
| 64-megapixel Smart affine footprint | This is a pixel-area guard, not 64 MiB. Preserve finite/coordinate/overflow validation; replace fixed product-size rejection only alongside a real renderer allocation preflight/tiled strategy. Never permit an unbounded dense warp as a consequence of renaming a constant. |
| 64 MiB reader chunk/preallocation | Keep: a streaming chunk or initial capacity is not a document/media size limit. |
| Preset, capture and unrelated import quotas | Inventory and distinguish; do not silently change security/input semantics to fix playback. Any retained product limit must be listed, rather than claiming every 64-number was removed. |

### Adaptive resource policy

Use the existing Memory Usage preference (8 GiB default) and detected physical RAM to derive retained media capacity. On this 32 GiB host, a conservative derived-cache target can use 2 GiB from the existing 8 GiB allowance instead of a fixed 256 MiB. This is an evictable cache target, not total application RAM or extra entitlement beyond document/history/GPU allocations. Protect foreground frames and shrink/reclaim optional thumbnails/waveforms first. Expose actual accounting and effective limits in diagnostic status. Changing the memory preference updates future reservations; retain current live reservations until safely released and evict gradually.

Document-authoritative pixels, history, GPU textures, transient decoder buffers, Smart render maps and derived caches need separately accounted costs; do not add these nominal budgets and call the sum a measured process cap. Continue checked dimensions, finite values, decompression limits, explicit errors and cancellation. A full out-of-core document/Smart graph with scratch-backed tiles is a larger architecture step; document its interfaces and current limits without pretending that a higher constant implements it.

## Ordered implementation and ownership

Implementation begins only after this plan and the fresh GPT Image reference are saved/inspected and the live issue is updated. No approval pause is required.

1. Primary: confirm recording/source evidence, plan, design and issue update; define shared resource/decoder interfaces.
2. Primary: identity conversion and cached transfer fast paths, persistent/early-publish preview decoding, generation-aware readiness, adaptive media accounting and measured diagnostics. One owner for shared engine integration.
3. Independent bounded worker: animation operation limits/background transactions, selected recording/native-format limits if safe, with exact untouched-source/undo contracts. No builds/Git mutations.
4. Independent bounded worker: transport/playhead and auxiliary timeline work scheduling, preserving typing, drawing tools, native seek and audio semantics. No builds/Git mutations. This can proceed alongside step 3 after shared interfaces are fixed.
5. Primary: integrate and run only the focused data/resource/performance evidence required for this severe playback failure and changed memory/undo boundaries. No broad suites, CUA, app capture, reviewer agents or private-artwork upload.
6. Within delivery: one production app+CLI build from exact source, benchmark that same binary before final artifact verification, package/verify once, push/release/tag and atomic normal-app/CLI install, then documentation-only receipts and scheduler no-replay checks. Preserve the running session and rollback copy. No dev app, watcher or build-triggered publication.

## Performance and correctness acceptance

Use disposable 1920×1080/30 fps and 3840×2160 fixtures with identifiable frame content and tagged SDR H.264, plus the existing exact-time/VFR, proxy/original, linked audio, nested Smart and undo boundaries as needed. Record host, codec, dimensions, frame count, cold startup, sustained output/presentation intervals, p50/p95, misses/drops, decoder process launches, retained bytes and source hashes. Compare the old conversion/batch path against the new path under the same fixture/hardware where feasible. Tiny 64×64 evidence cannot establish 1080p/4K playback.

The immediate 1080p goal is sustained 30 fps decode/presentation preparation with warm p95 within 33.3 ms and no per-frame process launches. 4K results must be reported, not inferred; lower preview quality may be explicit while authored/export quality stays original. UI event/paint ≤16.7 ms and native audio/display smoothness require a real native interaction sample; headless decode results cannot satisfy those gates. Do not call the product CapCut-speed until comparable native workflows are accepted. A result that only moves the playhead faster is a failure.

Resource evidence must cover an animation operation exceeding the former 64 MiB threshold (actual content above it), cancellation/undo atomicity and a too-large request giving a configured-budget error without partial mutation. Identity conversion must preserve encoded bytes and copy-on-write behavior in supported depths, and stale stream completion/relink/clear must not publish old source frames. Export remains original-source and uses the same time mapping. Mark implementation, compiler evidence, measured core performance and user-native acceptance separately.

## Primary references

- [FFmpeg hardware acceleration and stream options](https://ffmpeg.org/ffmpeg.html): hardware availability and GPU/CPU-copy caveat, persistent process input/output semantics.
- [Apple: Decode ProRes with AVFoundation and VideoToolbox](https://developer.apple.com/videos/play/wwdc2020/10090/): accelerated reader/decode integration and CVPixelBuffer/Metal lifetime/texture-cache design. This informs a future platform adapter, not a claim that the rawvideo path is GPU-resident.
- Existing [timeline specification](video-timeline-2026-11-01.md), [0.20.0 architecture record](video-timeline-completion-2026-11-01.md), source/format/command contracts and recorded shortcut inventory remain authoritative for PhotoStore behavior.
