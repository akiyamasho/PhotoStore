# Video timeline completion: source and media architecture (2026-11-01)

**Status:** Implementation changes for the current #23 scope are integrated in the working tree. Five focused engine and two format boundary checks passed; the production build is pending. This document records implemented architecture and its limits; it does not claim a successful build, native acceptance, benchmark result, or published 0.20.0 release. Issue #23 remains open for user acceptance.

- Earlier user-facing research/design: [timeline workflow proposal](video-timeline-2026-11-01.md)
- Product-level status: [timeline workspace](../timeline-workflow.md)
- Candidate release receipt: [0.20.0](../releases/0.20.0.md)
- GPT Image references and provenance: [design README](../design/2026-11-01/README.md)

## Scope and design boundary

This pass completes the source and media architecture around the timeline: external source identity, media insertion/relinking, exact time mapping, playback presentation, bounded derived caches, optional disk proxies, audio output, and editing/export behavior. It builds on the existing shared document, command registry, and native UI. It does not introduce a parallel project database or second timeline renderer.

The generated [workflow image](../design/2026-11-01/timeline-completion/workflow.png) is a synthetic GPT Image design reference. Its prompt, provenance, and inspected limitations are in that directory. It is not a running application capture. The cache label does not describe total process memory, proxies are never used for export, and the audio/time labels do not establish hardware playback quality.

## Implemented source architecture

### Source catalog and clip instances

- Documents persist a catalog of external `MediaAsset` records with stable document-local IDs, a revision, path, file-size/modification fingerprint, and probed video/audio metadata. Video layers, audio clips, and linked Smart Objects can refer to the same catalog source while retaining their existing editable instance data.
- A visual instance retains its typed stable `LayerId`; audio clips retain their instance IDs and explicit `linked_video` association. A shared source ID does not link every placement together. This reuses the existing atomic clip/link commands instead of introducing a second link-group authority.
- Source listing, sync, relink, and media insertion are command-registry operations. A source can report online, offline, changed, or unverified. Missing media keeps its identity and reference visible. Sync refreshes compatible fingerprints/metadata; timing or geometry changes require explicit relink confirmation. Relink invalidates dependent proxies and preview caches.
- Import and placement preserve the distinction between source and instance. Placing a source more than once does not create an unrelated source record. The Layers/document hierarchy remains the compositing authority; arranging timeline clips does not silently reparent or flatten layers.
- File fingerprints use path, byte length, and modification time. They are source revision metadata, not a cryptographic content hash, and cannot detect every same-size, same-timestamp rewrite. Explicit source sync/relink is the refresh boundary.

### Native format compatibility

The native archive writer is version 3. Readers migrate versions 1 and 2, bind existing external movie/audio/linked Smart paths into catalog IDs, and preserve legacy paths and timing. Version 3 persists source records, rational rate metadata, and affine frame snapshots. Older PhotoStore readers reject v3 archives; retain an older copy when interchange with an older installed app is required. The catalog is capped at 4,096 sources and validates references, metadata bounds, and finite affine values.

### Time mapping and playback freshness

- Project frame rate can retain a reduced rational numerator/denominator alongside the legacy `f32` compatibility field. Legacy timelines without a valid matching rational continue to derive a rate from that field. This is additive document data, not a claim that every source timestamp or operation has migrated to one universal rational clock.
- Source metadata records rational frame rate and, when available, stream time base and color tags. Shared source-time helpers map project frames through clip placement, source trim, and playback speed. Preview and export use presentation time instead of independently rounding through a nominal integer frame rate.
- Timeline presentation requests the exact desired movie frame and holds presentation until that request is ready. Generation and consumer checks prevent an older seek/decode completion from replacing a newer requested frame. Preview decode completions are render updates, not document edits, so they do not rebase the transport clock.
- Current-frame requests are prioritized and coalesced per explicit consumer. A bounded exact-time window API batches up to eight uniformly spaced timestamps through one decode operation; irregular requests fall back to exact single-frame requests. Movie metadata is bounded. Active decoders are not forcibly terminated when a seek arrives; stale results are ignored.
- These mechanisms address the identified starvation path in source, but there is no measured FPS, latency, or hardware performance claim until build and fixture/hardware acceptance are recorded.

### Retained cache and worker limits

The engine accounts retained derived timeline media with shared RAII reservations. The UI reserves cached thumbnail textures through the same budget:

| Retained derived data | Limit |
| --- | ---: |
| Movie preview frames | 160 MiB |
| Timeline thumbnails | 64 MiB |
| Audio waveform bins | 32 MiB |
| Shared total | 256 MiB |

Heavy derived-media work is limited to two active decoder slots. Background decode work is capped at one active slot and yields to waiting foreground frames. Movie, thumbnail, and waveform queues and caches are bounded. Reservations account retained cache allocations, not document canvas/layer surfaces, transient decoder buffers, allocator overhead, GPU textures outside the registered UI cache, or graphics-driver memory. Therefore 256 MiB is not a total application-memory limit. Disk proxies are not a RAM-limit workaround: decoded proxy surfaces expand to the source canvas dimensions to preserve geometry.

