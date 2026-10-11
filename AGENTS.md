# PhotoStore contributor guide

PhotoStore is an independent Rust-native image editor. Its GitHub repository is private by default and has no upstream sync relationship. Preserve existing history, licences, NOTICE, and attribution records.

## Product identity

Use **PhotoStore** in user-facing text, window titles, About/home screens, OS menus, installers, release artifacts, and executable names. All internal crates use the `photostore-*` naming. Exposed commands are `photostore` and `photostore-cli`. The native app uses a PhotoStore-specific bundle identifier and configuration directory so it never writes inherited PhotoCraft preferences. PhotoStore is maintained as a personal fork for the owner's workflows, with no upstream sync relationship.

## Architecture

Keep the Rust layering intact: foundation and format crates at the bottom; document and operations above them; algorithms, paint, text, and vector services above that; compose/gpu/format and I/O above the document; engine and automation behind the UI; native egui/wgpu, CLI, and web shells at the top. User-visible behaviour belongs in the command registry so UI, CLI, automation, and persistence share one implementation. Smart Objects, timeline state, and frame animation must remain serializable document data.

## Reliability

Input from files, commands, automation, and users is untrusted. Return actionable errors, validate bounds and sizes, and avoid panic paths in non-test code. Do not assume 8-bit RGB, one allocation, or a single platform. Keep the clean-room boundary: use public specifications and original implementations, and preserve a licence/provenance row for every third-party or generated asset.

## Resource use

Use `CARGO_BUILD_JOBS=1` for builds and tests. Use the shared Cargo target directory; do not create per-agent 10 GB target directories. Avoid fetching corpora or running heavy builds unless the task requires it. Do not run discretionary QA or validation passes unless the user explicitly opts in.

## Recording and release documentation

Recording is off by default. Do not enable capture, tracing, or session recording as a
side effect of normal startup, development, release, or screenshot work.

Every release must document each released feature or fix with a real PhotoStore screenshot.
Capture the relevant flow with disposable demo documents only; never include the owner's
private, unsaved, or unrelated artwork. Store compressed, readable assets under
`docs/screenshots/<version>/` as lossless PNG or readable WebP. The assets must be committed
before the final release tag. Release Markdown and the README must reference the actual
committed images using links that remain valid for this private GitHub repository. Keep a
short evidence record for each image (feature/fix, demo document, source version, and known
limits). Statuses must distinguish implemented, preview, verified, and published. Do not
claim full Photoshop parity from menu wiring or an unverified UI path.

When a release needs new screenshots, use this order: implement and run the required
compiler; make one provisional preview build, labeled as a preview and treated as one
shared-target job; capture and legibility-check the UI screenshots; commit the docs and
README image references; tag the exact source SHA; then perform the incremental final
rebuild and verify its exact SHA and package integrity before publishing. Never retag or
publish provisional binaries. Older immutable tags remain unchanged. A current release
may be supplemented with a screenshot link only when its source version is verified.

## Changes

Read the relevant crate and docs before editing. Keep manifests valid, preserve unrelated work, and make focused changes. Run only checks requested by the user or required to establish that an explicitly requested change is safe. Record significant product or format decisions in `docs/decisions.md`.

## Local development installation policy

The consolidated `PhotoStore-dev` bundle tracks integrated main development. Goal work is delivered
to `PhotoStore-dev-<goal>`, with its own bundle identifier, executable, configuration, recovery,
presets, logs, portable data, UI state, and process identity. Goal candidates must never be
relabeled into another goal or into the consolidated bundle.

Both development variants preserve live unsaved documents and recovery data, never register
document associations, and must be launched in the background without interrupting the user's
active UI. Production `PhotoStore.app` remains untouched. Production installation, tagging,
release, publication, or promotion happens only after an explicit user request.

The repository's recording, screenshot evidence, resource, licensing, and release policies
continue to apply to consolidated and goal-specific development work.

