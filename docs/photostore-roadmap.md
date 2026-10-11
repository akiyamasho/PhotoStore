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

1. Continue the integrated audio/clip milestone described in [audio-timeline.md](audio-timeline.md): finish body/trim commit controls, reload-footage and split-segment replacement behavior, and exact audio latency/shared-clock acceptance. The work remains on `codex/animation-audio-backfill-wip` at `287f7c94c065d37d3bdc177c3e44f28719490e84`; compiler verification passed, but packaging and local install are pending.
2. Deepen Smart Objects with contents editing, shared contents identity, smart-filter editing, linked-source workflows, and faithful PSD acceptance.
3. Improve brush behavior and interaction: pressure/tilt/rotation dynamics, smoothing, presets, tablet edge cases, deterministic replay, and responsive large-document painting.
4. Harden PSD and command coverage around the workflows PhotoStore promises. Add focused fixtures and measurements when a feature is implemented; avoid claiming parity before it is measured.

The animation and Smart Object pass now preserves embedded child timelines, advances animated
sources during parent playback, bounds decoded and flattened source caches, and exposes an
undoable Smart Object transform reset. Audio descriptors are detected but are not persisted
tracks or playback; transitions, stopwatches, transform tracks, clip groups, speed/slip, and
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

The action-recording release is complete: `photostore-action-recording-2026-10-11` is published
and PhotoStore 0.7.0 is installed locally. Broad native visual QA and UI parity remain unverified.
The old-workspace native-save issue remains a secondary reliability gap; the old process and artwork
were preserved, but the attempted recovery target did not appear.

The audio/clip source milestone is not yet compiled, published, or installed. Its remaining
integration status is tracked in [audio-timeline.md](audio-timeline.md); no acceptance claim is made.

## CI and release follow-up

The inherited workflow recipes are retained for reference but are manual `workflow_dispatch` jobs only. Automatic push, pull-request, scheduled performance, and deployment triggers are disabled while the independent repository is established. A future PhotoStore CI pass should choose its own checks, release signing, artifact storage, and documentation host before enabling automation; no original-domain deployment is configured here.

## Working constraints

Keep the engine headless, the UI thin, and the document format explicit. Preserve MIT/Apache licensing and asset provenance. Use one Cargo build worker and the shared target directory on development machines. Heavy builds and discretionary QA require an explicit user request.
