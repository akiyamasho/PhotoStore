# PhotoStore roadmap

This is the working roadmap for the independent PhotoStore product. It records the implemented foundation and the next product work. It deliberately avoids percentage claims until PhotoStore has its own measurements.

Current handoff and remaining priorities: [handoff.md](handoff.md). Current published receipt: [0.16.0 release notes](releases/0.16.0.md).

## Baseline

- The native desktop shell, headless CLI, automation/MCP entry points, and WebAssembly shell already share a Rust command engine.
- Layered documents, masks, adjustments, effects, type, vectors, brushes, PSD/PSB I/O, OpenRaster, flat codecs, and the native `.pcraft` format are present in the inherited tree.
- Smart Objects and smart filters have persisted document mapping, animated child timelines, linked and embedded source handling, transform reset, cache invalidation, and PSD mapping infrastructure; deeper editing and full external-format acceptance remain product work.
- The command-driven timeline foundation supports persisted video timelines and frame animation workflows, including timing, playback, layer states, frame stacks, and native UI/CLI integration.
- `FrameLayerState` snapshots position, blend, fill opacity, and effects, while `Timeline.property_frames` preserves editable property tracks; cumulative CFR conversion and configurable tweening are implemented in the timeline engine/UI (`crates/doc/src/video.rs`, `crates/engine/src/video_cmds.rs`, `crates/engine/src/timeline_cmds.rs`, `crates/ui-egui/src/timeline_ui.rs`).
- Optional Photoshop `.kys` import supports primary and alternate bindings, stable command IDs including Japanese labels, and preservation of PhotoStore defaults (`crates/engine/src/kys.rs`, `crates/engine/src/prefs.rs`, `crates/ui-egui/src/kys_import.rs`).
- Native FFmpeg movie probing, RGBA8 decode/export, asynchronous jobs, and timeline/movie UI integration are implemented (`crates/engine/src/media_backend.rs`, `crates/engine/src/video_cmds.rs`, `crates/ui-egui/src/movie_ui.rs`, `crates/ui-egui/src/timeline_ui.rs`).
- PhotoStore identity, executable names, configuration directories, packaging metadata, and icons are implemented independently of the inherited PhotoStore identity.

## Priorities

1. Deepen Smart Object workflows: shared/embedded/linked contents editing and identity, missing/relink/update behavior, nested timing and cache ownership, transforms/styles, Smart Filters, and external PSD acceptance. The 0.11 contents-edit reliability work is implemented; broader acceptance remains open. A linked child “Save Contents” currently embeds native content instead of writing the external source.
2. Continue timeline and media acceptance. Ordinary-layer and video Position/Opacity stopwatches are implemented; loop counts and onion skins are implemented. Remaining work includes full affine/style tracks, clip groups/transitions, arbitrary split-segment replacement envelopes, sparse audio, latency/master-clock synchronization, rational frame-rate persistence (currently `f32`), lazy long-video decode (eager RGBA8 surfaces capped at 512 MiB), and timing/UI acceptance.
3. Measure remaining Photoshop workflow families with PhotoStore-specific evidence: color/depth/HDR; layers/masks/transforms; filters/RAW; vectors/text; retouch/tablet; formats/print; actions/plugins; native UI and reliability. The 627/627 menu-liveness result measures command dispatch only.
4. Persist the APFS-safe packaging fix and add mounted-app inspection to release verification. The published artifact was regenerated and verified, while `package.sh` still creates an HFS hybrid image and `verify.sh` does not mount-check the bundled app.
5. If selected for a future pass, finish shortcut sorting/import-cancel/alternate editing and broader interaction acceptance. Recording remains off by default; deterministic replay, tablet pressure, and parity acceptance remain open.

The animation and Smart Object pass now preserves embedded child timelines, advances animated
sources during parent playback, bounds decoded and flattened source caches, and exposes an
undoable Smart Object transform reset. Persisted audio tracks and clips, linked movie audio,
mute/volume/fades, clip timing edits, shared preview/export, and streamed playback are now present
in the pushed source. Remaining audio gaps include sparse-track handling, exact output latency and
master-clock synchronization, and arbitrary split-segment replacement envelopes;
transitions, full affine/style tracks, clip groups, and
full external-format export fidelity remain open. Ordinary-layer and video Position/Opacity stopwatches are implemented in 0.15.0–0.16.0. Eager decoded movie surfaces are capped at
512 MiB, native range selection is currently command-parameter based, source rates are stored as
`f32`, and movie decode/export transfers RGBA8 pixels.

