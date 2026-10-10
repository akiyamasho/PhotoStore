# PhotoStore roadmap

This is the working roadmap for the independent PhotoStore product. It records the implemented foundation and the next product work. It deliberately avoids percentage claims until PhotoStore has its own measurements.

## Baseline

- The native desktop shell, headless CLI, automation/MCP entry points, and WebAssembly shell already share a Rust command engine.
- Layered documents, masks, adjustments, effects, type, vectors, brushes, PSD/PSB I/O, OpenRaster, flat codecs, and the native `.pcraft` format are present in the inherited tree.
- Smart Objects and smart filters have persisted document mapping, animated child timelines, linked and embedded source handling, transform reset, cache invalidation, and PSD mapping infrastructure; deeper editing and full external-format acceptance remain product work.
- The command-driven timeline foundation supports persisted video timelines and frame animation workflows, including timing, playback, layer states, frame stacks, and native UI/CLI integration.
- PhotoStore identity, executable names, configuration directories, packaging metadata, and icons are implemented independently of the inherited PhotoCraft identity.

## Priorities

1. Extend animation with audio, transitions, complex tween interpolation, and robust video import/export boundaries.
2. Deepen Smart Objects with contents editing, shared contents identity, smart-filter editing, linked-source workflows, and faithful PSD acceptance.
3. Improve brush behavior and interaction: pressure/tilt/rotation dynamics, smoothing, presets, tablet edge cases, deterministic replay, and responsive large-document painting.
4. Harden PSD and command coverage around the workflows PhotoStore promises. Add focused fixtures and measurements when a feature is implemented; avoid claiming parity before it is measured.

The animation and Smart Object pass now preserves embedded child timelines, advances animated
sources during parent playback, bounds decoded and flattened source caches, and exposes an
undoable Smart Object transform reset. Audio, transitions, complex tween interpolation, and full
external-format export fidelity remain open gaps.

## CI and release follow-up

The inherited workflow recipes are retained for reference but are manual `workflow_dispatch` jobs only. Automatic push, pull-request, scheduled performance, and deployment triggers are disabled while the independent repository is established. A future PhotoStore CI pass should choose its own checks, release signing, artifact storage, and documentation host before enabling automation; no original-domain deployment is configured here.

## Working constraints

Keep the engine headless, the UI thin, and the document format explicit. Preserve MIT/Apache licensing and asset provenance. Use one Cargo build worker and the shared target directory on development machines. Heavy builds and discretionary QA require an explicit user request.
