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

1. Continue the post-release pass after the completed 0.12.0 recording-default milestone. Source/tag `0ac6e812f1ded55e9a12c3a1fa0230c8fdbe3c6d` is published and installed; targeted New Document confirmation passed. Prioritize the unresolved unsaved-document closing/recovery reliability limit before non-video opacity stopwatches/key tracks, save-frame persistence, full transform/style and video property-track migrations, and linked external PSD fidelity.
2. Deepen Smart Objects with contents editing, shared contents identity, smart-filter editing, linked-source workflows, and faithful PSD acceptance.
3. Improve brush behavior and interaction: pressure/tilt/rotation dynamics, smoothing, presets, tablet edge cases, deterministic replay, and responsive large-document painting.
4. Harden PSD and command coverage around the workflows PhotoStore promises. Add focused fixtures and measurements when a feature is implemented; avoid claiming parity before it is measured.

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

The action-recording and animation/smart-mask releases are historical: `photostore-action-recording-2026-10-11`
and `photostore-animation-smart-masks-2026-10-11` were published and installed locally. The current
installed release is the 0.11.0 `photostore-smart-contents-2026-10-11` prerelease. Broad
native visual QA and UI parity remain unverified.
The old-workspace native-save issue remains a secondary reliability gap; the old process and artwork
were preserved, but the attempted recovery target did not appear.

The audio/clip source milestone is pushed, compiler-verified, packaged, and retained in the current
0.11.0 install. Its remaining integration status is tracked in [audio-timeline.md](audio-timeline.md);
no broad UI or acceptance claim is made. The current installed source/tag is
`3ebf38f77bd55bb57fc6c0f3cea3e9ead04667b9`.

Smart Filter Masks and the New Document Fix are implemented, compiler-verified, packaged, and
installed in the historical 0.8.0 source. The targeted New Document confirmation passed. The
0.9.0 release adds mask transfer, Channels, and keyframe control repairs; broad UI acceptance
remains unverified. See [smart-filter-masks.md](smart-filter-masks.md).

## CI and release follow-up

The inherited workflow recipes are retained for reference but are manual `workflow_dispatch` jobs only. Automatic push, pull-request, scheduled performance, and deployment triggers are disabled while the independent repository is established. A future PhotoStore CI pass should choose its own checks, release signing, artifact storage, and documentation host before enabling automation; no original-domain deployment is configured here.

## Working constraints

Keep the engine headless, the UI thin, and the document format explicit. Preserve MIT/Apache licensing and asset provenance. Use one Cargo build worker and the shared target directory on development machines. Heavy builds and discretionary QA require an explicit user request.
