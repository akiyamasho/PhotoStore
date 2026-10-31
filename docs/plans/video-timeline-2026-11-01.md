# Video timeline workflow: research, design, and proposal (2026-11-01)

**Status:** The initial research/design pass was followed by explicit user authorization to implement all open issues and enhancements, push, tag, release, and reinstall the normal PhotoStore app. The 0.19.0 pass integrates the existing major overhaul with additional timeline layout/editing/cache improvements. The generated image remains a design reference, never a runtime capture or manual acceptance. Implementation and delivery evidence is recorded in [0.19.0](../releases/0.19.0.md). Proposed architecture and performance targets below remain distinguished from shipped behavior.

- [Generated design concept and provenance](../design/2026-11-01/README.md)
- [Timeline workspace status and prior implementation evidence](../timeline-workflow.md)
- [Prior implementation plan and shortcut accounting](capcut-9.5-shortcuts.md)
- [User-facing design principles](../ui-design.md)

## User request and evidence boundary

The user reports that the supplied timeline screenshot still looks unlike the clean Photoshop/CapCut-style workflow they want. They asked to research and specify a well-optimized workflow for MP4 video, images, sound, layers, and Smart Objects. They want a design generated before implementation. Their screenshot shows frame 1/30, a millisecond delay control, a playhead slider, large property forms, and a separate `Video Clips` region. The screenshot does not identify the app build, active timeline mode, or capture time; those details are Unknown.

The screenshot has been added to issue #23 only as a privacy-cropped copy. It omits the private-artwork strip and the bottom document/status region. It is user-provided evidence of the visible state, not proof of its source build or the current code path.

The existing design proposal is an original PhotoStore interface. It is a GPT Image concept, not a running-app screenshot and not proof that any listed control, media behavior, or performance level exists. See the [design record](../design/2026-11-01/README.md) for the prompt and provenance.

## Source-state audit and interpretation

At the inspected repository source, `crates/ui-egui/src/timeline_ui.rs` routes `TimelineMode::Video` to `video_clip_ui::workspace`; its separate `FrameAnimation` branch still contains frame delay and playhead controls. `Timeline.fps` is currently `f32`. The current source therefore has distinct renderers. A screenshot containing frame delay controls is consistent with Frame Animation, an older runtime, or another state; the available still cannot distinguish these. It is not sound to conclude that a particular installed runtime selected the wrong mode.

The Video renderer already contains compact editing controls, a fixed track gutter/ruler direction, selected-clip property/keyframe UI, a media bin and visual-track paths. The audit for a future implementation should reuse these systems, including existing `Timeline`, `layer_ranges`, `visual_tracks`, command registry, movie preview cache and audio waveform cache. The prior plan and `timeline-workflow.md` retain the implementation history. Do not create a second renderer, project database, media authority, or source-specific parallel timing model to reproduce the concept.

### Mode clarity

- Make `Video Timeline` and `Frame Animation` explicit, named destinations with a visible active-mode indicator in the relevant switcher. Do not infer a mode from whether a layer contains video, sound, or a frame stack.
- Keep the two modes' existing data and renderers distinct. Switching views must never silently delete, flatten, or convert data. Any actual conversion remains an explicit command with a preview/guard where information may be lost.
- Keep frame delay controls in Frame Animation. In Video Timeline, show project frame rate and timecode and put per-frame duration under frame-animation controls.
- Identify source/runtime through existing About/build information when diagnosis is needed; do not fill the editing canvas with version diagnostics.

## Reference research

The following are primary product documentation references. Their documented workflows are evidence about those products, not specifications that PhotoStore must copy pixel-for-pixel.