### Optional disk proxies

- Proxy generation is opt-in and defaults disabled. The derived proxy cache has a configurable limit with a hard maximum of 10 GiB. The timeline menu can choose Original, Half, or Quarter preview quality and create or clear Half/Quarter proxies.
- Proxy files are derived H.264 media stored in the platform cache, keyed by source revision metadata and quality, with a manifest and atomic finalization. Encoding preserves source presentation cadence where supported and maps source audio when present. The original source is never replaced or deleted. Generation is cancellable, bounded by the configured disk limit, and reports failures.
- Preview uses a proxy only when enabled, selected, and matched to the current source metadata. Missing, stale, or invalid proxies fall back to original-source preview. Proxy decode output is scaled to source canvas dimensions so the layer footprint does not shrink.
- Export APIs always resolve the original source. Proxies never substitute for missing originals and are not used as export input. The UI exposes cache status and explicit clearing.
- The same original-source rule applies to non-native exports (including PNG/PSD), video/Smart rasterization, merge/flatten/stamp materialization, and clipboard copy. Native `.pcraft` saves remain possible while a source is offline; they retain source references and the last persisted preview rather than replacing the original path with proxy pixels.
- The limit covers owned proxy directories, not unrelated caches or a reservation of free disk space. Stress behavior under disk exhaustion, concurrent external file replacement, and all supported codecs has not been accepted in this pass.

### Audio playback

- Native output uses CPAL when the default device supports 48 kHz stereo `f32`; a bounded 500 ms PCM ring buffer feeds the device callback. The callback's consumed-sample count is the playback clock. Underruns output silence without advancing content time, and audio is not fully decoded into memory.
- When that native configuration is unavailable, the streaming ffmpeg/ffplay path remains available with monotonic timing fallback. Reverse ping-pong audio is explicitly unsupported and reports status rather than promising synchronized sound.
- Device discovery, callback stability, underrun behavior, drift, and fallback behavior have not received native hardware acceptance in this pass.

### Smart Objects, tracks, and editing

- Animated affine Smart Object placement can be baked into editable frame content through the explicit conversion path. Perspective/warp data is outside that affine bake and remains unsupported/guarded.
- Video, image, and audio lanes share the timeline's source/instance model. Track virtualization limits row painting to the visible region; source cards and filmstrip/waveform work are bounded and cached. Waveforms come from decoded audio envelope data, never placeholder bars.
- Ripple move and explicit Ripple Delete use atomic registry operations, preserve linked timing/key data where supported, respect locks and bounds, and create one undoable transaction. Ordinary delete behavior remains unchanged unless the user selects the explicit ripple action.
- Video export uses the shared source-time mapping and original media. Output is tagged Rec.709 SDR by default; the current decoder path uses an 8-bit RGBA intermediate. Unsupported HDR transfer/primaries fail until an explicit supported tone-map path exists; they are not silently clamped. H.264 MP4 requires an explicit background when the composite has transparency. MOV/WebM alpha depends on the selected supported encoder path. Container names do not promise support for every codec.

## Source ownership map

| Area | Main implementation files |
| --- | --- |
| Persisted source catalog, stream/time metadata, timeline rate | `crates/doc/src/video.rs`, `crates/doc/src/lib.rs` |
| Native serialization and migration defaults | `crates/format/src/manifest.rs`, `convert.rs`, `migrate.rs` |
| Source listing, sync, relink, and typed insert | `crates/engine/src/media_asset_cmds.rs`, `media_insert_cmds.rs` |
| Shared source-time evaluation, media decode/export, color handling | `crates/engine/src/video_cmds.rs`, `media_backend.rs` |
| Resident cache accounting, concurrency, proxy lifecycle | `crates/engine/src/media_budget.rs`, `movie_cache.rs`, `audio_waveform_cache.rs`, `media_proxy.rs` |
| Native audio sample-clock output | `crates/engine/src/media_audio_output.rs`, `audio_cmds.rs` |
| Edit commands and atomic ripple behavior | `crates/engine/src/timeline_editor_cmds.rs` |
| Native timeline, source cards, proxy controls, presentation | `crates/ui-egui/src/timeline_ui.rs`, `video_clip_ui.rs`, `audio_ui.rs`, `timeline_playback.rs` |

These files describe ownership points, not a claim of complete feature parity in every shell. User-visible document mutations stay on the shared command path.

## Verification and acceptance still pending

Five unique focused engine checks and two format checks passed in the primary-owned run/retry; see the [release evidence](../releases/0.20.0.md). The remaining evidence is:

- Format migration and native roundtrip passed (2 focused checks); see the release evidence.
- The required production compiler/build and its exact source/artifact receipts.
- Native UI inspection at representative window sizes; relink, mode, playback, and interaction acceptance; audio device testing; or installed-release export comparison.
- Measured decode latency, playback FPS, memory behavior under long sessions, or performance improvement over the prior build.

The design image is not a screenshot. The 0.19.0 receipts are historical and do not verify these source changes. Record tests/build evidence and artifact SHA in the candidate release receipt only after the primary completes those steps. Issue #23 remains open for final user native workflow acceptance.
