# PhotoStore roadmap

This is the working roadmap for the independent PhotoStore product. It records the baseline inherited from the PhotoCraft codebase and the next product work. It deliberately avoids percentage claims until PhotoStore has its own measurements.

## Baseline

- The native desktop shell, headless CLI, automation/MCP entry points, and WebAssembly shell already share a Rust command engine.
- Layered documents, masks, adjustments, effects, type, vectors, brushes, PSD/PSB I/O, OpenRaster, flat codecs, and the native `.pcraft` format are present in the inherited tree.
- Smart Objects and smart filters have document and PSD mapping infrastructure, but deeper editing, transform fidelity, filter coverage, and linked-source workflows remain product work.
- A timeline UI module exists in the inherited tree, but PhotoStore still needs a coherent persisted model for video timeline and frame animation workflows.
- The product identity, executable names, configuration directories, packaging metadata, and icon are being separated from the inherited PhotoCraft identity.

## Priorities

1. Establish PhotoStore identity end to end: native window/About/home copy, OS registration, installers, config migration policy, `photostore` and `photostore-cli`, and a first independent release workflow.
2. Design and implement a persisted animation model covering frame sequences, timing, exposure, playback, import/export boundaries, and the video timeline. Keep it command-driven and usable from automation.
3. Deepen Smart Objects: contents editing, linked and embedded sources, shared contents identity, non-destructive transforms and warps, smart-filter editing, cache invalidation, and faithful PSD round trips.
4. Improve brush behavior and interaction: pressure/tilt/rotation dynamics, smoothing, presets, tablet edge cases, deterministic replay, and responsive large-document painting.
5. Harden PSD and command coverage around the workflows PhotoStore promises. Add focused fixtures and measurements when a feature is implemented; avoid claiming parity before it is measured.

## CI and release follow-up

The inherited workflow recipes are retained for reference but are manual `workflow_dispatch` jobs only. Automatic push, pull-request, scheduled performance, and deployment triggers are disabled while the independent repository is established. A future PhotoStore CI pass should choose its own checks, release signing, artifact storage, and documentation host before enabling automation; no original-domain deployment is configured here.

## Working constraints

Keep the engine headless, the UI thin, and the document format explicit. Preserve MIT/Apache licensing and asset provenance. Use one Cargo build worker and the shared target directory on development machines. Heavy builds and discretionary QA require an explicit user request.
