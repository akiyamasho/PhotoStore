# PhotoStore roadmap

This is the working roadmap for the independent PhotoStore product. It records the implemented foundation and the next product work. It deliberately avoids percentage claims until PhotoStore has its own measurements.

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

1. The current PhotoStore development version remains 0.16.0 and the Position animation pass is documented in [position-animation.md](position-animation.md). It uses the separate `PhotoStore-dev-position-animation` executable, settings, recovery process, and bundle ID `ai.photostore.app.dev.position-animation`, with no registered file associations; production and consolidated goal applications remain untouched. Engine and UI are implemented and root-reviewed, so two of three milestones are complete. Delivery is 0/4: the combined compiler job is waiting on the shared Cargo lock, with log `/private/tmp/photostore-position-animation-check-20261011.log`; this is not a passing result. No package, source-SHA, install, runtime, or visual claim is made. A public inspection currently has no document ID; root may make a necessary fix after the compiler terminal.

2. The current 0.16.0 development pass is on `/private/tmp/PhotoStore-video-opacity-20261011`, branch `codex/video-opacity`: independent owned video-opacity engine/native and timeline-UI source work is implemented; the coupled legacy XY/opacity behavior remains untouched and the ignored-alpha editor is hidden whenever an owned track exists. The baked source is `d66ddbcbed3989f66cb568b1f6d8f7f30fc9d464`. The combined compiler passed in 45.07 seconds with exit 0; log `/private/tmp/photostore-video-opacity-check-20261011.log` records 7 engine warnings and 1 existing UI warning with no errors. The development package completed in 6m13s and the candidate is installed at `/Applications/PhotoStore-dev-video-opacity.app` with bundle ID `ai.photostore.app.dev.video-opacity` and executable SHA-256 `597d2e4da6e32cbaf12f0bcfb6db592ac70dc5dfbcdd9670f82250d0bcad89c6`; installer verification matched the hash and confirmed signature, ARM64 architecture, no document association, no goal process, no default configuration, and no other applications touched. The focused native runtime receipt passed with 42 RPC responses, including one expected rejection (`id18`, `timeline.addKeyframe` with owned opacity) and 41 successful responses, plus clean quit/listener closure; screenshot, release, publication, promotion, and production install remain pending. The development branch checkpoint is prepared on `codex/video-opacity`. First enable preserves the legacy opacity curve and dense snapshots; sparse unique curves migrate Linear, while duplicate/dense curves use bounded exact integer-frame Hold migration with raw interpolation before integer clamping, restoring the legacy/static value at frame extent after the dense extent when the extent is at most 999,999. Candidate identity is `PhotoStore-dev-video-opacity` with bundle ID `ai.photostore.app.dev.video-opacity`. Full Photoshop Smart Object/PSD parity, audio acceptance, and visual interaction acceptance remain unverified.
3. Deepen Smart Objects with contents editing, shared contents identity, smart-filter editing, linked-source workflows, and faithful PSD acceptance.
4. Improve brush behavior and interaction: pressure/tilt/rotation dynamics, smoothing, presets, tablet edge cases, deterministic replay, and responsive large-document painting.
5. Harden PSD and command coverage around the workflows PhotoStore promises. Add focused fixtures and measurements when a feature is implemented; avoid claiming parity before it is measured.

The animation and Smart Object pass now preserves embedded child timelines, advances animated
sources during parent playback, bounds decoded and flattened source caches, and exposes an
undoable Smart Object transform reset. Persisted audio tracks and clips, linked movie audio,
mute/volume/fades, clip timing edits, shared preview/export, and streamed playback are now present
in the pushed source. Remaining audio gaps include sparse-track handling, exact output latency and
master-clock synchronization, and arbitrary split-segment replacement envelopes;
transitions, stopwatches, transform tracks, clip groups, and
full external-format export fidelity remain open. Eager decoded movie surfaces are capped at
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

The action-recording, animation/smart-mask, and Smart Contents releases are historical. The current
canonical installed release is the 0.12.0 `photostore-recording-default-off-2026-10-11` prerelease;
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
