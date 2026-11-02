# Compact native video previews

The unpublished 0.20.8 native builds keep brush/pan input responsive, but eight overlapping 4K movies still fail continuous playback. Moving work to threads alone is insufficient. The RGBA path expands every decoded frame into dense tiles, then repacks those tiles into GPU buffers; the earlier comparison uploaded about 283 MB per eight-layer frame. Late clock jumps also make the raw pipes transport skipped frames. These are measured/inspected bottlenecks, not a CapCut comparison.

## Proposed representation and boundaries

Keep eligible opaque SDR 4:2:0 H.264 previews as full-resolution compact NV12 planes through the cache and recognized Normal GPU stack. Preserve existing timestamps, source references, placement and layer opacity. This is a representation change, not a lower-resolution proxy or a zero-copy claim. Other codecs, alpha, unsupported color metadata, high-bit-depth formats and generic effects retain the established raster path.

The foundational raster surface can hold an immutable NV12 frame with optional per-channel transfer ramps. Point/rectangle reads convert on demand. Generic tile readers lazily obtain the existing RGBA8 tile representation; mutations detach to normal raster tiles. Each surface clone gets its own initially empty raster projection while sharing compact planes, so materializing a document view does not inflate the retained movie cache entry. History accounting and UI fingerprints inspect physical allocations/identity without forcing raster projection. Serialization still emits the existing raster format; no document schema or original media changes.

Native preview and full-quality cache identities are separate. Blocking/original-quality operations continue to request the established RGB decode path. Compact frame/session reservations use their actual plane/raw-buffer sizes under the existing shared allowance; the maximum eight resident sessions and four heavyweight decoder slots remain unchanged. Eligible compact windows may retain current plus seven frames within that allowance; ordinary RGBA windows remain current plus two. Thumbnail work retains its separate bounded worker and pauses taking new jobs during native transport.

The GPU keeps seven source bindings plus the previous floating-point accumulator. It crops each NV12 source to an even-aligned source rectangle for the current transfer cell, maps it back through its integer document origin, and uploads Y plus interleaved UV. A stream source also supports ordinary tile-major RGBA and small sparse/clipped row-major paint, so drawing above videos does not force the movies back through RGB expansion. The stream uniform is 32 bytes plus seven 64-byte source records; the legacy eight-RGBA uniform remains unchanged. Unsupported GPU features retain raster fallback.

## Color and timing

Use original implementations of the limited-range [BT.601](https://www.itu.int/rec/R-REC-BT.601) and [BT.709](https://www.itu.int/rec/R-REC-BT.709) matrix equations, followed by the same supported source-transfer-to-working-SRGB ramps as the current decoder. Eligibility fails closed for full-range/JPEG 4:2:0, unsupported matrices/primaries/transfer, and unsupported chroma siting. A preliminary 4,096-point comparison of the owned 3840×2160 BT.709/limited/left-sited fixture against FFmpeg RGB had maximum one U8 code-value error with nearest chroma sampling. Final implementation still requires independent color/pixel evidence; the preliminary math comparison does not verify the app.

The existing one-frame-ahead document staging remains bounded and generation-checked. Staging does not publish, change document time/revision/history, or move audio. The due-time presentation commits the validated snapshot, and the native canvas worker composes it independently of input. Original file references remain authoritative for export and editable video/Smart Object data.

## Design and delivery

The built-in GPT Image [workflow reference](../design/2026-11-02/compact-video-preview/workflow.png) was generated and inspected before implementation; [exact prompt, provenance and discrepancies](../design/2026-11-02/compact-video-preview/prompt.md) accompany it. It is a mockup, not an application screenshot.

Implementation is split by Rust layer: raster representation; engine decode/cache; GPU stream packing/color; and primary-owned history/UI accounting/integration. Integrate before one exact-source production app/CLI build. Use the actual native callback/input workload, a paused staging-preservation boundary, and bounded independent color/GPU pixel checks before final artifact verification. No screenshots, private artwork, broad QA suite, developer bundle or scheduler replay. Failed native candidates remain evidence and do not satisfy completion.