- [Adobe: edit video clips and audio in the Photoshop timeline](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/organize-and-manage-video-groups/edit-video-clips-and-audio-in-the-timeline.html) (updated 2026-10-04) documents video groups, sequential clips, edge trimming, playhead splitting, clip movement, and independent audio/fades.
- [Adobe: animate a poster with text and images](https://www.adobe.com/learn/photoshop/web/animate-poster-text-images) documents editable layer/Smart Object content and timeline transform animation with keyframes.
- [CapCut: keyframes on PC](https://www.capcut.com/help/keyframes-in-capcut-pc) documents selecting video/image/text and setting Transform keyframes in the inspector.
- [CapCut: PC professional video editor](https://www.capcut.com/resource/pc-professional-video-editor) documents proxy editing with lower-resolution working media and full-resolution export, along with preview quality, hardware options, split and snapping. These are product descriptions, not independent performance measurements; no numerical speed claim is adopted here.
- [CapCut 9.5.0 macOS shortcut observations](capcut-9.5-shortcuts.md) are the previously recorded local profile, not newly repeated observations and not universal defaults. The record contains 107 action/gesture rows: 76 assigned and 31 unassigned.

Keep observed product behavior, the user's screenshot, existing PhotoStore code, and proposed PhotoStore design decisions labelled separately. Use no third-party UI assets, product artwork, or code.

## Proposed layout and responsive behavior

Treat the 1440×900 and 1280×800 logical-point canvas as initial design targets, not verified layouts. The generated 16:9 concept illustrates the hierarchy; its exact artwork/content is synthetic.

| Region | Initial target | Behavior |
| --- | ---: | --- |
| Editing toolbar | 32 px high | One row of common selection, split, undo/redo, snap, link and keyframe-pane actions; tooltips expose action and effective shortcut. Overflow holds less-used actions. |
| Ruler | 28 px high | Stays pinned while tracks scroll vertically; one shared time origin, playhead, markers and work area. Ticks adapt to zoom and project frame rate. |
| Track gutter | 180 px wide, resizable | Names, type badges and applicable eye/lock/mute controls. Remains fixed during horizontal time scroll. Color is not the only type/selection signal. |
| Timeline area | About 35–45% of window height, resizable | Shared vertical scrolling for headers and lanes; horizontal scrolling affects time content only. At common scaling, aim for 56 px video, 40 px still/image, and 48 px audio lanes; preserve legibility at accessibility scaling. |
| Inspector | 280–320 px wide when shown | Only selected-object fields and key diamonds. Selection in timeline and Layers panel stays synchronized. Hidden/collapsed on narrow windows before clips, crop controls, or inspector values are clipped. |

Avoid repeated `Video Clips`/`Layer tracks` headings, giant forms embedded below every layer, duplicate rulers, per-layer stopwatches, and unrelated frame-delay/playhead sliders in the video workspace. Keep the default keyframe pane collapsed. Let users resize/collapse the media bin and inspector; on narrower windows collapse the media bin and move secondary toolbar actions to overflow before squeezing timeline lanes. Maintain Japanese label legibility, accessible names, visible focus, useful tooltips, sufficient contrast, and keyboard access. These are design requirements for later manual review, not claims of verification.

## Document, source, track, and clip model

Keep identity layers explicit:

1. **Source asset**: proposed explicit stable source identity and probed stream/metadata, referenced by clip instances. `AssetId` is a design name here, not a claim that the current document model has that type. Import inspects MP4 container and codecs and returns actionable supported/unsupported diagnostics. MP4 is a container, not a promise that every codec is supported. Images resolve to decoded surfaces; audio exposes bounded PCM reads. A missing file remains a visible offline source with relink action; do not substitute another file or discard the original reference.
2. **Layer/document object** (`LayerId`): remains authoritative for compositing and editable layer/Smart Object contents. Layer-stack order is not silently rewritten when the user arranges timeline clips.
3. **Clip instance**: stable editable timeline instance with explicit source offset, project placement, rate and range. Visual clips currently refer to layers; `layer_ranges` remains the canonical project interval. Existing `ClipPlayback`/`VideoPlayback` hold source-in/speed information. A new common clip record or rational-time representation would be a proposed versioned change, not a current data type.
4. **Track**: persistent visual/audio lane containing ordered clip instances, with kind/name/lock/mute. Current `VisualTrack` and `AudioTrack` use `u64` IDs; `AudioClip` also uses `u64` and its `linked_video` is `Option<LayerId>`. A new typed identity is future design. Splitting creates adjacent instances on the same track. Track membership must not reparent or flatten document content.
5. **Audio link group**: explicit stable relationship between video source audio and its corresponding audio instance. Current `linked_video` is a layer reference; a stronger explicit group is a future proposal. Preview must not double-play a stream because it is both embedded and extracted. Linking/moving/splitting, independent mute, extraction and relinking are explicit operations.

A Smart Object is an editable nested document, not a flattened thumbnail. Shared source contents need copy-on-write/revision semantics, while each instance keeps its own transform and time mapping. Nested child time maps to parent/source time exactly once through the shared evaluator. Any source revision invalidates dependent preview/waveform/proxy data by identity and revision.

Still images have a project-duration hold; movies remain bounded by source presentation timestamps and trims. Define visible source handles and clamp/error behavior for trim, split, reverse, speed changes and offline relink before implementing. Project frame rate is separate from source frame rate.

## Concrete interaction and media-format contract (proposal)

| User action | Proposed behavior and guard | Format/persistence contract |
| --- | --- | --- |
| Import and insert media | Import creates/reuses one source reference; Insert at Playhead is explicit. MP4 goes to a chosen sequential video row, image to a visual row, audio to an audio row. A new image placement defaults to a five-second hold; users can change it. Existing project timing is never reset on import or mode switch. | Retain source path/identity and probed stream metadata; preserve existing serialized timing. New typed IDs/timebase require separately reviewed migration. |
| Drop/move/overlap | Preview the exact row and affected clips before commit. Incompatible or locked targets reject with a reason; never silently reparent or change compositing order. A sequential-row collision rejects by default. Explicit magnet/ripple mode previews each affected instance and commits as one undo. Overlap requires a deliberate overlay track; transitions remain future work with no dead controls. | All timing/track edits go through atomic registry commands; one undo record per gesture. |
| Split and trim | Split only when `start < playhead < end`, preventing a zero-length segment. Preserve source offset and keys; map the cut to source time and choose the nearest audio sample with one deterministic rounding policy shared by both resulting audio halves. Preserve the media remainder/timebase. Trim is nondestructive and keeps source/key data outside the visible range. Extending an image's right edge extends its hold; movie trim is clamped to available source range with the bound visible. Negative/reverse rates remain guarded until preview/export agree. No pitch-preservation promise for speed changes. | Preserve PTS/timebase and source trim; do not flatten Smart Objects or bake keys to pixels. |
| Smart Object edit and relink | Double-click or Edit Contents opens the child document in a tab while retaining parent selection and time context. Relink checks compatible stream/duration metadata and shows a trim/mapping preview before replacing the source reference. | Smart source revision invalidates dependent previews; per-instance transforms and source timing stay editable. |
| Export | Show supported output formats/codecs only. Define conversion from tagged source color space through document working space into tagged Rec.709 SDR output/metadata by default. HDR-to-SDR requires an explicit supported tone map or fails visibly; never silently clamp. Preserve channel/sample metadata. Default H.264 MP4 delivery has no alpha; require an explicit background/composite choice or offer a backend-supported alpha-capable sequence/output. Convert audio gain from dB to linear and export with a 48 kHz sample clock by default while preserving source information. Unsupported codecs/missing sources fail with actionable errors; no stale preview substitution. | Export uses same frame/time evaluator as preview, exact work area, and synchronized audio; original source remains untouched. |

These are proposed PhotoStore decisions, not existing capabilities. Product format support must be verified against the actual media backend before exposing an option.

## Candidate code and persistence touchpoints

| Concern | Existing source to audit before implementation |
| --- | --- |
| Timeline/mode/ranges/tracks/playback data | `crates/doc/src/video.rs`; document fields in `crates/doc/src/lib.rs` |
| Native document format / timeline manifest conversion | `crates/format/src/manifest.rs`, `crates/format/src/convert.rs` |
| Registry timeline edit operations | `crates/engine/src/timeline_editor_cmds.rs`, `timeline_cmds.rs`, `video_cmds.rs` |
| Media capability probing/decode/encode | `crates/engine/src/media_backend.rs` and callers |
| Movie and waveform preview caches | `crates/engine/src/movie_cache.rs`, `crates/engine/src/audio_waveform_cache.rs` |
| Native workspace and track/inspector rendering | `crates/ui-egui/src/timeline_ui.rs`, `video_clip_ui.rs`, `audio_ui.rs` |

These are investigation starting points only. Confirm current ownership, persistence compatibility, and call paths before edits; do not infer a missing feature or a module's contract from its filename.

## Time, playback, seek, and export correctness

- Use integer project frames for frame-addressed edits, with half-open ranges `[start, end)`. Convert from source timestamps using a rational time base. MP4 may be variable frame rate; do not assume `frame_index / fps` is a valid source timestamp. Preserve source PTS and project frame mapping.
- The current `Timeline.fps` is `f32`; source seconds and playback speeds on `VideoPlayback`, `ClipPlayback`, and `AudioClip` are `f64`. Do not rewrite or reinterpret persisted fields in this design pass. A future rational/versioned migration must be backward compatible and audited before implementation.
- Centralize source/project time mapping for preview, audio, nested Smart Objects, scrub, and export. Reject NaN, invalid duration, unsupported/reverse rates, or bounds that cannot be represented with actionable errors. Do not let UI and export calculate different clip timing.
- Audio playback uses an audio sample clock; a monotonic-clock fallback must not accumulate per-frame floating-point drift. A seek supersedes old decode/audio requests by generation; stale completions cannot overwrite the latest frame. Buffer underrun may be shown but cannot mutate document timing.
- Export uses original sources unless the user explicitly opts into a derived proxy. Missing/unsupported media fails visibly with a useful report, never silently exports a stale preview frame. Work-area and audio sample bounds are exact. MP4 codec choices reflect actual capabilities; an attached audio stream stays synchronized when requested.

## Commands, editing behavior, and keyframes

All document-changing operations go through the shared command registry and are atomic/undoable. A drag keeps temporary preview state in the UI; one command commits on release. Lock, offline, and invalid-range errors explain why no change occurred.

- Clip move carries editable animation and linked media according to explicit link state. Trim retains off-range source and key data. Split preserves source continuity, creates the second clip on the same track, and remaps persistent IDs safely. Copy/paste preserves editable sources, Smart Objects, clips, links, and keys, remapping IDs and placement without unbounded cloning.
- Distinguish magnet, snapping, linking, and ripple. Snap targets (playhead, markers, clip edges) are visible. Magnet/ripple semantics state which track and following instances are affected. Group editing preserves hierarchy; Smart Object contents open in context and do not rasterize.
- Inspector is selection/type aware. Position, supported transform, opacity, timing/speed and relevant audio fields each use only the controls that type supports. A property diamond enables/sets a key at playhead; subsequent direct edits create/update keys. Previous/next controls navigate keys. Linear, Hold and supported ease curves are visible and actually evaluated by the engine. Selected-property keys can be moved, copied, deleted and batch edited as one history operation. Keep unsupported warp/perspective commands unavailable instead of presenting a fake keyable control.
- Document the transform pivot (currently planned source-center) and link behavior. Avoid conflicting global shortcuts while a text field, rename field, modal or shortcut editor owns input. Shortcut list, dispatcher and help reference must use the same command definitions; unresolved reference-profile rows remain explicitly unassigned/out of scope.

## Media and resource design

Reuse and audit the current caches; do not promise unlimited support or add full-file allocations to make the layout look responsive. `movie_cache` currently budgets 160 MiB of frames, with request queue 16 and thumbnail request queue 4. `audio_waveform_cache` currently allows 64 entries, queue 8 and 200,000 cached bins; each request accepts 16–4096 bins. These are separate current limits, not a unified resident-memory measurement. Before implementation, trace their actual allocation cost, invalidation, queue behavior, and consumers. The following unified 256 MiB split is a future proposal and must be reconciled with measured accounting; it is not an existing cap:

- One accounted 256 MiB resident derived-media budget shared across consumers: up to 160 MiB movie frames, 64 MiB thumbnails, and 32 MiB waveform bins, accounting actual pixel format/channels and allocation overhead. Compare against the separate current limits above before changing them; it is a proposal, not an implemented cap.
- At most two low-priority decode jobs globally: visible playhead frame and audible audio first, visible thumbnail second, prefetch last. Scrubbing coalesces to the latest request (one active plus one latest pending), not an unbounded seek queue. Generation-cancelled work releases buffers promptly.
- Waveform demand remains bounded to visible clip pixels. Current references include bounded waveform queue/cache; inspect before harmonizing. Playback PCM uses a bounded-duration ring, not the entire decoded movie/audio in RAM.
- Optional disk proxies are derived, opt-in, manual-cleanup, and bounded to a proposed 10 GiB configurable limit. Never replace or delete source media; export defaults to source originals.
- Cache keys include source identity/revision, PTS/timebase, requested resolution, pixel/color format, and project/render generation so stale preview content is never shown as current.
- Avoid cloning the full document per repaint/frame; prefer immutable/shared snapshots and revision-keyed view models. Virtualize track rows and horizontally visible clips via interval lookup; expected work scales with visible items plus lookup cost. Repaint on input/completion, not by idle polling. Export gets a separately bounded pipeline that respects global resource limits.

Proposed acceptance targets for a later implementation/device record: UI event+paint p95 ≤16.7 ms excluding media decode, warm cached scrub response ≤50 ms, first cold frame ≤250 ms for a named supported fixture/hardware, and no memory growth across repeated scrubbing. These are goals, not current measurements or guarantees. Do not claim superiority over Photoshop/CapCut based on these targets.

## Delivery order and later acceptance

The user subsequently authorized implementation and release. Use the following dependency order when a phase changes the relevant subsystem; reuse the already implemented document, edit, and media systems rather than replacing them merely to follow a diagram:

1. **Mode identity and layout:** explicit mode switch/status, fixed ruler/gutter and responsive workspace; reuse current Video/Frame renderers.
2. **Media and timeline semantics:** source/clip/track identities, rational PTS mapping, linked audio, offline source and transaction-safe edit commands. Must precede polish that suggests unsupported editing.
3. **Playback/resource pipeline:** generation-safe decode, bounded memory/queues, visible scrub status, cache invalidation and virtualized rendering.
4. **Persistence/export and acceptance:** versioned migration, exact work-area/audio export, original-source behavior, Smart Object nested timing, documented manual acceptance.

Dependencies are one-way in that order for cross-cutting time/storage semantics; UI can refine against phases 1–2 but must not invent its own timing authority. Touchpoints likely include `crates/ui-egui/src/timeline_ui.rs`, `video_clip_ui.rs`, `audio_ui.rs`, engine timeline command/evaluator/cache modules, and document persistence. Confirm actual ownership and modules from code before any edits.

| Area | Source evidence now | Proposal / verification status |
| --- | --- | --- |
| Video and Frame Animation modes | Separate branch/renderers in current source. | Clear switch and mode badge proposed; exact source of user screenshot unresolved. |
| Compact Video workspace | Toolbar/gutter/ruler/inspector/key-pane paths exist in Video renderer. | User reports screenshot remains unacceptable; generated image is design-only. Manual acceptance not performed. |
| Media preview/waveform | Movie and waveform cache APIs are used. | Shared budget, queue priorities and performance targets are proposals; not measured. |
| Track/clip timing | `Timeline`, `layer_ranges`, visual tracks and registry commands exist. | Stable source/instance/link model and rational VFR mapping need audit/design and any required migration. |
| Smart Objects | Existing editable Smart Object and transform code exists. | Consistent nested source time/link semantics remain to be specified and accepted. |
| Export | Existing video render paths and prior work documented in timeline workflow. | Exact originals/audio/work-area/missing-source acceptance for this request is outstanding. |

Later manual acceptance should use disposable synthetic projects and supported known media fixtures, cover both modes, multiple adjacent clips, image/still holds, audio synchronization/waveforms, Smart contents, undo, keys, zoom/scroll/narrow layouts, offline relink, save/reopen and export. Capture reproducible device/build/media details. Use targeted tests only for actual implementation boundaries. The initial plan ran no tests, builds, screenshots, or manual acceptance. The later release receipt records the actual compiler/artifact evidence; computer use, screenshots, and native manual acceptance are unavailable for this session.