Required integration checks own compilation, the exact `.kys` API check, tiny MP4/MOV/WebM
checks, and one Smart appearance regression. Their evidence is recorded below, but this
roadmap still does not mark the current animation, media, or Smart Object work accepted or
complete as a whole.

The 2026-10-11 integration evidence is now available: all-targets Cargo check passed; the
external KYS API checks passed 3/3 and the KYS library checks 4/4; the 8×8 MP4/MOV/WebM smoke
passed 1/1 for colors, order, range, cancellation, and alpha; the Smart appearance regression
passed 1/1; and the post-repair engine check passed. The exact Photoshop fixture imported 152
primary rows, 3 alternates, and 51 nonempty tool keys, while unknown scripts, Bridge, Smart
Object, filtering/minimize/hide, and UI-only candidates were reported. This evidence covers
compiler and targeted function/integration behavior. It does not claim visual UI QA, Photoshop
parity, or complete PSD/external-format compatibility.

Historical snapshot (pre-0.16.0): the action-recording, animation/smart-mask, and Smart Contents releases are historical. At that earlier point, the canonical installed release was the 0.12.0 `photostore-recording-default-off-2026-10-11` prerelease;
the newer 0.13.0 `photostore-tool-shortcuts-2026-10-11` is installed side by side from source
`94751b66e9683aec52276fed789fcd26f39fce01`. Broad
native visual QA and UI parity remain unverified.
The old 0.11.0 unsaved-document process later disappeared after its canceled quit prompt and bundle
replacement; the on-disk PSD exists but was not opened or compared, so unsaved memory or recovery
is unknown.

The audio/clip source milestone is pushed, compiler-verified, packaged, and retained in the canonical
0.12.0 install. Its remaining integration status is tracked in [audio-timeline.md](audio-timeline.md);
no broad UI or acceptance claim is made. The canonical installed source/tag is
`0ac6e812f1ded55e9a12c3a1fa0230c8fdbe3c6d`.

Smart Filter Masks and the New Document Fix are implemented, compiler-verified, packaged, and
installed in the historical 0.8.0 source. The targeted New Document confirmation passed. The
0.9.0 release adds mask transfer, Channels, and keyframe control repairs; broad UI acceptance
remains unverified. See [smart-filter-masks.md](smart-filter-masks.md).

## CI and release follow-up

The inherited workflow recipes are retained for reference but are manual `workflow_dispatch` jobs only. Automatic push, pull-request, scheduled performance, and deployment triggers are disabled while the independent repository is established. A future PhotoStore CI pass should choose its own checks, release signing, artifact storage, and documentation host before enabling automation; no original-domain deployment is configured here.

## Working constraints

Keep the engine headless, the UI thin, and the document format explicit. Preserve MIT/Apache licensing and asset provenance. Use one Cargo build worker and the shared target directory on development machines. Heavy builds and discretionary QA require an explicit user request.

## Current production release — 0.16.0 (2026-10-11)

The immutable `v0.16.0` tag points to exact source `a87fd1049844a41d456cbdce9d5fb0fa5038ab3c`, published as [release v0.16.0](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.16.0) (release ID `409665038`) and installed at `/Applications/PhotoStore.app`. The ARM64 executable SHA-256 is `9bf8fea3b609532c6ef2e08959e17477166b4aa485701384e667945ab190faf6`; DMG SHA-256 is `396c880ec9b4c6a092a2dbee54b872e8f2936aac0f445758102e8ea5c143ccb8`; CLI ZIP SHA-256 is `5a6ab30230490c8b5e0c1b9df58126605a9b7cc96b0a28be9f3c46029ce143b5`. Mounted DMG app, CLI, signatures, full source, licenses, architecture, and checksums passed. Artifact-only APFS UDZO regeneration fixed synthesized HFS FinderInfo without source rebuild or retag. The app is ad hoc signed and not notarized. Previous bundle backup: `/Applications/.PhotoStore.backup.v0.16.0.1791723317.97155.app`; preferences, recovery, and artwork were untouched, and the app was not launched during promotion. See [release notes](releases/0.16.0.md) for preview provenance and [handoff.md](handoff.md) for remaining work. The full product backfill remains unfinished; no overall completion percentage is claimed.
